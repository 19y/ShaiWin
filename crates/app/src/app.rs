//! The Windows shell: one layered, always-on-top, mostly click-through window
//! that draws the line, plus the tray icon, the hot keys and the orchestration
//! that parks and restores other people's windows.
//!
//! Everything runs on the UI thread. The only background work is snapshots,
//! icon extraction and planning a park, which can block on another process.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use shaiwin_core::layout::SAG_PERCENT_MAX;
use shaiwin_core::theme::accent_for;
use shaiwin_core::{LineLayout, Spring};
use shaiwin_platform_win::monitor::{self, Monitor};
use shaiwin_platform_win::park::{self, ParkError, Parked};
use shaiwin_platform_win::window::{self, Rejection, WindowInfo};
use shaiwin_platform_win::Rect as WRect;
use tiny_skia::{IntSize, Pixmap};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture,
    UnregisterHotKey, VK_LBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::i18n::t;
use crate::journal;
use crate::line::{Card, Line, CARD_H, PIN_TOP};
use crate::render::{Painter, Ui};
use crate::settings::{self, Settings, SAG_AUTO};
use crate::worker::{Job, Reply, Worker};
use crate::{ico, settings_ui, tray};

const CLASS: PCWSTR = w!("ShaiWin.Line");
const TIMER_FRAME: usize = 1;
const TIMER_POLL: usize = 2;
/// Frame intervals, in milliseconds. The fast one is what smoothness wants; the
/// slower ones are what a frame that costs more than its slot can afford.
const FRAME_MS: u32 = 16;
const FRAME_MS_HALF: u32 = 33;
const FRAME_MS_SLOW: u32 = 50;
const HOTKEY_HANG: i32 = 1;
const HOTKEY_TOGGLE: i32 = 2;

const CMD_HANG: u32 = 1;
const CMD_TAKE_ALL: u32 = 2;
const CMD_TOGGLE: u32 = 3;
const CMD_LOGIN: u32 = 4;
const CMD_QUIT: u32 = 5;
const CMD_TAKE_ONE: u32 = 6;
const CMD_SETTINGS: u32 = 7;
const CMD_UNHOOK: u32 = 8;

/// Automation hook: `PostMessage(hwnd, WM_HANG_HWND, 0, hwnd)`. Used by the
/// smoke test and by external scripts; the CLI may grow a wrapper later.
const WM_HANG_HWND: u32 = WM_APP + 10;
/// Automation hook: take every parked window down, exactly like the tray item.
const WM_TAKE_ALL: u32 = WM_APP + 11;
/// Automation hook: write the next composed frame as a PNG into
/// `SHAIWIN_DUMP_DIR`. Lets a headless test see what would have been drawn.
const WM_DUMP_FRAME: u32 = WM_APP + 12;
/// Automation hook: take one window down (keep the hook) by HWND in lParam.
const WM_TAKE_HWND: u32 = WM_APP + 13;
/// Automation hook: stop managing one window by HWND in lParam.
const WM_UNHOOK_HWND: u32 = WM_APP + 14;

/// The sentinel hit rectangle for the "show the line" pill (see `peek`).
pub const PEEK_ID: u64 = u64::MAX;

/// Where a hang request came from, which decides where the window opens again.
///
/// A window dragged onto the line was put away by hand, and comes back in the
/// middle of the screen the line is on. A window hung up by hotkey or from the
/// tray is in the way rather than placed somewhere, so it goes back exactly
/// where it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HangSource {
    Drag,
    Command,
}

impl HangSource {
    fn centred(self) -> bool {
        matches!(self, HangSource::Drag)
    }
}

/// How long the cursor rests at the top edge before the line comes down.
const DWELL: Duration = Duration::from_millis(180);
/// The same, while the line is empty. A little longer, so a cursor that merely
/// grazes the top edge on its way somewhere else does not summon an empty line.
const DWELL_EMPTY: Duration = Duration::from_millis(450);
/// How long after leaving the line before it tucks away.
const AWAY: Duration = Duration::from_millis(420);
/// How long an empty line waits after the last card comes off before it goes
/// away on its own. Long enough to read the hint, short enough to stay out of
/// the way. A line the user asked for is pinned and never hides by itself.
const EMPTY_GRACE: Duration = Duration::from_millis(900);
/// A window that comes down stays "hooked": minimizing it hangs it up again.
/// Ignore a minimize in the first moments after taking it down, so restoring it
/// cannot immediately bounce it back.
const REHANG_GRACE: Duration = Duration::from_millis(650);
/// How long after parking a window to ignore a foreground transition that
/// arrives on its own. Long enough for Windows to finish showing the window it
/// has just moved, short enough that Alt+Tabbing to a card that has been hanging
/// for a moment still brings it back.
const PARK_FOREGROUND_GRACE: Duration = Duration::from_millis(1200);
/// How often to retry handing the focus away from a parked window that took it
/// from a hand-over and is still holding it. Each attempt activates another
/// window, and the shell repaints the taskbar for every activation, so there is
/// exactly one retry and it is quick.
const STUCK_RETRY: Duration = Duration::from_millis(300);
/// How long to keep retrying before leaving the focus where it is. Showing the
/// window instead would be the random card popping open again, which is worse.
const STUCK_GIVE_UP: Duration = Duration::from_millis(700);
/// How long the little "show the line" pill stays when the pointer leaves it.
const PEEK_AWAY: Duration = Duration::from_millis(500);
const PEEK_LIFE: Duration = Duration::from_secs(6);

pub enum Deferred {
    None,
    Menu(MenuKind),
}

#[derive(Clone, Copy)]
pub enum MenuKind {
    Tray,
    Card,
}

struct Dib {
    hdc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Dib {
    fn new(w: i32, h: i32) -> Option<Dib> {
        unsafe {
            let hdc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(hdc, bitmap.into());
            Some(Dib { hdc, bitmap, old, bits: bits as *mut u8, w, h })
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.hdc);
        }
    }
}

/// A window drag we are watching, so releasing it in the hang zone hangs it.
struct DragWatch {
    hwnd: isize,
    /// Where the window was when the watch was created. A window that has not
    /// moved since is not being dragged, whatever else is going on under the
    /// pointer.
    started: WRect,
    moved: bool,
    /// Whether this drag has already reported that it reached the band while
    /// the line was tucked away. A hidden line stays out of the drag, and the
    /// reason belongs in the log once, not once per poll.
    told_tucked: bool,
}

/// A parked window holding the foreground nobody asked it to have.
#[derive(Clone, Copy)]
struct StuckFocus {
    hwnd: isize,
    since: Instant,
    last_try: Instant,
}

/// A press on a card. On its face it becomes a drag once the pointer travels
/// far enough, and otherwise releasing it takes the window down. On its side
/// edge it resizes the card instead.
#[derive(Clone, Copy)]
struct CardPress {
    id: u64,
    start: (i32, i32),
    grabbed: bool,
    /// Which side was grabbed, for a resize.
    edge: Option<crate::line::Edge>,
    /// The zoom the card had when the press began, and the pin it grows around.
    zoom0: f32,
    centre: f32,
    /// For a bottom-edge resize: where the card's body starts, how many pixels
    /// one unit of height zoom is, and the height it had when the press began.
    /// All three are frozen at the press, so the top edge stays on the pin while
    /// the card grows.
    body_top: f32,
    height_unit: f32,
    zoom_y0: f32,
    zoomed: bool,
}

pub struct App {
    hwnd: HWND,
    self_pid: u32,
    self_elevated: bool,

    settings: Settings,
    line: Line,
    painter: Painter,
    worker: Worker,
    dib: Option<Dib>,
    /// The frame being composed. Reused between frames: a fresh full-width
    /// pixmap sixty times a second is a bill paid in proportion to the screen.
    canvas: Option<Pixmap>,

    monitor: Monitor,
    layout: LineLayout,
    panel: WRect,
    origin: (i32, i32),

    revealed: bool,
    pinned: bool,
    /// The user put the line away on purpose (the toggle hotkey or the tray
    /// menu). A line that merely tucked itself away because the pointer left is
    /// not dismissed, and a window dragged to the top still opens it and hangs.
    /// A dismissed line is inert: it must not light up, come back or accept a
    /// drop just because a window is dragged over the place it used to occupy.
    dismissed: bool,
    hold_until: Option<Instant>,
    slide: f32,
    shown: bool,
    anim_running: bool,
    dirty: bool,
    last_frame: Instant,
    /// Smoothed cost of composing and presenting one frame, and the frame
    /// interval that cost has earned. A machine that cannot afford 60 redraws a
    /// second of a screen-wide layered window is better served by a calm 30 than
    /// by a 60 Hz stutter that holds the whole desktop up with it.
    frame_cost: f32,
    frame_ms: u32,

    hover: Option<u64>,
    hits: Vec<(u64, WRect)>,
    click_through: bool,

    drag: Option<DragWatch>,
    /// Where the pointer was when this press was first seen. A press that has
    /// not travelled is a click, not a drag, whatever the window happens to be
    /// doing under it.
    press_at: Option<(i32, i32)>,
    /// Whether the currently dragged window's own top edge is in the hang band.
    /// It is what lights the rope; the release asks the same question again.
    band_hot: bool,
    buttons_were_down: bool,
    hot_since: Option<Instant>,
    away_since: Option<Instant>,
    empty_since: Option<Instant>,
    /// False after an explicit "hide line" until the pointer leaves the top
    /// edge, so hiding the line while the cursor rests there sticks.
    hover_ready: bool,
    /// The small "show the line" pill, used instead of hover-to-open when the
    /// user turned that setting off.
    peek: bool,
    peek_hover: bool,
    peek_since: Option<Instant>,
    peek_away: Option<Instant>,
    /// The card being dragged along the line (reordering).
    card_press: Option<CardPress>,
    /// Foreground window as of the last poll, to notice when a parked window is
    /// brought forward by the taskbar or Alt+Tab.
    last_foreground: isize,
    /// A parked window that came to the front while still off screen, waiting
    /// for the next poll: the window it came from may not have finished
    /// minimizing yet, so one sample cannot tell a hand-over from an activation.
    pending_foreground: Option<(isize, isize)>,
    /// A parked window that got the foreground by hand-over and would not give
    /// it up. The focus is retried for a moment; the window is never shown.
    stuck_foreground: Option<StuckFocus>,
    /// The window just parked, and when. Showing a window we have just moved off
    /// screen can hand it the foreground by itself, and the transition below
    /// would read that as "the user asked to see it" and take the card straight
    /// back down again.
    recent_park: Option<(isize, Instant)>,

    /// Large icon for Alt-Tab and small icon for the title bar, taskbar and
    /// tray. Both are loaded at the size Windows asks for at this DPI, so no
    /// surface has to rescale one of them into a different-looking icon.
    window_icon: HICON,
    tray_icon: HICON,
    /// Text last given to the shell. Sending an unchanged tooltip is a shell
    /// round trip that redraws the notification area for nothing.
    tray_tip: String,
    settings_wnd: HWND,
    last_poll: Instant,
    polls: u32,
    icons: HashMap<String, Pixmap>,
    /// Which card a right-click menu belongs to.
    card_menu_target: Option<u64>,
    /// `SHAIWIN_DUMP_DIR`, when set: where WM_DUMP_FRAME writes PNGs.
    dump_dir: Option<PathBuf>,
    dump_request: bool,
    dump_seq: u32,
    /// Set once if UpdateLayeredWindow refuses a frame, so the log has the
    /// reason without one line per frame.
    present_failed: bool,

    quitting: bool,
}

pub fn register_class(hinst: HINSTANCE, big: HICON, small: HICON) {
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big,
            hIconSm: small,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
    }
}

impl App {
    pub fn create() -> Option<*mut App> {
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
            let settings = Settings::load();
            let window_icon = ico::load(ico::large_size()).unwrap_or_default();
            let tray_icon = ico::load(ico::small_size()).unwrap_or_default();
            register_class(hinst, window_icon, tray_icon);
            settings_ui::register_class(hinst, window_icon, tray_icon);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS,
                w!("ShaiWin"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinst),
                None,
            )
            .ok()?;
            let raw = hwnd.0 as isize;

            let monitor = monitor::at_point(monitor::cursor().0, monitor::cursor().1);
            let layout = settings.layout_for(monitor.bounds.width() as f32, monitor.scale);
            let panel = WRect {
                left: monitor.bounds.left,
                top: monitor.bounds.top,
                right: monitor.bounds.right,
                bottom: monitor.bounds.top + layout.panel_height() as i32,
            };

            let worker = Worker::spawn(raw);
            let app = Box::new(App {
                hwnd,
                self_pid: std::process::id(),
                self_elevated: window::self_elevated(),
                painter: Painter::new(),
                settings,
                line: Line::new(),
                worker,
                dib: None,
                canvas: None,
                monitor,
                layout,
                panel,
                origin: (panel.left, panel.top),
                revealed: false,
                pinned: false,
                dismissed: false,
                hold_until: None,
                slide: 0.0,
                shown: false,
                anim_running: false,
                dirty: true,
                last_frame: Instant::now(),
                frame_cost: 0.0,
                frame_ms: FRAME_MS,
                hover: None,
                hits: Vec::new(),
                click_through: true,
                drag: None,
                press_at: None,
                band_hot: false,
                buttons_were_down: false,
                hot_since: None,
                away_since: None,
                empty_since: None,
                hover_ready: true,
                peek: false,
                peek_hover: false,
                peek_since: None,
                peek_away: None,
                card_press: None,
                last_foreground: window::foreground().map(window::raw_of).unwrap_or(0),
                pending_foreground: None,
                stuck_foreground: None,
                recent_park: None,
                window_icon,
                tray_icon,
                tray_tip: crate::i18n::tray_tip(0),
                settings_wnd: HWND::default(),
                last_poll: Instant::now(),
                polls: 0,
                icons: HashMap::new(),
                card_menu_target: None,
                dump_dir: std::env::var_os("SHAIWIN_DUMP_DIR").map(PathBuf::from),
                dump_request: false,
                dump_seq: 0,
                present_failed: false,
                quitting: false,
            });
            let ptr = Box::into_raw(app);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            (*ptr).start();
            Some(ptr)
        }
    }

    fn start(&mut self) {
        if !self.tray_icon.is_invalid() {
            self.tray_tip = crate::i18n::tray_tip(0);
            tray::add(self.hwnd, self.tray_icon, &self.tray_tip);
        }
        unsafe {
            if !self.window_icon.is_invalid() {
                let _ = SendMessageW(
                    self.hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(self.window_icon.0 as isize)),
                );
            }
            if !self.tray_icon.is_invalid() {
                let _ = SendMessageW(
                    self.hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_SMALL as usize)),
                    Some(LPARAM(self.tray_icon.0 as isize)),
                );
            }
        }
        let mut failed: Vec<String> = Vec::new();
        unsafe {
            if self.settings.hang_vk != 0 {
                let mods = HOT_KEY_MODIFIERS(self.settings.hang_mods) | MOD_NOREPEAT;
                if let Err(error) = RegisterHotKey(Some(self.hwnd), HOTKEY_HANG, mods, self.settings.hang_vk) {
                    crate::log!("hotkey {} was refused: {error}", self.settings.hang_label());
                    failed.push(self.settings.hang_label());
                }
            }
            if self.settings.toggle_vk != 0 {
                let mods = HOT_KEY_MODIFIERS(self.settings.toggle_mods) | MOD_NOREPEAT;
                if let Err(error) =
                    RegisterHotKey(Some(self.hwnd), HOTKEY_TOGGLE, mods, self.settings.toggle_vk)
                {
                    crate::log!("hotkey {} was refused: {error}", self.settings.toggle_label());
                    failed.push(self.settings.toggle_label());
                }
            }
            SetTimer(Some(self.hwnd), TIMER_POLL, 50, None);
        }
        if !failed.is_empty() && !self.tray_icon.is_invalid() {
            let body = format!(
                "{}：{}。可以在托盘菜单的「设置」里换一个组合。",
                t("hotkey_failed"),
                failed.join("、")
            );
            tray::balloon(self.hwnd, self.tray_icon, t("app_name"), &body, true);
        }
        self.recover();
        self.sync_journal();

        if !self.settings.welcomed {
            self.settings.welcomed = true;
            self.settings.save();
            self.revealed = true;
            self.hold_until = Some(Instant::now() + Duration::from_secs(6));
            // The line is only useful once somebody knows it is there. Say how
            // it works, once.
            if !self.tray_icon.is_invalid() {
                tray::balloon(self.hwnd, self.tray_icon, t("howto_title"), t("howto_body"), false);
            }
            self.invalidate();
        }
        // Elevation is a real safety trade-off, not a success: say so once.
        if self.self_elevated {
            tray::balloon(self.hwnd, self.tray_icon, t("app_name"), t("elevated_tip"), true);
        }
        // The line is the only window this app has. Tucked away at the top edge
        // it reads as "ShaiWin is not running", so by default it stays down
        // until it is dismissed on purpose (the toggle shortcut un-pins it).
        if self.settings.always_on {
            self.revealed = true;
            self.pinned = true;
        }
        self.invalidate();
    }

    pub fn shutdown(&mut self) {
        self.stow_all_now();
        self.settings.save();
        unsafe {
            if !self.settings_wnd.is_invalid() {
                let _ = DestroyWindow(self.settings_wnd);
                self.settings_wnd = HWND::default();
            }
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_HANG);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_TOGGLE);
            let _ = KillTimer(Some(self.hwnd), TIMER_POLL);
            let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
        }
        tray::remove(self.hwnd);
        if !self.window_icon.is_invalid() {
            ico::release(self.window_icon);
        }
        if !self.tray_icon.is_invalid() {
            ico::release(self.tray_icon);
        }
    }

    // MARK: Recovery

    fn recover(&mut self) {
        let entries = journal::load();
        if entries.is_empty() {
            return;
        }
        let entries_len = entries.len();
        let mut survivors: Vec<(journal::Entry, WRect)> = Vec::new();
        for entry in entries {
            if !window::still_same_window(entry.hwnd, entry.pid, entry.process_start) {
                continue;
            }
            let Some(now) = window::rect_of(window::hwnd_of(entry.hwnd)) else { continue };
            if entry.hooked_away {
                // The window was on the desktop, hooked, when we stopped. No
                // window to put back: only the hook is worth recovering.
                survivors.push((entry, now));
                continue;
            }
            // If the window is no longer where we left it, somebody already took
            // it back; there is nothing to recover.
            if !park::still_hung(window::hwnd_of(entry.hwnd), &entry.parked) {
                continue;
            }
            survivors.push((entry, now));
        }
        if survivors.is_empty() {
            crate::log!("recover: {} journal entries, none still ours", entries_len);
            journal::save(&[]);
            return;
        }
        let parked_count = survivors.iter().filter(|(e, _)| !e.hooked_away).count();
        crate::log!(
            "recover: {} of {} entries still parked, {} hooked on the desktop",
            parked_count,
            entries_len,
            survivors.len() - parked_count
        );
        // Put the line back on the monitor the last hang happened on.
        let anchor = survivors[0].0.anchor;
        self.place(monitor::at_point(anchor.0, anchor.1));

        let mut revealed_any = false;
        for (entry, now) in survivors {
            let id = self.line.next_id();
            let icon = self.icon_for(&entry.exe).cloned();
            let card = Card {
                id,
                hwnd: entry.hwnd,
                pid: entry.pid,
                process_start: entry.process_start,
                title: entry.title.clone(),
                app: app_name(&entry.exe),
                exe: entry.exe.clone(),
                accent: accent_for(&entry.exe),
                placement: entry.placement,
                parked: entry.parked,
                window_rect: if entry.hooked_away { now } else { entry.placement.normal },
                centred: entry.centred,
                thumb: None,
                thumb_version: 0,
                icon,
                zoom: entry.zoom,
                zoom_y: entry.zoom_y,
                offset: entry.offset,
                x: Spring::new(0.0, 0.0, 0.0),
                sway: Spring::new(0.0, 0.0, 0.0),
                scale: 1.0,
                scale_goal: 1.0,
                anchor: entry.anchor,
                leaving: false,
                away: entry.hooked_away,
                away_at: None,
                unhook: false,
            };
            revealed_any |= !entry.hooked_away;
            self.line.recover(card, &self.layout);
            self.worker.send(Job::Snapshot { id, hwnd: entry.hwnd });
            if !self.icons.contains_key(&entry.exe) {
                self.worker.send(Job::Icon { id, exe_path: entry.exe.clone() });
            }
        }
        if revealed_any {
            self.revealed = true;
            self.dismissed = false;
            self.hold_until = Some(Instant::now() + Duration::from_secs(3));
        }
    }

    // MARK: Window messages

    fn on_message(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> (LRESULT, Deferred) {
        let mut deferred = Deferred::None;
        let result = match msg {
            crate::WM_EVENT => {
                self.drain_worker();
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_FRAME => self.animate(),
                    TIMER_POLL => self.poll(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                match wp.0 as i32 {
                    HOTKEY_HANG => {
                        if let Some(hwnd) = window::foreground() {
                            self.request_hang(window::raw_of(hwnd), HangSource::Command);
                        }
                    }
                    HOTKEY_TOGGLE => self.toggle_reveal(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_HANG_HWND => {
                let raw = lp.0;
                if raw != 0 {
                    self.request_hang(raw, HangSource::Command);
                }
                LRESULT(0)
            }
            WM_TAKE_HWND => {
                if let Some(id) = self.line.cards.iter().find(|c| c.hwnd == lp.0).map(|c| c.id) {
                    self.take_down(id);
                }
                LRESULT(0)
            }
            WM_UNHOOK_HWND => {
                if let Some(id) = self.line.cards.iter().find(|c| c.hwnd == lp.0).map(|c| c.id) {
                    self.unhook(id);
                }
                LRESULT(0)
            }
            WM_TAKE_ALL => {
                self.take_all();
                LRESULT(0)
            }
            WM_DUMP_FRAME => {
                if self.dump_dir.is_some() {
                    self.dump_request = true;
                    self.invalidate();
                }
                LRESULT(0)
            }
            tray::WM_TRAY => {
                match (lp.0 & 0xFFFF) as u32 {
                    WM_LBUTTONUP => self.toggle_reveal(),
                    WM_RBUTTONUP => deferred = Deferred::Menu(MenuKind::Tray),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                self.command((wp.0 & 0xFFFF) as u32);
                LRESULT(0)
            }
            WM_NCHITTEST => {
                let point = screen_point(lp);
                let local = (point.x - self.origin.0, point.y - self.origin.1);
                if self.card_at(local).is_some() || self.peek_at(local) {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                }
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_SETCURSOR => {
                let (x, y) = monitor::cursor();
                let local = (x - self.origin.0, y - self.origin.1);
                let over = self.card_at(local).is_some() || self.peek_at(local);
                let edge = self.card_at(local).filter(|id| *id != PEEK_ID).and_then(|id| {
                    self.card_rect(id)
                        .and_then(|rect| crate::line::edge_at(&rect, local, self.layout.scale))
                });
                unsafe {
                    let icon = match edge {
                        Some(crate::line::Edge::Bottom) => IDC_SIZENS,
                        Some(crate::line::Edge::BottomLeft) => IDC_SIZENESW,
                        Some(crate::line::Edge::BottomRight) => IDC_SIZENWSE,
                        Some(_) => IDC_SIZEWE,
                        None if over => IDC_HAND,
                        None => IDC_ARROW,
                    };
                    if let Ok(cursor) = LoadCursorW(None, icon) {
                        SetCursor(Some(cursor));
                    }
                }
                LRESULT(1)
            }
            WM_LBUTTONDOWN => {
                let point = client_point(lp);
                crate::log!("press at {point:?}: card={:?}", self.card_at(point));
                if let Some(id) = self.card_at(point) {
                    if id != PEEK_ID {
                        // The side edges resize the width, the bottom edge the
                        // height, and the bottom corners both at once. The face
                        // drags or clicks.
                        let rect = self.card_rect(id);
                        let edge = rect
                            .and_then(|rect| crate::line::edge_at(&rect, point, self.layout.scale));
                        let (zoom0, centre, zoom_y0) = self
                            .line
                            .get(id)
                            .map(|card| (card.zoom, card.x.x, card.zoom_y))
                            .unwrap_or((1.0, point.0 as f32, 1.0));
                        // How tall one unit of vertical zoom is on screen right
                        // now: the card is drawn at its own zoom and the line's
                        // packed fit, and the body starts below the pin.
                        let k = self.line.get(id).map_or(1.0, |card| card.scale) * self.line.fit(&self.layout);
                        let height_unit = CARD_H * self.layout.scale * k;
                        let body_top = rect.map_or(0.0, |rect| rect.bottom as f32 - height_unit * zoom_y0);
                        self.card_press = Some(CardPress {
                            id,
                            start: point,
                            grabbed: false,
                            edge,
                            zoom0,
                            centre,
                            body_top,
                            height_unit,
                            zoom_y0,
                            zoomed: false,
                        });
                        self.line.dragging = None;
                        unsafe {
                            SetCapture(self.hwnd);
                        }
                        self.invalidate();
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                self.drag_card_to(client_point(lp));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let point = client_point(lp);
                if self.card_press.is_none() && self.peek_at(point) {
                    self.open_from_peek();
                } else {
                    self.finish_card_press(point);
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                self.cancel_card_press();
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let point = client_point(lp);
                if let Some(id) = self.card_at(point) {
                    if id != PEEK_ID {
                        self.card_menu_target = Some(id);
                        deferred = Deferred::Menu(MenuKind::Card);
                    }
                }
                LRESULT(0)
            }
            WM_PAINT => {
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                let (x, y) = monitor::cursor();
                self.place(monitor::at_point(x, y));
                self.invalidate();
                LRESULT(0)
            }
            WM_QUERYENDSESSION => {
                self.stow_all_now();
                LRESULT(1)
            }
            WM_ENDSESSION => {
                if wp.0 != 0 {
                    self.stow_all_now();
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                self.quitting = true;
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe {
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            m if m == taskbar_created() => {
                if !self.tray_icon.is_invalid() {
                    self.tray_tip = crate::i18n::tray_tip(self.line.live_count());
                    tray::add(self.hwnd, self.tray_icon, &self.tray_tip);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(self.hwnd, msg, wp, lp) },
        };
        (result, deferred)
    }

    fn finish_menu(&mut self, kind: MenuKind) -> Option<u32> {
        let menu = self.build_menu(kind);
        let hwnd = self.hwnd;
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
        let at = tray::cursor();
        let cmd = unsafe { TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, at.x, at.y, None, hwnd, None).0 as u32 };
        unsafe {
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        }
        (cmd != 0).then_some(cmd)
    }

    fn build_menu(&self, kind: MenuKind) -> HMENU {
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            let add = |id: u32, text: &str, flags: MENU_ITEM_FLAGS| {
                let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(menu, flags | MF_STRING, id as usize, PCWSTR(wide.as_ptr()));
            };
            let sep = || {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            };
            match kind {
                MenuKind::Card => {
                    add(CMD_TAKE_ONE, t("restore"), MF_ENABLED);
                    sep();
                    add(CMD_UNHOOK, t("unhook"), MF_ENABLED);
                }
                MenuKind::Tray => {
                    let current = window::foreground()
                        .filter(|h| {
                            let pid = window::pid_of(*h);
                            pid != 0 && pid != self.self_pid
                        })
                        .and_then(|h| window::check(h, self.self_pid).ok());
                    let hang = match &current {
                        Some(info) => {
                            format!("{}\t{}", truncate(&info.title, 40), self.settings.hang_label())
                        }
                        None => format!("{}\t{}", t("hang_short"), self.settings.hang_label()),
                    };
                    add(CMD_HANG, &hang, if current.is_some() { MF_ENABLED } else { MF_GRAYED });
                    add(CMD_TAKE_ALL, t("take_down_all"), if self.line.hooked_count() > 0 { MF_ENABLED } else { MF_GRAYED });
                    sep();
                    let toggle = if self.revealed { t("hide_line") } else { t("show_line") };
                    add(CMD_TOGGLE, &format!("{toggle}\t{}", self.settings.toggle_label()), MF_ENABLED);
                    add(CMD_SETTINGS, t("settings"), MF_ENABLED);
                    sep();
                    let check = if tray::start_at_login() { MF_CHECKED } else { MF_UNCHECKED };
                    add(CMD_LOGIN, t("open_at_login"), check | MF_ENABLED);
                    sep();
                    add(CMD_QUIT, t("quit"), MF_ENABLED);
                }
            }
            menu
        }
    }

    fn command(&mut self, id: u32) {
        match id {
            CMD_HANG => {
                if let Some(hwnd) = window::foreground() {
                    self.request_hang(window::raw_of(hwnd), HangSource::Command);
                }
            }
            CMD_TAKE_ALL => self.take_all(),
            CMD_TOGGLE => self.toggle_reveal(),
            CMD_SETTINGS => self.open_settings(),
            CMD_LOGIN => {
                tray::set_start_at_login(!tray::start_at_login());
            }
            CMD_QUIT => {
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            CMD_TAKE_ONE => {
                if let Some(id) = self.card_menu_target {
                    self.take_down(id);
                }
            }
            CMD_UNHOOK => {
                if let Some(id) = self.card_menu_target {
                    self.unhook(id);
                }
            }
            _ => {}
        }
    }

    // MARK: Hanging a window

    fn request_hang(&mut self, hwnd: isize, source: HangSource) {
        let h = window::hwnd_of(hwnd);
        let info = match window::check(h, self.self_pid) {
            Ok(info) => info,
            Err(rejection) => {
                crate::log!("hang {hwnd}: rejected ({rejection:?})");
                self.complain(rejection_message(rejection));
                return;
            }
        };
        crate::log!("hang {hwnd}: preparing {} ({})", info.exe, info.title);
        if self.settings.is_excluded(&info.exe) {
            self.complain(t("rej_excluded").to_string());
            return;
        }
        // A window that was taken down is still hooked, with its card hidden. The
        // only way to put it back on the line is to hang it again, and refusing
        // that with "already hung" - while nothing of it is on the line - is a
        // dead end the user cannot get out of.
        if let Some(id) = self.line.cards.iter().find(|c| c.hwnd == hwnd && c.away).map(|c| c.id) {
            crate::log!("hang {hwnd}: hooked window put back on the line");
            self.rehang(id, Some(source));
            return;
        }
        if self.line.contains_hwnd(hwnd) {
            self.complain(t("already_hung").to_string());
            return;
        }
        self.worker.send(Job::Prepare { info, source });
    }

    fn on_prepared(
        &mut self,
        info: WindowInfo,
        image: Option<shaiwin_platform_win::Image>,
        plan: Result<shaiwin_platform_win::park::ParkPlan, ParkError>,
        source: HangSource,
    ) {
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                self.complain(error.message());
                return;
            }
        };
        if self.line.contains_hwnd(info.hwnd) {
            return;
        }

        // The window may be on another monitor: the line follows it.
        let anchor = info.rect.center();
        let target_monitor = monitor::at_point(anchor.0, anchor.1);
        if target_monitor.handle != self.monitor.handle {
            self.place(target_monitor);
        }

        let id = self.line.next_id();
        let exe = info.exe.clone();
        let accent = accent_for(&exe);
        let icon = self.icon_for(&exe).cloned();
        let thumb = image.and_then(|img| to_pixmap(&img));

        let mut card = Card {
            id,
            hwnd: info.hwnd,
            pid: info.pid,
            process_start: info.process_start,
            title: info.title.clone(),
            app: info.app_name(),
            exe: exe.clone(),
            accent,
            placement: plan.placement,
            parked: Parked { placement: plan.placement, parked: plan.target, was_maximized: plan.was_maximized },
            window_rect: info.rect,
            centred: source.centred(),
            thumb,
            thumb_version: 1,
            icon,
            zoom: 1.0,
            zoom_y: 1.0,
            offset: 0.0,
            x: Spring::new(0.0, 0.0, 0.0),
            sway: Spring::new(0.0, 0.0, 0.0),
            scale: 1.0,
            scale_goal: 1.0,
            anchor,
            leaving: false,
            away: false,
            away_at: None,
            unhook: false,
        };

        // Journal first, then move: if we die in between, startup recovery puts
        // the window back where it was.
        let entry = journal_entry(&card);
        self.write_journal_append(entry);

        match park::execute(window::hwnd_of(info.hwnd), &plan) {
            Ok(parked) => {
                crate::log!("parked hwnd={} at {:?} (from {:?})", info.hwnd, parked.parked, plan.placement.normal);
                card.parked = parked;
                // The window is off screen but still in front. Give the foreground
                // away, so that clicking its taskbar button activates it (which
                // brings it back down) instead of minimizing it.
                self.recent_park = Some((info.hwnd, Instant::now()));
                self.hand_focus_off(window::hwnd_of(info.hwnd), Instant::now());
                let reduced = self.settings.reduced_motion;
                self.line.hang(card, &self.layout, reduced);
                self.revealed = true;
                self.dismissed = false;
                self.hold_until = Some(Instant::now() + Duration::from_secs(2));
                self.sync_journal();
                self.invalidate();
                if !self.icons.contains_key(&exe) {
                    self.worker.send(Job::Icon { id, exe_path: exe });
                }
            }
            Err(error) => {
                crate::log!("park failed hwnd={}: {:?}", info.hwnd, error);
                self.sync_journal();
                self.complain(error.message());
            }
        }
    }

    // MARK: Taking a window down

    /// Puts the window back in the same frame as the click.
    ///
    /// There is no animation on the way down on purpose: anything between the
    /// click and the window being in front reads as "the click did nothing",
    /// and the card is gone by then, so the user has nothing left to click.
    fn take_down(&mut self, id: u64) {
        if self.line.take_down(id) {
            self.finish_take_down(id);
        }
    }

    /// Takes a window down and stops managing it. The card menu offers this;
    /// a plain click keeps the hook so a later minimize brings the window back.
    fn unhook(&mut self, id: u64) {
        if let Some(card) = self.line.get_mut(id) {
            card.unhook = true;
        }
        if self.line.get(id).is_some_and(|c| c.away) {
            self.painter.forget(id);
            self.line.remove(id);
            self.sync_journal();
            self.invalidate();
        } else {
            self.take_down(id);
        }
    }

    fn finish_take_down(&mut self, id: u64) {
        let Some(card) = self.line.get(id) else { return };
        let hwnd = card.hwnd;
        let parked = restore_record(card, self.monitor.work);
        let unhook = card.unhook;
        let before = window::rect_of(window::hwnd_of(hwnd));
        let ok = park::restore(window::hwnd_of(hwnd), &parked);
        let after = window::rect_of(window::hwnd_of(hwnd));
        crate::log!(
            "take_down {id}: hwnd={hwnd} ok={ok} unhook={unhook} before={before:?} after={after:?} target={:?}",
            parked.placement.normal
        );
        if !ok {
            // Keep the card: the window is still off screen, and hiding the
            // only way to get it back would strand it.
            self.line.cancel_take_down(id);
            self.complain(t("restore_failed").to_string());
            self.invalidate();
            return;
        }
        if self.line.dragging == Some(id) {
            self.line.dragging = None;
        }
        if unhook {
            self.painter.forget(id);
            self.line.remove(id);
        } else {
            // The window is on the desktop, but it is still hooked. Hide the
            // card and wait for a minimize (or an explicit unhook).
            self.line.detach(id, Instant::now());
            self.show_hook_hint();
        }
        self.sync_journal();
        self.invalidate();
    }

    fn show_hook_hint(&mut self) {
        if self.settings.hook_taught || self.tray_icon.is_invalid() {
            return;
        }
        self.settings.hook_taught = true;
        self.settings.save();
        tray::balloon(
            self.hwnd,
            self.tray_icon,
            t("taken_one"),
            t("hook_hint"),
            false,
        );
    }

    fn take_all(&mut self) {
        // Windows that are already on the desktop only need their hooks
        // dropped; restoring them would move them back to where they came from.
        let away: Vec<u64> = self.line.cards.iter().filter(|c| c.away).map(|c| c.id).collect();
        for id in away {
            self.painter.forget(id);
            self.line.remove(id);
        }
        let ids: Vec<u64> = self.line.cards.iter().filter(|c| !c.leaving && !c.away).map(|c| c.id).collect();
        crate::log!("take_all: {} card(s), unhooking", ids.len());
        if ids.is_empty() {
            self.sync_journal();
            self.invalidate();
            return;
        }
        self.revealed = true;
        self.dismissed = false;
        for id in &ids {
            if let Some(card) = self.line.get_mut(*id) {
                card.unhook = true;
            }
        }
        for id in ids {
            self.take_down(id);
        }
    }

    /// Every window goes back to its remembered place minimized.
    ///
    /// Quitting used to restore them all, which turned a line of eight cards
    /// into eight windows on the desktop at the same moment. Minimized, each one
    /// is a taskbar button, and a click on it brings it back exactly where it
    /// belongs. A window that refuses to minimize is put back instead: leaving it
    /// off screen with nothing pointing at it would strand it.
    fn stow_all_now(&mut self) {
        let work = self.monitor.work;
        let cards: Vec<(u64, isize, Parked, bool)> = self
            .line
            .cards
            .iter()
            .map(|c| (c.id, c.hwnd, restore_record(c, work), c.away))
            .collect();
        for (id, hwnd, parked, away) in cards {
            if !away
                && window::is_alive(hwnd)
                && !park::stow(window::hwnd_of(hwnd), &parked)
            {
                crate::log!("quit: hwnd={hwnd} would not stay minimized; putting it back instead");
                park::restore(window::hwnd_of(hwnd), &parked);
            }
            self.painter.forget(id);
            self.line.remove(id);
        }
        journal::clear();
    }

    // MARK: Worker

    fn drain_worker(&mut self) {
        let mut replies = Vec::new();
        while let Ok(reply) = self.worker.replies.try_recv() {
            replies.push(reply);
        }
        for reply in replies {
            match reply {
                Reply::Prepared { info, image, plan, source } => self.on_prepared(info, image, plan, source),
                Reply::Snapshotted { id, image } => {
                    if let Some(card) = self.line.get_mut(id) {
                        if let Some(img) = image {
                            card.thumb = to_pixmap(&img);
                            card.thumb_version += 1;
                        }
                    }
                    self.invalidate();
                }
                Reply::Iconed { id, image } => {
                    let Some(card) = self.line.get(id) else { continue };
                    let exe = card.exe.clone();
                    if let Some(img) = image {
                        if let Some(pix) = to_pixmap(&img) {
                            self.icons.insert(exe.clone(), pix.clone());
                            if let Some(card) = self.line.get_mut(id) {
                                card.icon = Some(pix);
                                card.thumb_version += 1;
                            }
                        }
                    }
                    self.invalidate();
                }
            }
        }
    }

    fn icon_for(&self, exe: &str) -> Option<&Pixmap> {
        self.icons.get(exe)
    }

    // MARK: Polling

    fn poll(&mut self) {
        let now = Instant::now();
        self.last_poll = now;
        self.polls = self.polls.wrapping_add(1);
        let (cx, cy) = monitor::cursor();

        if self.polls % 20 == 0 {
            self.refresh_titles();
        }
        if self.polls % 8 == 0 {
            self.poll_windows(now);
        }

        self.poll_drag(cx, cy);

        // A parked window that the user brings forward (taskbar, Alt+Tab,
        // another app activating it) is a request to see it. Move it back on
        // screen instead of leaving it invisible. The foreground is recorded
        // per poll, so simply hanging the window it was using is not a
        // transition.
        let foreground = window::foreground().map(window::raw_of).unwrap_or(0);
        if foreground != self.last_foreground {
            let previous = std::mem::replace(&mut self.last_foreground, foreground);
            self.pending_foreground = None;
            let just_parked = self
                .recent_park
                .is_some_and(|(hwnd, at)| hwnd == foreground && now.duration_since(at) < PARK_FOREGROUND_GRACE);
            if foreground != 0 && !just_parked {
                if let Some(id) = self.parked_card(foreground) {
                    let h = window::hwnd_of(foreground);
                    if !window::on_screen(h) {
                        // When the window in front is minimized or closed, Windows
                        // hands the foreground to whatever is next in the z-order.
                        // A parked window is visible, so it can be exactly that
                        // next window: it was never asked for, and treating the
                        // hand-over as a request is what made minimizing one
                        // window pop a random card onto the desktop.
                        if window::focus_handed_over(previous, self.self_pid) {
                            self.keep_on_line(id, h, now);
                        } else {
                            // The window it came from can still finish minimizing
                            // a moment after the shell moved the foreground, so one
                            // sample cannot tell a hand-over from an activation.
                            // Look again on the next poll.
                            self.pending_foreground = Some((foreground, previous));
                        }
                    }
                }
            }
        } else if let Some((parked, previous)) = self.pending_foreground {
            self.pending_foreground = None;
            if foreground == parked {
                if let Some(id) = self.parked_card(parked) {
                    if window::focus_handed_over(previous, self.self_pid) {
                        self.keep_on_line(id, window::hwnd_of(parked), now);
                    } else {
                        self.take_down(id);
                    }
                }
            }
        }

        // A hand-over left the focus on a window the user cannot see: typing
        // would go into it. Keep trying to give it to something visible, but
        // never show the parked window to get the focus back.
        if let Some(stuck) = self.stuck_foreground {
            if foreground != stuck.hwnd {
                self.stuck_foreground = None;
            } else if now.duration_since(stuck.since) >= STUCK_GIVE_UP {
                crate::log!("foreground: hwnd={} would not give up the focus; leaving it there", stuck.hwnd);
                self.stuck_foreground = None;
            } else if now.duration_since(stuck.last_try) >= STUCK_RETRY {
                // Only hand it to a real window this time. Activating the
                // desktop, or switching through candidates, repaints the
                // taskbar for every step and that is the flicker the user sees.
                if window::hand_off_foreground(window::hwnd_of(stuck.hwnd), self.self_pid) {
                    self.last_foreground = window::foreground().map(window::raw_of).unwrap_or(0);
                    self.stuck_foreground = None;
                    crate::log!("foreground: handed the focus away from the parked hwnd={} on retry", stuck.hwnd);
                } else {
                    self.stuck_foreground = Some(StuckFocus { last_try: now, ..stuck });
                }
            }
        }

        let top = self.monitor.bounds.top;
        let at_top = cy <= top + 2
            && cx >= self.monitor.bounds.left
            && cx < self.monitor.bounds.right;
        // After an explicit "hide line" the pointer is often still resting on
        // the edge. Wait until it leaves before hover may summon the line
        // again, otherwise the line pops straight back out from under it.
        if !at_top {
            self.hover_ready = true;
        }
        let local = (cx - self.origin.0, cy - self.origin.1);
        let peek_hover = self.peek && self.peek_at(local);
        if peek_hover != self.peek_hover {
            self.peek_hover = peek_hover;
            self.invalidate();
        }

        let inside = self.revealed
            && cx >= self.panel.left
            && cx < self.panel.right
            && cy >= self.panel.top - 2
            && cy < self.panel.bottom + 24;

        if !self.revealed {
            if self.settings.hover_open {
                if self.peek {
                    self.close_peek();
                }
                // Hovering the top edge works even when nothing is hanging:
                // that is how somebody with an empty line discovers the hint
                // and the gesture. It just takes a little longer than it does
                // with cards.
                let armed = at_top
                    && self.hover_ready
                    && !self.drag.as_ref().is_some_and(|d| d.moved)
                    && self.card_press.is_none();
                if armed {
                    let since = *self.hot_since.get_or_insert(now);
                    let wait = if self.line.live_count() == 0 { DWELL_EMPTY } else { DWELL };
                    if now.duration_since(since) >= wait {
                        self.hot_since = None;
                        self.revealed = true;
                        self.dismissed = false;
                        crate::log!("line shown: hover");
                        self.invalidate();
                    }
                } else {
                    self.hot_since = None;
                }
            } else {
                self.poll_peek(now, cx, cy, at_top);
            }
        } else {
            if self.peek {
                self.close_peek();
            }
            self.hot_since = None;
            if self.pinned
                || self.hold_until.is_some_and(|until| now < until)
                || self.drag.is_some()
                || self.card_press.is_some()
                || inside
                || self.hover.is_some()
            {
                self.away_since = None;
            } else {
                let since = *self.away_since.get_or_insert(now);
                if now.duration_since(since) >= AWAY {
                    self.away_since = None;
                    self.revealed = false;
                    self.band_hot = false;
                    self.hover = None;
                    self.away_since = None;
                    crate::log!("line hidden: pointer left");
                    self.invalidate();
                }
            }
        }

        // Hover and click-through only matter while the line is down.
        let next_hover = if self.revealed && inside {
            self.card_at(local).filter(|id| *id != PEEK_ID)
        } else {
            None
        };
        if next_hover != self.hover {
            self.hover = next_hover;
            self.invalidate();
        }
        let over_card = next_hover.is_some();
        self.set_click_through(!over_card && !peek_hover && self.card_press.is_none());

        // An empty line is only interesting for a moment. It goes away on its
        // own, unless the user pinned it (explicit "show line"), the pointer is
        // still in it, or a card is still flying back to its window.
        let empty = self.line.live_count() == 0
            && !self.peek
            && self.hold_until.is_none()
            && !self.pinned
            && !inside
            && self.drag.is_none()
            && self.card_press.is_none();
        if empty && self.revealed {
            let since = *self.empty_since.get_or_insert(now);
            if now.duration_since(since) >= EMPTY_GRACE {
                self.empty_since = None;
                self.revealed = false;
                self.band_hot = false;
                crate::log!("line hidden: empty");
                self.invalidate();
            }
        } else {
            self.empty_since = None;
        }
        if self.hold_until.is_some_and(|until| now >= until) {
            self.hold_until = None;
        }

        if self.line.gauge(now) {
            self.invalidate();
        }
    }

    /// The "show the line" pill, for people who turned off direct hover.
    fn poll_peek(&mut self, now: Instant, cx: i32, cy: i32, at_top: bool) {
        let top = self.monitor.bounds.top;
        if self.peek {
            let near = cx >= self.monitor.bounds.left
                && cx < self.monitor.bounds.right
                && cy >= top - 2
                && cy <= top + 120;
            if near {
                self.peek_away = None;
            } else {
                let since = *self.peek_away.get_or_insert(now);
                if now.duration_since(since) >= PEEK_AWAY {
                    self.close_peek();
                    return;
                }
            }
            if !self.peek_hover && self.peek_since.is_some_and(|t| now.duration_since(t) >= PEEK_LIFE) {
                self.close_peek();
            }
            return;
        }
        let armed =
            at_top && self.hover_ready && self.drag.is_none() && self.card_press.is_none();
        if armed {
            let since = *self.hot_since.get_or_insert(now);
            let wait = if self.line.live_count() == 0 { DWELL_EMPTY } else { DWELL };
            if now.duration_since(since) >= wait {
                self.hot_since = None;
                self.peek = true;
                self.peek_since = Some(now);
                self.peek_away = None;
                self.peek_hover = false;
                crate::log!("line peek shown");
                self.invalidate();
            }
        } else {
            self.hot_since = None;
        }
    }

    fn close_peek(&mut self) {
        if !self.peek {
            return;
        }
        self.peek = false;
        self.peek_hover = false;
        self.peek_since = None;
        self.peek_away = None;
        self.invalidate();
    }

    fn open_from_peek(&mut self) {
        if !self.peek {
            return;
        }
        self.peek = false;
        self.peek_hover = false;
        self.peek_since = None;
        self.peek_away = None;
        self.revealed = true;
        self.dismissed = false;
        // Not pinned: the pill was a way in, not a request to keep the line on
        // the screen forever. It tucks away when the pointer leaves it, and the
        // pill comes back when the pointer returns to the top edge.
        self.hold_until = None;
        self.empty_since = None;
        self.hot_since = None;
        self.away_since = None;
        crate::log!("line shown: peek button");
        self.invalidate();
    }

    fn poll_drag(&mut self, cx: i32, cy: i32) {
        let down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 };
        if !self.settings.drag_hang {
            // The line stays out of dragging altogether: no light and no
            // hang. Windows go up by hotkey or from the tray.
            if self.drag.take().is_some() {
                self.set_band_hot(false, None);
            }
            self.buttons_were_down = false;
            self.press_at = None;
            return;
        }
        if !down {
            let dropped = self.drag.take();
            if let Some(watch) = dropped {
                if watch.moved {
                    // Asked again at the pointer's current place rather than
                    // trusting the last poll's answer: a quick flick to the top
                    // can start and end between two polls. The window is asked
                    // what the system made of it: `IsZoomed` is the honest
                    // answer to "the user meant to enlarge it", and a top edge
                    // still flush is the snap about to become one.
                    let screen_top = self.monitor.bounds.top;
                    let rect = window::visible_rect(window::hwnd_of(watch.hwnd));
                    let zoomed = window::maximized(watch.hwnd);
                    let on_line = rect.is_some_and(|rect| self.layout.window_on_line(rect.top, screen_top));
                    let where_ = format!(
                        " (top={} edge={} band={})",
                        rect.map(|rect| rect.top).unwrap_or(screen_top),
                        screen_top,
                        self.layout.hang_depth()
                    );
                    if zoomed || rect.is_some_and(|rect| self.layout.window_flush(rect.top, screen_top)) {
                        crate::log!(
                            "drag {:#x}: {} at the top, so Windows gets to maximize it{where_}",
                            watch.hwnd,
                            if zoomed { "zoomed" } else { "flush" }
                        );
                    } else if on_line && self.dismissed {
                        // The line was put away before the release. Repainting
                        // it now to explain "nothing happened" would be exactly
                        // the line the user hid.
                        crate::log!(
                            "drag {:#x}: released on the line after the line was put away; not hanging{where_}",
                            watch.hwnd
                        );
                    } else if on_line {
                        crate::log!("drag {:#x}: released on the line at y={cy}{where_}", watch.hwnd);
                        self.request_hang(watch.hwnd, HangSource::Drag);
                    } else {
                        crate::log!("drag {:#x}: released off the line at y={cy}{where_}", watch.hwnd);
                    }
                }
            }
            self.set_band_hot(false, None);
            self.buttons_were_down = false;
            self.press_at = None;
            return;
        }

        // The window being dragged is normally the one under the pointer, but
        // the user can press on our own panel first, and a window can be
        // released and grabbed again inside one poll. So a press that did not
        // resolve into a window is retried while the button stays down, rather
        // than being dropped until the next press.
        let first = !self.buttons_were_down;
        self.buttons_were_down = true;
        if first {
            self.press_at = Some((cx, cy));
        }

        // A card in hand is not a window drag, whatever the point had under it
        // before the press landed: the click-through state can be a poll stale.
        if self.card_press.is_some() {
            if self.drag.take().is_some() {
                self.set_band_hot(false, None);
            }
            return;
        }

        if self.drag.is_none() {
            if !first && self.polls % 2 != 0 {
                return;
            }
            let mut found = None;
            let mut refusal = None;
            let mut ours_under = false;
            // The system's move loop names the window the user is dragging. It
            // is the only honest source: for the whole gesture the point can
            // hit-test to a helper window (a snap preview, a drag image, the
            // title bar's own child), none of which Windows is moving.
            let moving = moving_window(cx, cy);
            if let Some(mv) = moving {
                if window::pid_of(mv) == self.self_pid {
                    ours_under = true;
                } else {
                    match window::check(mv, self.self_pid) {
                        Ok(_) => found = Some(mv),
                        Err(rejection) => {
                            refusal = Some(format!(
                                "{rejection:?} on {} class={}",
                                window::describe(window::raw_of(mv)),
                                window::class_name(mv)
                            ))
                        }
                    }
                }
            }
            if found.is_none() && moving.is_none() {
                if let Some(under) = window::root_at(cx, cy) {
                    if window::pid_of(under) == self.self_pid {
                        ours_under = true;
                    } else {
                        match window::check(under, self.self_pid) {
                            Ok(_) => found = Some(under),
                            Err(rejection) => {
                                refusal = Some(format!(
                                    "{rejection:?} on {} class={}",
                                    window::describe(window::raw_of(under)),
                                    window::class_name(under)
                                ))
                            }
                        }
                    }
                }
            }
            if found.is_none() && !ours_under {
                // Some apps put a helper window under the pointer for the length
                // of a drag (Chromium's drag image, a tooltip). The window
                // Windows is actually moving is the foreground one, and the
                // pointer sits inside it.
                found = window::foreground().filter(|fg| {
                    window::pid_of(*fg) != self.self_pid
                        && window::rect_of(*fg)
                            .is_some_and(|r| cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom)
                        && window::check(*fg, self.self_pid).is_ok()
                });
            }
            let Some(hwnd) = found else {
                if first && !ours_under {
                    if let Some(mv) = moving {
                        let why = refusal.unwrap_or_else(|| {
                            format!(
                                "no rectangle for {} class={}",
                                window::describe(window::raw_of(mv)),
                                window::class_name(mv)
                            )
                        });
                        crate::log!("drag: the move loop is dragging a window that cannot be hung ({why})");
                    } else if let Some(why) = refusal {
                        crate::log!("drag: nothing hangable under the pointer ({why})");
                    }
                }
                return;
            };
            let raw = window::raw_of(hwnd);
            // The rectangle DWM draws, not GetWindowRect's: the invisible resize
            // border hangs 7-8 px above what the user sees, and measuring the
            // hang band with it would call a window whose visible edge is clear
            // of the top flush with it. Everything below compares against this
            // same rectangle, including the movement test.
            let Some(started) = window::visible_rect(hwnd) else { return };
            crate::log!(
                "drag {raw:#x}: following {} class={}{}",
                window::describe(raw),
                window::class_name(hwnd),
                if moving.is_some() { " (move loop)" } else { "" }
            );
            self.drag = Some(DragWatch { hwnd: raw, started, moved: false, told_tucked: false });
            return;
        }

        // The move loop can start a poll or two after the press, and it can name
        // a different window than the point did: a helper under the pointer is
        // not what Windows is moving. When it speaks, it wins. The watched
        // window's own thread answers first, so the gesture keeps working even
        // if the window is not the foreground one.
        let self_pid = self.self_pid;
        let watched = self.drag.as_ref().map(|w| w.hwnd).unwrap_or(0);
        let moving = window::moving_on(window::thread_of(window::hwnd_of(watched)))
            .or_else(|| moving_window(cx, cy))
            .filter(|mv| window::pid_of(*mv) != self_pid && window::check(*mv, self_pid).is_ok());
        let mut gone = false;
        if let Some(watch) = self.drag.as_mut() {
            if let Some(mv) = moving {
                let raw = window::raw_of(mv);
                if raw != watch.hwnd {
                    crate::log!("drag {:#x}: the move loop names {raw:#x}; following that", watch.hwnd);
                    watch.hwnd = raw;
                    watch.started = match window::rect_of(mv) {
                        Some(r) => r,
                        None => watch.started,
                    };
                }
            }
            gone = !window::is_alive(watch.hwnd);
        }
        if gone {
            self.drag = None;
            self.set_band_hot(false, None);
            return;
        }

        // A drag needs two things, and neither alone is enough. The pointer must
        // have left the press point, or a click on a title bar (which briefly
        // passes through the move loop) would read as a drag. And Windows must
        // actually be moving the window — by the move loop when it answers, by
        // the rectangle otherwise — or dragging across a page's text would hang
        // a window nobody touched.
        let travelled =
            self.press_at.is_some_and(|(px, py)| (cx - px).abs() > 8 || (cy - py).abs() > 8);
        let mut moved = false;
        let mut rect = None;
        if let Some(watch) = self.drag.as_mut() {
            rect = window::visible_rect(window::hwnd_of(watch.hwnd));
            let rect_moved = rect.is_some_and(|rect| {
                (rect.left - watch.started.left).abs() > 8 || (rect.top - watch.started.top).abs() > 8
            });
            if travelled && (moving.is_some() || rect_moved) && !watch.moved {
                crate::log!("drag {:#x}: the pointer has moved; the line is armed", watch.hwnd);
                watch.moved = true;
            }
            moved = watch.moved;
        }
        if !moved {
            self.set_band_hot(false, None);
            return;
        }
        // A window can be dragged from one monitor to another. The line goes
        // where the pointer is, so the band, the layout and the DPI all belong
        // to the screen the drop would happen on.
        let at = monitor::at_point(cx, cy);
        if at.handle != self.monitor.handle {
            self.place(at);
        }
        let screen_top = self.monitor.bounds.top;
        // The highlight is recomputed from this poll's rectangle rather than
        // latched, in both directions: a window that is flush because the system
        // is restoring it from maximized has to become hangable the moment it
        // comes down, and one pushed up there mid-gesture has to go cold right
        // away. A release the 50 ms poll happens to miss must not leave "let go"
        // on screen either.
        //
        // Only the window's own top edge decides this, never the pointer: a
        // window can be gripped well below its top, and gating on the pointer as
        // well made the light flicker off while the window sat on the rope.
        let on_line = rect.is_some_and(|rect| self.layout.window_on_line(rect.top, screen_top));
        // A line the user put away is inert: it must not light up, come back
        // or accept a drop just because a window is dragged over the place it
        // used to occupy. Hanging a window while the line is hidden would also
        // hide the thing the window went to, which looks like the window was
        // closed. Hover, the pill and the hotkey are the ways back. A line that
        // merely tucked itself away is still available: dragging a window up
        // brings it back, which is the documented gesture.
        if on_line && self.dismissed {
            if let Some(watch) = self.drag.as_mut() {
                if !watch.told_tucked {
                    watch.told_tucked = true;
                    crate::log!(
                        "drag {:#x}: the window is on the line, but the line was put away; it stays out of the drag",
                        watch.hwnd
                    );
                }
            }
        }
        let near = on_line && !self.dismissed;
        let numbers = rect.map(|rect| (rect.top, screen_top, self.layout.hang_depth()));
        self.set_band_hot(near, numbers);
        if near {
            self.revealed = true;
        }
    }

    /// Light the rope for a dragged window, or take the light away. `false` means
    /// the drag is nowhere near enough to be about to hang, and the line goes
    /// back to being a picture. The numbers say why: the window's own top edge,
    /// the top of the screen and how deep the band reaches, so a report of "it
    /// would not light" can be measured, not guessed.
    fn set_band_hot(&mut self, hot: bool, where_: Option<(i32, i32, i32)>) {
        if self.band_hot == hot {
            return;
        }
        self.band_hot = hot;
        let detail = where_.map_or_else(String::new, |(top, edge, depth)| {
            format!(" (window top={top} screen edge={edge} band={depth})")
        });
        crate::log!(
            "drag band: {}",
            if hot { "the window is on the line, letting go will hang it" } else { "off the line" }
                .to_string()
                + &detail
        );
        // The frame timer stops while the line is still, so nothing would repaint
        // the light away on its own.
        self.invalidate();
    }

    fn poll_windows(&mut self, now: Instant) {
        // "Show Desktop" (Win+D) minimizes every window at once. A single
        // minimized hooked window is the user asking to see it; a burst is the
        // user asking for the desktop, so the line must stay out of the way.
        // A card we hung up while it was already minimized is minimized every
        // second and must not count as part of the burst.
        let iconic = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving)
            .filter(|c| c.away || !park::parked_minimized(&c.parked))
            .filter(|c| window::is_alive(c.hwnd) && window::minimized(window::hwnd_of(c.hwnd)))
            .count();
        let on_line: Vec<(u64, isize, u32, Parked)> = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving && !c.away)
            .map(|c| (c.id, c.hwnd, c.pid, c.parked))
            .collect();
        let away: Vec<(u64, isize, u32, Option<Instant>)> = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving && c.away)
            .map(|c| (c.id, c.hwnd, c.pid, c.away_at))
            .collect();

        let mut changed = false;
        for (id, hwnd, pid, parked) in on_line {
            if !window::same_window(hwnd, pid) {
                crate::log!("card {id}: window {hwnd} closed");
                self.painter.forget(id);
                self.line.remove(id);
                changed = true;
                continue;
            }
            let h = window::hwnd_of(hwnd);
            if window::minimized(h) {
                if park::parked_minimized(&parked) {
                    // We hung it up while it was already minimized, so this is
                    // the state we left it in. Restoring it here is what made
                    // the window flash back onto the desktop.
                    continue;
                }
                if iconic <= 1 {
                    // Clicking the taskbar button of the window that was hung
                    // while it still had focus minimizes it. Show it again
                    // rather than silently dropping the card.
                    crate::log!("card {id}: window {hwnd} was minimized; taking it down");
                    self.take_down(id);
                }
                continue;
            }
            if !park::still_hung(h, &parked) {
                if window::on_screen(h) {
                    crate::log!("card {id}: window {hwnd} was put back by hand; keeping the hook");
                    self.line.detach(id, now);
                    changed = true;
                } else {
                    crate::log!("card {id}: window {hwnd} moved but is still off screen; restoring it");
                    self.take_down(id);
                }
            }
        }
        for (id, hwnd, pid, away_at) in away {
            if !window::same_window(hwnd, pid) {
                crate::log!("card {id}: hooked window {hwnd} closed");
                self.painter.forget(id);
                self.line.remove(id);
                changed = true;
                continue;
            }
            let h = window::hwnd_of(hwnd);
            let settled = away_at.is_none_or(|t| now.duration_since(t) >= REHANG_GRACE);
            if window::minimized(h) && iconic <= 1 && settled {
                self.rehang(id, None);
            }
        }
        if changed {
            self.sync_journal();
            self.invalidate();
        }
    }

    /// The card of a window that is on the line right now, if it is still ours.
    fn parked_card(&self, hwnd: isize) -> Option<u64> {
        self.line.cards.iter().find(|c| c.hwnd == hwnd && !c.away && !c.leaving).map(|c| c.id)
    }

    /// The focus fell on a parked window without being asked for. Give it to a
    /// window the user can see and leave the card where it is.
    ///
    /// Showing the window instead is what made minimizing one window open a
    /// random card on the desktop: the parked window is only visible to the
    /// shell, and the user never asked to see it.
    fn keep_on_line(&mut self, id: u64, hwnd: HWND, now: Instant) {
        crate::log!("card {id}: the foreground fell to a parked window; leaving it on the line");
        self.hand_focus_off(hwnd, now);
    }

    /// Hands the focus to something the user can see because `hwnd` is off
    /// screen but still in front. Returns whether it worked; the window is
    /// never shown to get the focus back, and a failure is retried for a while
    /// by the poll.
    fn hand_focus_off(&mut self, hwnd: HWND, now: Instant) -> bool {
        let raw = window::raw_of(hwnd);
        if window::foreground() != Some(hwnd) {
            return false;
        }
        if window::hand_off_foreground(hwnd, self.self_pid) || window::hand_to_shell() {
            self.last_foreground = window::foreground().map(window::raw_of).unwrap_or(0);
            self.stuck_foreground = None;
            return true;
        }
        match self.stuck_foreground {
            Some(stuck) if stuck.hwnd == raw => {
                self.stuck_foreground = Some(StuckFocus { last_try: now, ..stuck });
            }
            _ => {
                crate::log!("foreground: hwnd={raw} is off screen but kept the focus; nothing else would take it");
                self.stuck_foreground = Some(StuckFocus { hwnd: raw, since: now, last_try: now });
            }
        }
        false
    }

    /// The user minimized a window they had taken down. Park it again and
    /// animate its card back onto the line. The old thumbnail is kept: the
    /// window has not been drawn on screen long enough for a fresh, reliable
    /// snapshot, and a black one would be worse.
    /// Puts a hooked window back on the line, keeping its record.
    ///
    /// `source` is the request that asked for it, when a request did: hanging a
    /// window again is a fresh decision about where it opens, so a window
    /// dragged onto the line now comes back centred even if it was hooked by
    /// hotkey before. `None` is the automatic re-hang when the user minimizes
    /// the window, which is not a new choice and keeps the old one.
    fn rehang(&mut self, id: u64, source: Option<HangSource>) {
        let Some(card) = self.line.get(id) else { return };
        if !card.away {
            return;
        }
        let hwnd = card.hwnd;
        let pid = card.pid;
        let start = card.process_start;
        let h = window::hwnd_of(hwnd);
        if !window::still_same_window(hwnd, pid, start) {
            self.painter.forget(id);
            self.line.remove(id);
            self.sync_journal();
            self.invalidate();
            return;
        }
        let (placement, parked, before) = if window::minimized(h) {
            // The window is already out of sight. Moving it would first mean
            // bringing it back, and the user would watch it flash onto the
            // desktop before it vanished again. So it hangs up where it stands
            // and only the card moves.
            let Some(placement) = park::Placement::capture(h) else {
                return;
            };
            (placement, park::hung_up_in_place(placement), placement.normal)
        } else {
            let previous = window::foreground();
            let before = window::rect_of(h).unwrap_or(card.window_rect);
            let result = park::plan(h).and_then(|plan| park::execute(h, &plan).map(|parked| (plan, parked)));
            match result {
                Ok((plan, parked)) => {
                    // Do not leave an invisible window holding the keyboard.
                    // The window just parked is usually the one that had the
                    // focus (the user dragged it to the line), so give the
                    // focus to something the user can still see.
                    match previous {
                        Some(prev)
                            if window::raw_of(prev) != hwnd
                                && window::raw_of(prev) != self.hwnd.0 as isize
                                && window::is_alive(window::raw_of(prev)) =>
                        {
                            window::force_foreground(prev);
                            self.last_foreground = window::foreground().map(window::raw_of).unwrap_or(0);
                        }
                        _ => {
                            self.hand_focus_off(h, Instant::now());
                        }
                    }
                    (plan.placement, parked, before)
                }
                Err(error) => {
                    crate::log!("rehang {id}: hwnd={hwnd} failed: {error:?}");
                    return;
                }
            }
        };
        self.recent_park = Some((hwnd, Instant::now()));
        if let Some(card) = self.line.get_mut(id) {
            card.placement = placement;
            card.parked = parked;
            card.window_rect = before;
            if let Some(source) = source {
                card.centred = source.centred();
            }
        }
        if self.line.rehang(id, &self.layout) {
            if park::parked_minimized(&parked) {
                crate::log!("rehang {id}: hwnd={hwnd} was already minimized; hung it up where it stands");
            } else {
                crate::log!("rehang {id}: hwnd={hwnd} parked at {:?}", parked.parked);
            }
            // No reveal here: a line the user tucked away stays away. The card
            // is on the line the next time it comes down, and minimizing a
            // window is not a request to see the card that just went up.
            self.sync_journal();
            self.invalidate();
        }
    }

    fn refresh_titles(&mut self) {
        let updates: Vec<(u64, String)> = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving)
            .filter_map(|c| {
                let title = window::title(window::hwnd_of(c.hwnd));
                (!title.is_empty() && title != c.title).then_some((c.id, title))
            })
            .collect();
        if updates.is_empty() {
            return;
        }
        let mut changed = Vec::new();
        for (id, title) in updates {
            if let Some(card) = self.line.get_mut(id) {
                card.title = title;
                card.thumb_version += 1;
                changed.push(id);
            }
        }
        for id in changed {
            self.painter.forget(id);
        }
        self.sync_journal();
        self.invalidate();
    }

    // MARK: Showing, hiding, animating

    fn toggle_reveal(&mut self) {
        if self.revealed {
            self.revealed = false;
            self.pinned = false;
            // Put away on purpose. Unlike an automatic tuck, this line must not
            // answer a drag any more: a window over the old spot neither lights
            // it nor hangs.
            self.dismissed = true;
            self.hold_until = None;
            self.away_since = None;
            self.empty_since = None;
            self.hot_since = None;
            self.hover_ready = false;
            self.peek = false;
            self.peek_hover = false;
            self.band_hot = false;
            self.hover = None;
        } else {
            // Asked for on purpose: keep it down until it is dismissed on
            // purpose, even when there is nothing hanging on it yet.
            self.revealed = true;
            self.pinned = true;
            self.dismissed = false;
            self.hold_until = None;
            self.empty_since = None;
            self.peek = false;
            self.peek_hover = false;
            let (x, y) = monitor::cursor();
            let at = monitor::at_point(x, y);
            if at.handle != self.monitor.handle {
                self.place(at);
            }
        }
        crate::log!("line toggled: revealed={} pinned={}", self.revealed, self.pinned);
        self.invalidate();
    }

    // MARK: Settings

    fn open_settings(&mut self) {
        if !self.settings_wnd.is_invalid() && unsafe { IsWindow(Some(self.settings_wnd)).as_bool() } {
            unsafe {
                let _ = ShowWindow(self.settings_wnd, SW_RESTORE);
                let _ = SetForegroundWindow(self.settings_wnd);
            }
            return;
        }
        let app = self as *mut App;
        self.settings_wnd = settings_ui::open(self.window_icon, self.tray_icon, &self.settings, self.monitor, app)
            .unwrap_or_default();
    }

    pub(crate) fn settings_window_closed(&mut self) {
        self.settings_wnd = HWND::default();
    }

    /// Live preview of the curve while the settings window drags its slider,
    /// and again when a preview is cancelled: moves the line and shows it, but
    /// writes nothing to the settings file. `SAG_AUTO` here restores the curve
    /// the program ships with.
    pub(crate) fn preview_sag(&mut self, sag_percent: f32) {
        self.settings.sag_percent = sag_percent;
        let monitor = self.monitor;
        self.place(monitor);
        self.revealed = true;
        self.dismissed = false;
        self.hold_until = Some(Instant::now() + Duration::from_millis(3500));
        self.invalidate();
    }

    /// Called by the settings window once the user presses Save. The hotkeys
    /// are re-registered first: if one is taken, nothing else changes and the
    /// window stays open with the old shortcut still working.
    pub(crate) fn apply_settings(
        &mut self,
        hang: (u32, u32),
        toggle: (u32, u32),
        sag_percent: f32,
        hover_open: bool,
        always_on: bool,
        drag_hang: bool,
    ) -> Result<(), String> {
        self.apply_hotkeys(hang.0, hang.1, toggle.0, toggle.1)?;
        self.settings.hang_mods = hang.0;
        self.settings.hang_vk = hang.1;
        self.settings.toggle_mods = toggle.0;
        self.settings.toggle_vk = toggle.1;
        self.settings.sag_percent =
            if sag_percent < 0.0 { SAG_AUTO } else { sag_percent.clamp(0.0, SAG_PERCENT_MAX) };
        self.settings.hover_open = hover_open;
        self.settings.always_on = always_on;
        self.settings.drag_hang = drag_hang;
        self.settings.save();
        crate::log!(
            "settings saved: hang={} toggle={} sag={} hover_open={} always_on={} drag_hang={}",
            self.settings.hang_label(),
            self.settings.toggle_label(),
            if self.settings.sag_is_auto() {
                "auto".to_string()
            } else {
                format!("{:.1}%", self.settings.sag_percent)
            },
            hover_open,
            always_on,
            drag_hang
        );
        // A different sag changes the strip height and where the cards sit.
        let monitor = self.monitor;
        self.place(monitor);
        self.invalidate();
        Ok(())
    }

    fn apply_hotkeys(
        &mut self,
        hang_mods: u32,
        hang_vk: u32,
        toggle_mods: u32,
        toggle_vk: u32,
    ) -> Result<(), String> {
        let old = (
            self.settings.hang_mods,
            self.settings.hang_vk,
            self.settings.toggle_mods,
            self.settings.toggle_vk,
        );
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_HANG);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_TOGGLE);
        }
        let mut failed = Vec::new();
        unsafe {
            if hang_vk != 0 {
                let mods = HOT_KEY_MODIFIERS(hang_mods) | MOD_NOREPEAT;
                if RegisterHotKey(Some(self.hwnd), HOTKEY_HANG, mods, hang_vk).is_err() {
                    failed.push(settings::hotkey_label(hang_mods, hang_vk));
                }
            }
            if toggle_vk != 0 {
                let mods = HOT_KEY_MODIFIERS(toggle_mods) | MOD_NOREPEAT;
                if RegisterHotKey(Some(self.hwnd), HOTKEY_TOGGLE, mods, toggle_vk).is_err() {
                    failed.push(settings::hotkey_label(toggle_mods, toggle_vk));
                }
            }
        }
        if !failed.is_empty() {
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_HANG);
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_TOGGLE);
            }
            unsafe {
                if old.1 != 0 {
                    let mods = HOT_KEY_MODIFIERS(old.0) | MOD_NOREPEAT;
                    let _ = RegisterHotKey(Some(self.hwnd), HOTKEY_HANG, mods, old.1);
                }
                if old.3 != 0 {
                    let mods = HOT_KEY_MODIFIERS(old.2) | MOD_NOREPEAT;
                    let _ = RegisterHotKey(Some(self.hwnd), HOTKEY_TOGGLE, mods, old.3);
                }
            }
            return Err(format!("{}: {}", t("settings_hotkey_failed"), failed.join(", ")));
        }
        crate::log!(
            "hotkeys now {} / {}",
            settings::hotkey_label(hang_mods, hang_vk),
            settings::hotkey_label(toggle_mods, toggle_vk)
        );
        Ok(())
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        if !self.anim_running {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_FRAME, self.frame_ms, None);
            }
            self.anim_running = true;
            self.last_frame = Instant::now();
        }
    }

    fn animate(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;

        let finished = self.line.step(dt, now, &self.layout, self.hover);
        for id in finished {
            self.finish_take_down(id);
        }

        let target = if self.revealed { 1.0 } else { 0.0 };
        if self.settings.reduced_motion {
            self.slide = target;
        } else {
            let rate = if target > self.slide { 16.0 } else { 20.0 };
            self.slide += (target - self.slide) * (1.0 - (-dt * rate).exp());
            if (target - self.slide).abs() < 0.002 {
                self.slide = target;
            }
        }

        self.ensure_panel();
        // The frame timer should only run while something is actually moving.
        // A line that is simply down is static: it is redrawn on demand (hover,
        // new thumbnails, polls) by `invalidate`, not sixty times a second.
        let animating = self.line.animating() || (self.slide - target).abs() > 0.0005;
        let visible = self.slide > 0.001 || self.peek;
        if visible {
            // The timer only runs while something is moving or a redraw was
            // asked for, so every tick paints. In particular the tick that
            // lands a flight has to paint the resting card: the frame before it
            // still showed the ghost, and skipping that tick left the ghost on
            // screen until some unrelated event asked again.
            let started = Instant::now();
            self.redraw();
            self.pace_frame(started.elapsed().as_secs_f32());
            self.set_shown(true);
        } else {
            self.set_shown(false);
        }
        self.dirty = false;

        if !animating {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
            }
            self.anim_running = false;
        }
    }

    /// Slows the frame timer when a frame costs more than its slot.
    ///
    /// Ten milliseconds of work cannot keep a sixteen-millisecond rhythm once
    /// the timer and Windows' own delivery are counted, and a screen-wide
    /// layered window uploaded faster than the machine can blend it holds up
    /// every other window on the desktop. Half the frame rate costs a little
    /// smoothness and gives the desktop back.
    fn pace_frame(&mut self, cost: f32) {
        self.frame_cost = if self.frame_cost <= 0.0 {
            cost
        } else {
            self.frame_cost * 0.8 + cost * 0.2
        };
        let want = if self.frame_cost > 0.018 {
            FRAME_MS_SLOW
        } else if self.frame_cost > 0.009 {
            FRAME_MS_HALF
        } else {
            FRAME_MS
        };
        if want == self.frame_ms {
            return;
        }
        self.frame_ms = want;
        if self.anim_running {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_FRAME, want, None);
            }
        }
    }

    fn set_shown(&mut self, on: bool) {
        if on == self.shown {
            return;
        }
        self.shown = on;
        unsafe {
            if on {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    fn set_click_through(&mut self, on: bool) {
        if self.click_through == on {
            return;
        }
        self.click_through = on;
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let bit = WS_EX_TRANSPARENT.0 as isize;
            let next = if on { ex | bit } else { ex & !bit };
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        self.invalidate();
    }

    fn place(&mut self, monitor: Monitor) {
        self.monitor = monitor;
        self.layout = self.settings.layout_for(self.monitor.bounds.width() as f32, self.monitor.scale);
        self.panel = self.strip();
        self.origin = (self.panel.left, self.panel.top);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                self.panel.left,
                self.panel.top,
                self.panel.width(),
                self.panel.height(),
                SWP_NOACTIVATE,
            );
        }
        self.invalidate();
    }

    fn strip(&self) -> WRect {
        WRect {
            left: self.monitor.bounds.left,
            top: self.monitor.bounds.top,
            right: self.monitor.bounds.right,
            bottom: self.monitor.bounds.top + self.layout.panel_height().ceil() as i32,
        }
    }

    fn desired_panel(&self) -> WRect {
        let mut rect = self.strip();
        let s = self.layout.scale;
        let fit = self.line.fit(&self.layout);
        for card in &self.line.cards {
            if !card.away && !card.leaving {
                // A card the user has made taller hangs below the strip. The
                // panel only draws what fits in it, so it has to grow, or the
                // bottom of the card would be cut off.
                let k = card.scale * fit;
                let bottom = self.layout.rope_y(card.x.x)
                    + ((PIN_TOP - 22.0) + CARD_H * card.zoom_y) * s * k;
                rect.bottom = rect.bottom.max(self.monitor.bounds.top + bottom.ceil() as i32 + 10);
            }
        }
        // Never leave the monitor: a layered window straddling screens is both
        // ugly and slow.
        let b = self.monitor.bounds;
        WRect {
            left: rect.left.clamp(b.left, b.right - 1),
            top: rect.top.clamp(b.top, b.bottom - 1),
            right: rect.right.clamp(b.left + 1, b.right),
            bottom: rect.bottom.clamp(b.top + 1, b.bottom),
        }
    }

    fn ensure_panel(&mut self) {
        let want = self.desired_panel();
        if want == self.panel {
            return;
        }
        self.panel = want;
        self.origin = (want.left, want.top);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                want.left,
                want.top,
                want.width(),
                want.height(),
                SWP_NOACTIVATE,
            );
        }
    }

    fn redraw(&mut self) {
        if !self.compose() {
            return;
        }
        if self.dump_request {
            self.dump_request = false;
            if let Some(dir) = &self.dump_dir {
                let _ = std::fs::create_dir_all(dir);
                self.dump_seq += 1;
                let path = dir.join(format!("frame-{}-{:02}.png", std::process::id(), self.dump_seq));
                if let Some(canvas) = self.canvas.as_ref() {
                    let _ = canvas.save_png(&path);
                }
                self.dump_hits(dir);
            }
        }
        self.present();
    }

    /// Debug aid for the regression tests: the clickable rectangles of the cards
    /// just drawn, in panel-local pixels, one `id<TAB>hwnd<TAB>l,t,r,b` per line.
    /// Without it a test cannot say which card is where.
    fn dump_hits(&self, dir: &std::path::Path) {
        let mut text = String::new();
        for (id, rect) in &self.hits {
            let hwnd = self.line.get(*id).map(|card| card.hwnd).unwrap_or(0);
            text.push_str(&format!(
                "{}\t{}\t{},{},{},{}\n",
                id, hwnd, rect.left, rect.top, rect.right, rect.bottom
            ));
        }
        let path = dir.join(format!("hits-{}.txt", std::process::id()));
        let _ = std::fs::write(&path, text);
    }

    /// Composes the frame into `self.canvas`. Returns false when the canvas
    /// cannot be sized, which is a state that cannot last.
    fn compose(&mut self) -> bool {
        let w = self.panel.width().max(1) as u32;
        let h = self.panel.height().max(1) as u32;
        if self.canvas.as_ref().is_none_or(|c| c.width() != w || c.height() != h) {
            let Some(canvas) = Pixmap::new(w, h) else { return false };
            self.canvas = Some(canvas);
        }
        let Some(mut canvas) = self.canvas.take() else { return false };
        let frame = if self.peek && self.slide <= 0.001 {
            let hot = self.peek_hover;
            self.painter.compose_peek_into(&mut canvas, &self.layout, hot)
        } else {
            let hot = self.band_hot && self.drag.is_some();
            let hint: Option<String> = if self.line.live_count() == 0 {
                Some(crate::i18n::empty_hint(&self.settings.hang_label()))
            } else {
                None
            };
            let ui = Ui {
                hover: self.hover,
                y_off: (self.slide - 1.0) * self.layout.panel_height(),
                hint: hint.as_deref(),
                hot,
            };
            self.painter
                .compose_into(&mut canvas, &self.line, &self.layout, &ui)
        };
        self.canvas = Some(canvas);
        self.hits = frame.hits;
        true
    }

    fn present(&mut self) {
        let Some(canvas) = self.canvas.as_ref() else { return };
        let (w, h) = (canvas.width() as i32, canvas.height() as i32);
        // Grow the DIB only when the frame no longer fits in it. A flight makes
        // the panel grow and shrink with every frame; allocating the GDI bitmap
        // each time is a cost the frame rate pays for no visible reason.
        let reuse = self.dib.as_ref().is_some_and(|d| {
            let area = (d.w * d.h) as i64;
            let want = (w * h) as i64;
            d.w >= w && d.h >= h && area <= want * 5 / 2
        });
        if !reuse {
            self.dib = Dib::new(w, h);
        }
        let Some(dib) = self.dib.as_ref() else { return };
        let stride = (dib.w * 4) as usize;
        let row = (w * 4) as usize;
        unsafe {
            let dst = std::slice::from_raw_parts_mut(dib.bits, stride * dib.h as usize);
            // tiny-skia speaks RGBA, the layered window wants BGRA. One 32-bit
            // swap per pixel, a row at a time, because the DIB may be wider
            // than this frame.
            for (y, src) in canvas.data().chunks_exact(row).enumerate() {
                let out = &mut dst[y * stride..y * stride + row];
                for (to, from) in out.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    let v = u32::from_ne_bytes([from[0], from[1], from[2], from[3]]);
                    let swapped = (v & 0xFF00_FF00) | ((v & 0xFF) << 16) | ((v >> 16) & 0xFF);
                    to.copy_from_slice(&swapped.to_ne_bytes());
                }
            }
        }
        let origin = POINT { x: self.origin.0, y: self.origin.1 };
        let size = SIZE { cx: w, cy: h };
        let source = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            if let Err(error) = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&origin),
                Some(&size),
                Some(dib.hdc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            ) {
                if !self.present_failed {
                    self.present_failed = true;
                    crate::log!("UpdateLayeredWindow failed (nothing will show): {error}");
                }
            }
        }
    }

    // MARK: Small helpers

    fn card_at(&self, point: (i32, i32)) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(_, rect)| {
                point.0 >= rect.left - 4
                    && point.0 < rect.right + 4
                    && point.1 >= rect.top - 4
                    && point.1 < rect.bottom + 4
            })
            .map(|(id, _)| *id)
    }

    /// The card's clickable rectangle in canvas coordinates, as of the last
    /// frame. Its sides and bottom corners are the resize handles.
    fn card_rect(&self, id: u64) -> Option<WRect> {
        self.hits.iter().rev().find(|(hit, _)| *hit == id).map(|(_, rect)| *rect)
    }

    fn peek_at(&self, point: (i32, i32)) -> bool {
        self.peek
            && self.hits.iter().rev().any(|(id, rect)| {
                *id == PEEK_ID
                    && point.0 >= rect.left - 4
                    && point.0 < rect.right + 4
                    && point.1 >= rect.top - 4
                    && point.1 < rect.bottom + 4
            })
    }

    /// A press on a card's face becomes a drag after a few pixels. Until then it
    /// is still a click, which takes the window down. The line reorders as the
    /// card passes its neighbours, and the drop position is kept as an offset,
    /// so a card does not spring back to its old spot.
    fn drag_card_to(&mut self, point: (i32, i32)) {
        let Some(press) = self.card_press else { return };
        if let Some(edge) = press.edge {
            let mut changed = false;
            if edge.resizes_height() {
                let zoom_y = crate::line::zoom_y_from_bottom(point.1 as f32, press.body_top, press.height_unit);
                // The top stays on the pin, so the card only moves its bottom;
                // the line does not need re-packing.
                changed |= self.line.set_zoom_y(press.id, zoom_y);
            }
            if edge.resizes_width() {
                let zoom = crate::line::zoom_from_edge(point.0 as f32, press.centre, self.layout.scale);
                if self.line.set_zoom(press.id, zoom) {
                    // A wider card re-packs the line, which would carry it away
                    // from the edge the pointer is holding. Keep its hanging
                    // point put.
                    self.line.place_at(press.id, &self.layout, press.centre);
                    changed = true;
                }
            }
            if changed {
                if let Some(p) = self.card_press.as_mut() {
                    p.grabbed = true;
                    p.zoomed = true;
                }
                self.revealed = true;
                self.hold_until = Some(Instant::now() + Duration::from_secs(1));
                self.invalidate();
            }
            return;
        }
        let dx = point.0 - press.start.0;
        let dy = point.1 - press.start.1;
        if !press.grabbed && dx * dx + dy * dy <= 25 {
            return;
        }
        if let Some(p) = self.card_press.as_mut() {
            p.grabbed = true;
        }
        let id = press.id;
        let x = (point.0 as f32).clamp(0.0, self.layout.width);
        self.line.dragging = Some(id);
        // Neighbours make room while the card is still in hand, so the line
        // answers the drag instead of waiting for the drop.
        let target = self.line.drop_index(&self.layout, id, x);
        if self.line.live_position(id) != Some(target) {
            self.line.reorder(id, target);
        }
        if let Some(card) = self.line.get_mut(id) {
            // The offset is re-decided on release; while dragging the pointer is
            // the only thing that says where the card is.
            card.offset = 0.0;
            // Under the pointer, not chasing it: a card that lags behind the
            // mouse feels like it is stuck to the rope.
            card.x.snap(x);
        }
        self.peek = false;
        self.peek_hover = false;
        self.revealed = true;
        self.hover = Some(id);
        self.hold_until = Some(Instant::now() + Duration::from_secs(2));
        self.invalidate();
    }

    fn finish_card_press(&mut self, point: (i32, i32)) {
        let Some(press) = self.card_press.take() else { return };
        unsafe {
            let _ = ReleaseCapture();
        }
        self.line.dragging = None;
        if press.edge.is_some() {
            if press.zoomed {
                let card = self.line.get(press.id);
                crate::log!(
                    "card {} resized to {:.2} x {:.2}",
                    press.id,
                    card.map_or(1.0, |c| c.zoom),
                    card.map_or(1.0, |c| c.zoom_y)
                );
                self.write_journal();
                self.hold_until = Some(Instant::now() + Duration::from_secs(1));
            }
            self.invalidate();
            return;
        }
        if press.grabbed {
            self.line.place_at(press.id, &self.layout, point.0 as f32);
            crate::log!("card {} dropped at x={}", press.id, point.0);
            // The journal keeps the line's order and offsets, so a restart puts
            // the cards back where the user left them.
            self.write_journal();
            self.hold_until = Some(Instant::now() + Duration::from_secs(1));
            self.invalidate();
        } else {
            self.take_down(press.id);
        }
    }

    fn cancel_card_press(&mut self) {
        if let Some(press) = self.card_press.take() {
            self.line.dragging = None;
            if press.edge.is_some() {
                // A resize that lost the capture never happened.
                self.line.set_zoom(press.id, press.zoom0);
                self.line.set_zoom_y(press.id, press.zoom_y0);
            }
            self.invalidate();
        }
    }

    /// Writes the journal from the current line. Every card that is still hooked
    /// is in it, including the ones whose window is on the desktop: a hook the
    /// user did not cancel has to outlive a restart of ShaiWin. The order of the
    /// records is the order on the line, so a restart puts the cards back where
    /// the user put them.
    fn write_journal(&mut self) {
        let entries: Vec<journal::Entry> = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving)
            .map(journal_entry)
            .collect();
        journal::save(&entries);
    }

    fn sync_journal(&mut self) {
        self.write_journal();
        if !self.tray_icon.is_invalid() {
            let tip = crate::i18n::tray_tip(self.line.live_count());
            // The count is what the tip says, and it only changes on a hang or a
            // take-down. Sending the same text again makes the shell redraw the
            // notification area, which looks like the taskbar flickering.
            if tip != self.tray_tip {
                self.tray_tip = tip;
                tray::set_tip(self.hwnd, &self.tray_tip);
            }
        }
    }

    fn write_journal_append(&mut self, entry: journal::Entry) {
        let mut entries: Vec<journal::Entry> = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving)
            .map(journal_entry)
            .collect();
        entries.push(entry);
        journal::save(&entries);
    }

    fn complain(&mut self, message: String) {
        if !self.tray_icon.is_invalid() {
            tray::balloon(self.hwnd, self.tray_icon, t("cannot_hang"), &message, true);
        }
        tray::beep();
    }
}

/// The record a card's window comes back to.
///
/// A window that was dragged onto the line opens in the middle of the screen
/// the line is on; a window hung up by hotkey or from the tray goes back
/// exactly where it was. Both keep their size, and a maximized window is left
/// alone: it comes back full screen either way.
fn restore_record(card: &Card, work: WRect) -> Parked {
    if card.centred {
        park::centred_on(&card.parked, work)
    } else {
        card.parked
    }
}

fn journal_entry(card: &Card) -> journal::Entry {
    journal::Entry {
        hwnd: card.hwnd,
        pid: card.pid,
        process_start: card.process_start,
        title: card.title.clone(),
        exe: card.exe.clone(),
        placement: card.placement,
        parked: card.parked,
        anchor: card.anchor,
        hooked_away: card.away,
        zoom: card.zoom,
        zoom_y: card.zoom_y,
        offset: card.offset,
        centred: card.centred,
    }
}

fn rejection_message(rejection: Rejection) -> String {
    let key = match rejection {
        Rejection::Gone => "rej_gone",
        Rejection::Ours => "rej_ours",
        Rejection::Hidden => "rej_hidden",
        Rejection::NoTitle => "rej_notitle",
        Rejection::ToolWindow => "rej_tool",
        Rejection::Cloaked => "rej_cloaked",
        Rejection::Minimized => "rej_minimized",
        Rejection::Elevated => "rej_elevated",
        Rejection::TooSmall => "rej_small",
    };
    t(key).to_string()
}

fn app_name(exe: &str) -> String {
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Window".to_string(),
    }
}

fn to_pixmap(image: &shaiwin_platform_win::Image) -> Option<Pixmap> {
    if image.is_empty() {
        return None;
    }
    let size = IntSize::from_wh(image.width, image.height)?;
    Pixmap::from_vec(image.to_premultiplied_rgba(), size)
}

fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}


fn client_point(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

fn screen_point(lp: LPARAM) -> POINT {
    let (x, y) = client_point(lp);
    POINT { x, y }
}

/// The window the system is moving or sizing, looked up through the threads
/// that could be running the move loop.
///
/// The thread of the window under the pointer is the answer while the pointer
/// is over the window being dragged; the foreground thread covers the rest.
/// `GetGUIThreadInfo(0)` on its own is not enough: it follows the foreground
/// window, and that is not the dragged window for the whole gesture.
fn moving_window(cx: i32, cy: i32) -> Option<HWND> {
    let under = window::root_at(cx, cy);
    let foreground = window::foreground();
    under
        .and_then(|h| window::moving_on(window::thread_of(h)))
        .or_else(|| foreground.and_then(|h| window::moving_on(window::thread_of(h))))
        .or_else(|| window::moving_on(0))
}

fn taskbar_created() -> u32 {
    use std::sync::OnceLock;
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let (result, deferred) = (*ptr).on_message(msg, wp, lp);
        match deferred {
            Deferred::None => result,
            Deferred::Menu(kind) => {
                if let Some(id) = (*ptr).finish_menu(kind) {
                    (*ptr).command(id);
                }
                LRESULT(0)
            }
        }
    }
}
