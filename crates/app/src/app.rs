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
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, ValidateRect,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey,
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, VK_ESCAPE, VK_LBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::t;
use crate::journal;
use crate::line::{Card, Line, CARD_H, PIN_TOP};
use crate::render::{Painter, Ui};
use crate::settings::{self, Settings, SAG_AUTO};
use crate::worker::{Job, Reply, Worker};
use crate::{ico, settings_ui, tray};

const CLASS: PCWSTR = w!("ShaiWin.Line");
/// The rope's own layer. It outlives a card layer that has sunk behind the
/// user's work: the line must stay visible enough to reach for, even when the
/// cards are no longer on top. It is always click-through, so it never steals a
/// click from the window underneath.
const ROPE_CLASS: PCWSTR = w!("ShaiWin.Rope");
const TIMER_FRAME: usize = 1;
const TIMER_POLL: usize = 2;
/// Frame intervals, in milliseconds. The fast one is what smoothness wants; the
/// slower ones are what a frame that costs more than its slot can afford.
const FRAME_MS: u32 = 16;
const FRAME_MS_HALF: u32 = 33;
const FRAME_MS_SLOW: u32 = 50;
const HOTKEY_HANG: i32 = 1;
const HOTKEY_TOGGLE: i32 = 2;

const CMD_TAKE_ALL: u32 = 2;
const CMD_TOGGLE: u32 = 3;
const CMD_LOGIN: u32 = 4;
const CMD_QUIT: u32 = 5;
const CMD_TAKE_ONE: u32 = 6;
const CMD_SETTINGS: u32 = 7;
const CMD_UNHOOK: u32 = 8;
/// Opens the tag editor for the card under the menu.
const CMD_TAG_EDIT: u32 = 9;
/// Removes the card's tag without opening the editor.
const CMD_TAG_CLEAR: u32 = 10;

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
/// Automation hook: open the settings window, so a script can look at it.
const WM_OPEN_SETTINGS: u32 = WM_APP + 15;
/// Automation hook: open the tag editor for the card of the HWND in lParam.
const WM_OPEN_TAG: u32 = WM_APP + 16;
/// Automation hook: answer every pointer question from (`wParam`, `lParam`)
/// instead of the real cursor, until `wParam == u32::MAX` puts it back. The
/// z-order rule is entirely "where is the pointer", and a headless test
/// session is not allowed to move the real one.
const WM_POINT_AT: u32 = WM_APP + 17;
/// Automation hook: pretend a window of the user's is being dragged, the way
/// the system's move loop reports one, until sent again with `wParam == 0`.
/// A headless session cannot start a real move loop, and the rule under test -
/// a drag never brings the cards up - is the poll's, not the loop's.
const WM_PRETEND_DRAG: u32 = WM_APP + 18;

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

/// How long the pointer has to rest **on the pill** before it opens the line by
/// itself, the alternative being to click it. Short on purpose: a pointer that
/// has actually travelled onto the button is asking for the line, and making it
/// wait is the "why is nothing happening?" the button used to cause. This is
/// only the fallback for a pointer that arrived without visible movement; a
/// pointer that moves while on the pill opens it at once, see `poll_peek`.
const DWELL: Duration = Duration::from_millis(80);
/// The same while the line is empty. A little longer, so a pointer that grazes
/// the button on its way somewhere else does not pull out a rope with nothing on
/// it.
const DWELL_EMPTY: Duration = Duration::from_millis(220);
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
/// After the cards have sunk behind the user's window, the pointer has to rest
/// on the rope (not merely brush the top edge on its way to a title bar) for
/// this long before the cards come back. Short enough to feel immediate, long
/// enough that passing over the rope does not cover the window being used.
const RAISE_DWELL: Duration = Duration::from_millis(140);

/// Non-resident mode: the line is a short visit, not furniture. `auto_hide_secs`
/// in the settings is the idle clock; these are the events that can end an empty
/// visit sooner, and the grace after a take-down.
///
/// The slack after the last real action — a card let go, a window released on
/// the rope, the pointer leaving the line. Long enough to put the pointer back
/// on the rope and reach for the next window; short enough that a line nobody
/// is using is not furniture.
const VISIT_GRACE: Duration = Duration::from_millis(1_200);
/// A click on another window, or another window taking the foreground, says the
/// visit is over. Escape says it louder. The 0.28 s is the slide out, not a
/// decision period.
const VISIT_LEAVE: Duration = Duration::from_millis(280);
/// The line can be empty on purpose: the user pulled it out to have somewhere
/// to put a window, went to find that window, and is on their way back. That
/// trip - opening Alt+Tab, choosing, dragging - takes seconds, so an empty line
/// gets this much after the pointer leaves rather than the one-second visit.
/// Long enough to choose; short enough that a rope nobody came back to is gone
/// before it becomes furniture.
const EMPTY_LINE_LINGER: Duration = Duration::from_secs(4);
/// After a card goes back to the desktop the line stays down for a moment. The
/// restored window is about to take the foreground, and that is not the user
/// going elsewhere; dragging that window anywhere re-arms the busy clock.
const VISIT_AFTER_TAKE: Duration = Duration::from_millis(1_500);
/// The tag editor closes on the card, so the line stays a moment longer for the
/// new label to be read before the visit clock takes over.
const VISIT_AFTER_TAG: Duration = Duration::from_millis(1_800);
/// A launch shows the line whatever mode it is in — see the startup block.
const LAUNCH_LOOK: Duration = Duration::from_secs(4);

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
            let bitmap =
                CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(hdc, bitmap.into());
            Some(Dib {
                hdc,
                bitmap,
                old,
                bits: bits as *mut u8,
                w,
                h,
            })
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

/// The window that was just taken off the line, and where it stood at the last
/// look. Its rectangle moving while the left button is held is the user dragging
/// it away, which is the same gesture as the hang that came before it.
#[derive(Clone, Copy)]
struct TakeWatch {
    hwnd: isize,
    rect: WRect,
    until: Instant,
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
    /// menu), or a visit ran out. A dismissed line is inert: it must not light
    /// up, come back or accept a drop just because a window is dragged over the
    /// place it used to occupy.
    dismissed: bool,
    hold_until: Option<Instant>,
    /// In non-resident mode, when the line should tuck itself away. The pointer
    /// resting on the line and a card going back to the desktop both push it
    /// forward; nothing shortens it but a click outside an empty line.
    transient_until: Option<Instant>,
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
    /// Whether the card layer is asking Windows for the top of the screen. It
    /// does that while the line is being used and gives it back the moment a
    /// click lands somewhere else — see `set_topmost`.
    topmost: bool,
    /// The rope's own always-on-top, click-through layer. It is shown only while
    /// the card layer is not topmost, so the rope stays reachable after the
    /// cards have sunk behind the user's work.
    rope_hwnd: HWND,
    rope_canvas: Option<Pixmap>,
    rope_dib: Option<Dib>,
    rope_shown: bool,
    /// Cursor position at the previous poll, so "the pointer moved onto the
    /// pill" can open the line immediately instead of waiting out the dwell.
    last_cursor: (i32, i32),
    /// Automation hook: a stand-in for the pointer. A session with no input
    /// desktop refuses to move the real one, and the whole z-order rule is a
    /// question about where the pointer is, so the test sets this instead.
    hover_point: Option<(i32, i32)>,
    /// Automation hook: stand in for a drag the way `hover_point` stands in
    /// for the pointer. Inert unless a test posts `WM_PRETEND_DRAG`.
    pretend_drag: bool,

    drag: Option<DragWatch>,
    /// Where the pointer was when this press was first seen. A press that has
    /// not travelled is a click, not a drag, whatever the window happens to be
    /// doing under it.
    press_at: Option<(i32, i32)>,
    /// Whether the currently dragged window's own top edge is in the hang band.
    /// It is what lights the rope; the release asks the same question again.
    band_hot: bool,
    buttons_were_down: bool,
    /// The left button as of the last poll. Its falling edge is how the app
    /// notices a click outside the line without taking input away from the
    /// window that was clicked.
    left_down: bool,
    /// Whether the press now in progress moved a window or a card. A press that
    /// turned into a drag is the user bringing something to the line; a press
    /// that came back where it went in is a click, and a click that lands off
    /// the line is the user going elsewhere.
    press_dragged: bool,
    /// The window last taken down and where it stood. While the left button is
    /// held and that rectangle keeps changing, the user is dragging the window
    /// they just freed away from the line: the gesture is still going on, and no
    /// clock gets to decide when the line leaves.
    take_watch: Option<TakeWatch>,
    /// Escape as of the last poll. Its falling edge ends a visit at once, the
    /// way a click on another window does.
    esc_was_down: bool,
    /// False after an explicit "hide line" until the pointer leaves the top
    /// edge, so hiding the line while the cursor rests there sticks.
    hover_ready: bool,
    /// The small "show the line" pill a non-resident line offers before the
    /// user opens it. Hovering the edge never opens the line by itself.
    peek: bool,
    peek_hover: bool,
    peek_since: Option<Instant>,
    /// When the pointer came onto the pill. Resting on it for the dwell opens the
    /// line without the click; the pointer merely reaching the top edge does not.
    peek_hot: Option<Instant>,
    /// When the pointer first rested on the rope after the cards were demoted.
    /// The cards come back after `RAISE_DWELL`, so brushing the rope does not
    /// cover the window being worked in.
    raise_since: Option<Instant>,
    /// Whether the pointer was on the line at the previous poll, so leaving it
    /// is an edge the visit rules can see once.
    line_hot: bool,
    /// Whether the line was empty at the previous poll, so the moment it runs
    /// out of cards is an edge the visit rules can see once.
    line_was_empty: bool,
    /// A Win32 popup menu of ours is up. A menu is a normal window and cannot
    /// be painted over a topmost layer, so the cards stay down until it closes;
    /// and a menu is the user using the line, so the visit clock is paused.
    menu_open: bool,
    /// The card whose tag editor is open. While it is open the card layer keeps
    /// the top and the visit clock is paused, so the live preview stays visible.
    tag_card: Option<u64>,
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
    /// A card that is hung but has not been painted yet, and when its window was
    /// moved off screen. The gap is the part of a hang the user actually waits
    /// for: everything before it happens while the window is still on screen.
    hang_undrawn: Option<(isize, Instant)>,

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
        let rope = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(rope_wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big,
            hIconSm: small,
            lpszClassName: ROPE_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&rope);
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
            crate::tag_ui::register_class(hinst, window_icon, tray_icon);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
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
            let rope_hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                ROPE_CLASS,
                w!("ShaiWinRope"),
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
            .unwrap_or_default();

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
                transient_until: None,
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
                topmost: true,
                rope_hwnd,
                rope_canvas: None,
                rope_dib: None,
                rope_shown: false,
                last_cursor: monitor::cursor(),
                hover_point: None,
                pretend_drag: false,
                drag: None,
                press_at: None,
                band_hot: false,
                buttons_were_down: false,
                left_down: false,
                hover_ready: true,
                peek: false,
                peek_hover: false,
                peek_since: None,
                peek_hot: None,
                raise_since: None,
                line_hot: false,
                line_was_empty: true,
                menu_open: false,
                tag_card: None,
                peek_away: None,
                card_press: None,
                last_foreground: window::foreground().map(window::raw_of).unwrap_or(0),
                press_dragged: false,
                take_watch: None,
                esc_was_down: false,
                pending_foreground: None,
                stuck_foreground: None,
                recent_park: None,
                hang_undrawn: None,
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
                if let Err(error) =
                    RegisterHotKey(Some(self.hwnd), HOTKEY_HANG, mods, self.settings.hang_vk)
                {
                    crate::log!("hotkey {} was refused: {error}", self.settings.hang_label());
                    failed.push(self.settings.hang_label());
                }
            }
            if self.settings.toggle_vk != 0 {
                let mods = HOT_KEY_MODIFIERS(self.settings.toggle_mods) | MOD_NOREPEAT;
                if let Err(error) = RegisterHotKey(
                    Some(self.hwnd),
                    HOTKEY_TOGGLE,
                    mods,
                    self.settings.toggle_vk,
                ) {
                    crate::log!(
                        "hotkey {} was refused: {error}",
                        self.settings.toggle_label()
                    );
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
            let until = Instant::now() + Duration::from_secs(6);
            self.hold_until = Some(until);
            if !self.settings.always_on {
                self.transient_until = Some(until);
            }
            // The line is only useful once somebody knows it is there. Say how
            // it works, once.
            if !self.tray_icon.is_invalid() {
                tray::balloon(
                    self.hwnd,
                    self.tray_icon,
                    t("howto_title"),
                    t("howto_body"),
                    false,
                );
            }
            self.invalidate();
        }
        // Elevation is a real safety trade-off, not a success: say so once.
        if self.self_elevated {
            tray::balloon(
                self.hwnd,
                self.tray_icon,
                t("app_name"),
                t("elevated_tip"),
                true,
            );
        }
        // The line is the only window this app has. Tucked at the top edge it
        // reads as "ShaiWin never started", and that is exactly how every launch
        // that left it up there has been read. So a launch always brings it down:
        // resident mode keeps it, non-resident gets one look at it before the
        // visit rules put it away.
        self.revealed = true;
        self.dismissed = false;
        if self.settings.always_on {
            self.pinned = true;
            self.transient_until = None;
        } else {
            self.start_visit();
            let until = Instant::now() + LAUNCH_LOOK;
            self.hold_until = Some(until);
            self.transient_until = Some(self.transient_until.map_or(until, |t| t.max(until)));
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
            if !self.rope_hwnd.is_invalid() {
                let _ = DestroyWindow(self.rope_hwnd);
                self.rope_hwnd = HWND::default();
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
            let Some(now) = window::rect_of(window::hwnd_of(entry.hwnd)) else {
                continue;
            };
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
                window_rect: if entry.hooked_away {
                    now
                } else {
                    entry.placement.normal
                },
                centred: entry.centred,
                tag: entry.tag.clone(),
                tag_version: 0,
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
            self.worker.send(Job::Snapshot {
                id,
                hwnd: entry.hwnd,
            });
            if !self.icons.contains_key(&entry.exe) {
                self.worker.send(Job::Icon {
                    id,
                    exe_path: entry.exe.clone(),
                });
            }
        }
        if revealed_any {
            self.revealed = true;
            self.dismissed = false;
            self.pinned = self.settings.always_on;
            self.hold_until = Some(Instant::now() + Duration::from_secs(3));
            if !self.settings.always_on {
                self.transient_until = Some(Instant::now() + Duration::from_secs(6));
            }
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
                if let Some(id) = self
                    .line
                    .cards
                    .iter()
                    .find(|c| c.hwnd == lp.0)
                    .map(|c| c.id)
                {
                    self.take_down(id);
                }
                LRESULT(0)
            }
            WM_UNHOOK_HWND => {
                if let Some(id) = self
                    .line
                    .cards
                    .iter()
                    .find(|c| c.hwnd == lp.0)
                    .map(|c| c.id)
                {
                    self.unhook(id);
                }
                LRESULT(0)
            }
            WM_TAKE_ALL => {
                self.take_all();
                LRESULT(0)
            }
            WM_OPEN_SETTINGS => {
                self.open_settings();
                LRESULT(0)
            }
            WM_OPEN_TAG => {
                if let Some(id) = self
                    .line
                    .cards
                    .iter()
                    .find(|c| c.hwnd == lp.0)
                    .map(|c| c.id)
                {
                    let initial = self
                        .line
                        .get(id)
                        .map(|card| card.tag.clone())
                        .unwrap_or_default();
                    self.open_tag_dialog(id, &initial);
                }
                LRESULT(0)
            }
            WM_POINT_AT => {
                let packed = wp.0 as u32;
                self.hover_point = if packed == u32::MAX {
                    None
                } else {
                    Some((packed as i32, lp.0 as i32))
                };
                LRESULT(0)
            }
            WM_PRETEND_DRAG => {
                self.pretend_drag = wp.0 != 0;
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
                let edge = self
                    .card_at(local)
                    .filter(|id| *id != PEEK_ID)
                    .and_then(|id| {
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
                        // The card layer cannot take activation, so a click on a
                        // card is not a deactivation for the tag popover. Close
                        // it here, and save what was typed: the user's next
                        // gesture is about the card, not about the field. Posted
                        // rather than called: DestroyWindow would re-enter this
                        // handler while it still owns the app.
                        crate::tag_ui::close_saving();
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
                        let k = self.line.get(id).map_or(1.0, |card| card.scale)
                            * self.line.fit(&self.layout);
                        let height_unit = CARD_H * self.layout.scale * k;
                        let body_top =
                            rect.map_or(0.0, |rect| rect.bottom as f32 - height_unit * zoom_y0);
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
        // A Win32 popup menu is itself a topmost window while it is tracked,
        // and Windows keeps it above every other topmost window - the card
        // layer included. So opening a menu does not ask the cards to give up
        // the top: they stay exactly where they are, which is what "the line
        // is still on top while I use its menu" means. `menu_open` only tells
        // the poll not to re-decide the z-order, follow the monitor or slide
        // the line while the menu is anchored to it.
        self.menu_open = true;
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
        self.sync_rope();
        let at = tray::cursor();
        let cmd = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD,
                at.x,
                at.y,
                None,
                hwnd,
                None,
            )
            .0 as u32
        };
        // Cleared before the command runs: "add a tag" opens the editor, and
        // that one is the line being used again.
        self.menu_open = false;
        self.sync_rope();
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
                    // A tag is typed in, not picked from a list: every window
                    // needs a different name, and presets only get in the way.
                    let current = self
                        .card_menu_target
                        .and_then(|id| self.line.get(id))
                        .map(|card| card.tag.as_str())
                        .unwrap_or("");
                    if current.is_empty() {
                        add(CMD_TAG_EDIT, t("tag_menu"), MF_ENABLED);
                    } else {
                        add(
                            CMD_TAG_EDIT,
                            &format!("{}{}", t("tag_current"), current),
                            MF_ENABLED,
                        );
                        add(CMD_TAG_CLEAR, t("tag_clear"), MF_ENABLED);
                    }
                    sep();
                    add(CMD_UNHOOK, t("unhook"), MF_ENABLED);
                }
                MenuKind::Tray => {
                    add(
                        CMD_TAKE_ALL,
                        t("take_down_all"),
                        if self.line.hooked_count() > 0 {
                            MF_ENABLED
                        } else {
                            MF_GRAYED
                        },
                    );
                    sep();
                    let toggle = if self.revealed {
                        t("hide_line")
                    } else {
                        t("show_line")
                    };
                    add(
                        CMD_TOGGLE,
                        &format!("{toggle}\t{}", self.settings.toggle_label()),
                        MF_ENABLED,
                    );
                    add(CMD_SETTINGS, t("settings"), MF_ENABLED);
                    sep();
                    let check = if tray::start_at_login() {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    };
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
            CMD_TAKE_ALL => self.take_all(),
            CMD_TOGGLE => self.toggle_reveal(),
            CMD_SETTINGS => self.open_settings(),
            CMD_LOGIN => {
                tray::set_start_at_login(!tray::start_at_login());
            }
            CMD_QUIT => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
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
            CMD_TAG_EDIT => {
                if let Some(id) = self.card_menu_target {
                    let initial = self
                        .line
                        .get(id)
                        .map(|card| card.tag.clone())
                        .unwrap_or_default();
                    self.open_tag_dialog(id, &initial);
                }
            }
            CMD_TAG_CLEAR => {
                if let Some(id) = self.card_menu_target {
                    self.set_card_tag(id, "");
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
        if let Some(id) = self
            .line
            .cards
            .iter()
            .find(|c| c.hwnd == hwnd && c.away)
            .map(|c| c.id)
        {
            crate::log!("hang {hwnd}: hooked window put back on the line");
            self.rehang(id, Some(source));
            return;
        }
        if self.line.contains_hwnd(hwnd) {
            self.complain(t("already_hung").to_string());
            return;
        }
        self.worker.send(Job::Prepare {
            info,
            source,
            asked: Instant::now(),
        });
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
            parked: Parked {
                placement: plan.placement,
                parked: plan.target,
                was_maximized: plan.was_maximized,
            },
            window_rect: info.rect,
            centred: source.centred(),
            tag: String::new(),
            tag_version: 0,
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
                crate::log!(
                    "parked hwnd={} at {:?} (from {:?})",
                    info.hwnd,
                    parked.parked,
                    plan.placement.normal
                );
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
                self.pinned = self.settings.always_on;
                self.hold_until = Some(Instant::now() + Duration::from_secs(2));
                self.start_visit();
                // The card stays down unless the pointer is on the line: the
                // rope's own layer is what shows where the window went, and the
                // cards come up when he reaches for the rope. Raising them here
                // regardless is how a hang used to cover the window he went back
                // to.
                self.sync_journal();
                self.invalidate();
                self.hang_undrawn = Some((info.hwnd, Instant::now()));
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
        let Some(card) = self.line.get(id) else {
            return;
        };
        let hwnd = card.hwnd;
        let parked = restore_record(card, self.monitor.work);
        let unhook = card.unhook;
        let before = window::rect_of(window::hwnd_of(hwnd));
        // The line is topmost and the window is about to come back underneath
        // it. The frame on screen still has this card painted in it, so during
        // the restore the card sits on top of the window that just appeared -
        // the small card that flashes before the window shows. Paint the line
        // with the card already gone first; the restore then lands on a screen
        // that no longer shows it.
        self.redraw();
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
        // A non-resident line is a short visit, but this is the moment the user
        // is most likely to need it again: the window just opened, they move it
        // aside, and come back for the next card. The line waits for that. Held
        // as well as long-deadlined, because opening that window changed the
        // foreground, and a foreground change is one of the reasons a visit is
        // normally over - it must not cut the grace short in the same poll.
        if self.revealed {
            let now = Instant::now();
            self.keep_alive(now, VISIT_AFTER_TAKE);
            self.hold_until = Some(self.hold_until.map_or(now + VISIT_AFTER_TAKE, |until| {
                until.max(now + VISIT_AFTER_TAKE)
            }));
            // The window is back on the desktop and the user is about to move it
            // out of the way. That is this gesture, not the next one: while its
            // rectangle keeps changing under a held button the visit stays open,
            // instead of the line leaning on a grace period to stay out.
            self.take_watch = Some(TakeWatch {
                hwnd,
                rect: after.unwrap_or(parked.placement.normal),
                until: now + Duration::from_secs(6),
            });
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
        let away: Vec<u64> = self
            .line
            .cards
            .iter()
            .filter(|c| c.away)
            .map(|c| c.id)
            .collect();
        for id in away {
            self.painter.forget(id);
            self.line.remove(id);
        }
        let ids: Vec<u64> = self
            .line
            .cards
            .iter()
            .filter(|c| !c.leaving && !c.away)
            .map(|c| c.id)
            .collect();
        crate::log!("take_all: {} card(s), unhooking", ids.len());
        if ids.is_empty() {
            self.sync_journal();
            self.invalidate();
            return;
        }
        self.revealed = true;
        self.dismissed = false;
        self.pinned = self.settings.always_on;
        for id in &ids {
            if let Some(card) = self.line.get_mut(*id) {
                card.unhook = true;
            }
        }
        for id in ids {
            self.take_down(id);
        }
        // Everything went back because the user asked for it, so there is no
        // next card to pick: the line goes on the empty-line clock.
        self.start_visit();
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
            if !away && window::is_alive(hwnd) && !park::stow(window::hwnd_of(hwnd), &parked) {
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
                Reply::Prepared {
                    info,
                    image,
                    plan,
                    source,
                } => self.on_prepared(info, image, plan, source),
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
                    let Some(card) = self.line.get(id) else {
                        continue;
                    };
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
        let (cx, cy) = self.hover_point.unwrap_or_else(monitor::cursor);
        let cursor_moved = (cx - self.last_cursor.0).abs() + (cy - self.last_cursor.1).abs() >= 3;
        self.last_cursor = (cx, cy);

        // The line belongs to the screen the pointer is on. On a multi-monitor
        // desktop, moving to another display carries the cards and the rope
        // there in the same poll, so the drop band is always where the user is
        // about to let go. The bounds check keeps the monitor lookup off the
        // common path; a point inside the current screen cannot have moved it.
        // A card in hand or an open editor is a gesture that started here and
        // must not be yanked to another screen underneath the pointer; a window
        // being dragged is followed by the drag poll, which also knows where the
        // drop would land; an open popup menu is anchored to the cards it was
        // opened on.
        if self.card_press.is_none() && self.tag_card.is_none() && !self.menu_open {
            let b = self.monitor.bounds;
            let inside = cx >= b.left && cx < b.right && cy >= b.top && cy < b.bottom;
            if !inside {
                let at = monitor::at_point(cx, cy);
                if at.handle != self.monitor.handle {
                    crate::log!("line follows the pointer to another monitor");
                    self.place(at);
                }
            }
        }

        if self.polls % 20 == 0 {
            self.refresh_titles();
        }
        if self.polls % 8 == 0 {
            self.poll_windows(now);
        }

        let left_state = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) };
        let left_down = left_state < 0;
        let left_pressed = left_down && !self.left_down;
        let left_released = !left_down && self.left_down;
        self.left_down = left_down;
        // Bit 0 is not the state, it is a latch: "the button went down sometime
        // since the last time we asked". The edges above miss a click that opens
        // and closes between two polls, and a click on the window he is already
        // using changes no foreground either - that is the click that used to
        // leave the cards painted over his work.
        let press_latched = (left_state & 1) != 0;
        let esc_state = unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) };
        let esc_down = esc_state < 0;
        // Escape while the tag field has focus belongs to the field: it puts
        // the old tag back and must not also put the whole line away.
        let esc_pressed =
            ((esc_down && !self.esc_was_down) || (esc_state & 1) != 0) && self.tag_card.is_none();
        self.esc_was_down = esc_down;
        if left_pressed || press_latched {
            self.press_dragged = false;
        }
        self.poll_drag(cx, cy, left_down);

        // The card under the editor can go away - taken down, unhooked, gone
        // from the line - while the popover is still open. Saving leaves the
        // label in the journal for when the window comes back; closing on the
        // user's typing would lose it.
        if let Some(id) = self.tag_card {
            if self.line.get(id).is_none_or(|card| card.leaving) {
                crate::tag_ui::close_saving();
            }
        }

        // A parked window that the user brings forward (taskbar, Alt+Tab,
        // another app activating it) is a request to see it. Move it back on
        // screen instead of leaving it invisible. The foreground is recorded
        // per poll, so simply hanging the window it was using is not a
        // transition.
        let foreground = window::foreground().map(window::raw_of).unwrap_or(0);
        let foreground_changed = foreground != self.last_foreground;
        // The tag popover and the settings window are ours. Their taking the
        // foreground is not the user going back to work, and treating it as one
        // is what used to sink the cards the moment the editor opened.
        let foreground_ours =
            foreground != 0 && window::pid_of(window::hwnd_of(foreground)) == self.self_pid;
        let foreground_left = foreground_changed && !foreground_ours;
        if foreground_changed {
            let previous = std::mem::replace(&mut self.last_foreground, foreground);
            self.pending_foreground = None;
            let just_parked = self.recent_park.is_some_and(|(hwnd, at)| {
                hwnd == foreground && now.duration_since(at) < PARK_FOREGROUND_GRACE
            });
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
                crate::log!(
                    "foreground: hwnd={} would not give up the focus; leaving it there",
                    stuck.hwnd
                );
                self.stuck_foreground = None;
            } else if now.duration_since(stuck.last_try) >= STUCK_RETRY {
                // Only hand it to a real window this time. Activating the
                // desktop, or switching through candidates, repaints the
                // taskbar for every step and that is the flicker the user sees.
                if window::hand_off_foreground(window::hwnd_of(stuck.hwnd), self.self_pid) {
                    self.last_foreground = window::foreground().map(window::raw_of).unwrap_or(0);
                    self.stuck_foreground = None;
                    crate::log!(
                        "foreground: handed the focus away from the parked hwnd={} on retry",
                        stuck.hwnd
                    );
                } else {
                    self.stuck_foreground = Some(StuckFocus {
                        last_try: now,
                        ..stuck
                    });
                }
            }
        }

        let top = self.monitor.bounds.top;
        let at_top =
            cy <= top + 2 && cx >= self.monitor.bounds.left && cx < self.monitor.bounds.right;
        // After the line is put away the pointer is often still resting on the
        // edge. Wait until it leaves before the pill may appear again, otherwise
        // it pops straight back out from under the cursor.
        if !at_top {
            self.hover_ready = true;
        }
        let local = (cx - self.origin.0, cy - self.origin.1);
        let peek_hover = self.peek && self.peek_at(local);
        if peek_hover != self.peek_hover {
            self.peek_hover = peek_hover;
            self.invalidate();
        }

        // The strip rectangle is not the line. It is as tall as the cards and as
        // wide as the monitor, so a cursor working near the top of a maximized
        // window sits inside it without touching anything the rack owns. Only the
        // rope, its hang band, and the cards count as being on the line.
        //
        // A card is a handle only where it is really on screen. The layer is sent
        // to the bottom of the z-order when it is not in use and the strip is
        // exactly where a maximized window lives, so the card's old place must
        // not pull the rack back over the work. A card the user can see, though,
        // is part of the line: that is what makes a visible card hoverable and
        // clickable without first reaching for the rope. The rope - not the old
        // position - is what always brings a covered line back.
        let card_hit = self.card_at(local).filter(|id| *id != PEEK_ID);
        let on_card = card_hit.is_some() && (self.topmost || !self.covered_at((cx, cy)));
        let over_rope =
            self.revealed && self.on_rope(local, (self.slide - 1.0) * self.layout.panel_height());
        let on_line = self.revealed && (over_rope || on_card);
        let in_hand = self.in_hand();
        // Only gestures of the line's own keep the cards up. A window of the
        // user's being dragged past the top is not one: the cards must not come
        // up over it, which is what `in_hand` alone used to make them do.
        let holding = self.card_press.is_some() || self.tag_card.is_some();
        let dragging = self.drag.is_some() || self.pretend_drag;
        // A click that lands off the line and moved nothing is the user going
        // elsewhere. Judged when the button is up, from either edge of it: the
        // press that starts dragging a window to the rope also lands off the line,
        // and the cards must not sink in the middle of that gesture.
        let click_off_line = !left_down
            && (left_released || press_latched)
            && !self.press_dragged
            && !on_line
            && !in_hand;
        // The window just taken down being dragged about is the same gesture as
        // taking it: as long as its rectangle keeps changing under a held button,
        // the visit is still running.
        let taking_away = self.poll_take_watch(now, left_down);

        if !self.revealed {
            self.transient_until = None;
            if self.settings.always_on {
                // Resident mode: while the line is put away, the top edge does
                // not summon a pill or the line itself. The show/hide shortcut
                // is the only way back, which is what "resident" means.
                if self.peek {
                    self.close_peek();
                }
            } else {
                self.poll_peek(now, cx, cy, cursor_moved);
            }
        } else {
            if self.peek {
                self.close_peek();
            }
            if esc_pressed && !in_hand {
                // Escape means the look is over. It works in both modes: in
                // resident mode it is the quickest way to put the line away, and
                // in the ordinary one there is nothing left to wait out.
                self.tuck_line("Escape");
            } else if self.settings.always_on {
                // Resident: stays down until the user puts it away on purpose.
                self.transient_until = None;
            } else {
                self.poll_visit(now, on_line, taking_away, click_off_line, foreground_left);
            }
        }

        // The cards hold the top of the screen only while the line itself is
        // being used: a card in the hand, the editor, or the pointer on a card
        // that is actually visible or on the rope. Any other place - his work,
        // another window, the taskbar - and the layer goes back behind it. That
        // is the whole rule now: no click or foreground event has to be watched
        // for, because a pointer that is not on the line is already the answer.
        // The rope's own topmost layer keeps the line in sight while the cards
        // are down, and reaching for it is what brings them back.
        //
        // A window being dragged is the one case that always sinks the cards. It
        // is the user's own gesture on his own window, it is usually headed for
        // the top of the screen, and a wall of cards coming up over it is exactly
        // in the way. The rope and its drop band stay visible on their own layer.
        //
        // The one exception is an open menu of ours: the popup is above the
        // card layer on its own, and the layer must keep the z-order it had
        // when the menu opened until the menu closes.
        if self.revealed && !self.menu_open {
            if holding {
                self.raise_since = None;
                self.set_topmost(true);
            } else if dragging {
                self.raise_since = None;
                self.set_topmost(false);
            } else if on_card {
                // A visible card under the pointer means the line is being used.
                self.raise_since = None;
                self.set_topmost(true);
            } else if over_rope {
                // The rope is not a button: a pointer crossing it on the way to
                // a title bar must not bring the cards back. Rest on it for a
                // moment and the line answers.
                if self.topmost {
                    self.raise_since = None;
                } else {
                    let since = *self.raise_since.get_or_insert(now);
                    if now.duration_since(since) >= RAISE_DWELL {
                        self.raise_since = None;
                        self.set_topmost(true);
                    }
                }
            } else {
                self.raise_since = None;
                self.set_topmost(false);
            }
        } else {
            self.raise_since = None;
        }

        // Hover and click-through: a card is a target wherever the pointer can
        // reach it. Clearing the click-through bit over one is what lets a click
        // land on a demoted layer; where another window is above that point, the
        // window still wins the click, so this cannot eat a click meant for the
        // user's work. The layer being down used to make the whole strip
        // click-through, which is why a visible card ignored clicks until the
        // rope had brought the cards back up.
        let next_hover = if self.revealed { card_hit } else { None };
        if next_hover != self.hover {
            self.hover = next_hover;
            self.invalidate();
        }
        let over_card = next_hover.is_some();
        self.set_click_through(!over_card && !peek_hover && self.card_press.is_none());

        if self.hold_until.is_some_and(|until| now >= until) {
            self.hold_until = None;
        }

        if self.line.gauge(now) {
            self.invalidate();
        }
    }

    /// Puts the line away without the user asking. The difference from a
    /// deliberate hide is only that nothing in the log says the user did it;
    /// both leave the line inert until the pill, the shortcut or a hang brings
    /// it back.
    fn tuck_line(&mut self, why: &str) {
        self.revealed = false;
        self.pinned = false;
        self.dismissed = true;
        self.hold_until = None;
        self.transient_until = None;
        self.band_hot = false;
        self.line_hot = false;
        self.hover = None;
        self.close_peek();
        // The pointer may still be on the edge. Do not let the pill appear under
        // it before it has left once.
        self.hover_ready = false;
        crate::log!("line hidden: {why}");
        self.invalidate();
    }

    /// Starts a non-resident visit: the line is on screen from now until its
    /// clock runs out. Resident mode has no clock, so it clears one. A reveal is
    /// the "pulled it out for a look" case, so it gets the fallback clock; the
    /// gestures that follow are what set the short ones. An empty line is a
    /// place to put a window rather than a look at one, so it waits the longer
    /// empty-line time from the start.
    fn start_visit(&mut self) {
        self.transient_until = if self.settings.always_on {
            None
        } else {
            let empty = self.line.live_count() == 0;
            self.auto_hide().map(|after| {
                Instant::now()
                    + if empty {
                        after.max(EMPTY_LINE_LINGER)
                    } else {
                        after
                    }
            })
        };
    }

    /// The non-resident line's clock. While the pointer is on the line it is
    /// kept alive; this is how long the line gets after the pointer leaves.
    /// `auto_hide_secs = 0` means the user prefers to put it away by hand.
    fn auto_hide(&self) -> Option<Duration> {
        (self.settings.auto_hide_secs > 0)
            .then(|| Duration::from_secs(self.settings.auto_hide_secs as u64))
    }

    /// Guarantees the line stays down for at least `after` from now. Never
    /// shortens a visit that is already going to last longer.
    fn keep_alive(&mut self, now: Instant, after: Duration) {
        let until = now + after;
        self.transient_until = Some(
            self.transient_until
                .map_or(until, |current| current.max(until)),
        );
    }

    /// Shortens a transient visit to at most `after` from now. Never lengthens
    /// it: a foreground change or a click outside should not extend a line that
    /// is already on its way out.
    fn tuck_soon(&mut self, now: Instant, after: Duration) {
        let until = now + after;
        self.transient_until = Some(
            self.transient_until
                .map_or(until, |current| current.min(until)),
        );
    }

    /// The rules of a non-resident visit.
    ///
    /// The line is a short visit, not furniture. Everything that means "the
    /// user is still using this" restarts the same short clock: the pointer on
    /// the rope or a card, a card pressed, a window or card in hand, the tag
    /// editor open, a menu of ours up. When the pointer leaves, `auto_hide_secs`
    /// (1 s by default) is all it gets to be picked up again; `0` removes the
    /// clock and leaves only Escape and the shortcut.
    ///
    /// Deliberate leave-taking does not wait for the clock. A click away or
    /// another window taking the foreground ends the visit after the slide-out
    /// (0.28 s). An empty line is the exception in both directions: there is
    /// nothing to look at yet, and the user is most likely off choosing the
    /// window that goes on it - so a click into that window is not the user
    /// leaving, and the line waits `EMPTY_LINE_LINGER` after the pointer goes.
    fn poll_visit(
        &mut self,
        now: Instant,
        on_line: bool,
        taking_away: bool,
        click_off_line: bool,
        foreground_left: bool,
    ) {
        let busy = self.in_hand() || taking_away || self.menu_open;
        let held = self.hold_until.is_some_and(|until| now < until);
        let Some(after) = self.auto_hide() else {
            return;
        };
        if busy || on_line {
            self.keep_alive(now, after);
        }
        let left_line = std::mem::replace(&mut self.line_hot, on_line);
        let empty = self.line.live_count() == 0;
        let became_empty = empty && !self.line_was_empty;
        self.line_was_empty = empty;
        if !busy && !held && empty && (became_empty || left_line) {
            // One shot per edge, or re-arming every poll would push the
            // deadline out forever and an empty line would never leave.
            self.keep_alive(now, EMPTY_LINE_LINGER);
        }
        if !busy && !held && !empty && (foreground_left || click_off_line) {
            self.tuck_soon(now, VISIT_LEAVE);
        }
        if !busy && !held && now >= self.transient_until.unwrap_or(now) {
            self.tuck_line("the visit is over");
        }
    }

    /// Whether the user is holding something the line is part of: a window
    /// being dragged on it, a card being reordered, the drop band lit, or the
    /// tag editor open on a card.
    fn in_hand(&self) -> bool {
        self.drag.is_some()
            || self.pretend_drag
            || self.card_press.is_some()
            || self.band_hot
            || self.tag_card.is_some()
    }

    /// Whether the window that was just taken down is being dragged away from
    /// the line. Reports true while its rectangle keeps changing under a held
    /// button; the motion is the gesture, so the visit stays open as long as it
    /// lasts and gets the ordinary slack when it stops.
    fn poll_take_watch(&mut self, now: Instant, left_down: bool) -> bool {
        let Some(watch) = self.take_watch else {
            return false;
        };
        if now >= watch.until || !left_down {
            self.take_watch = None;
            return false;
        }
        let hwnd = window::hwnd_of(watch.hwnd);
        let Some(rect) = window::visible_rect(hwnd) else {
            self.take_watch = None;
            return false;
        };
        let moved = (rect.left - watch.rect.left).abs() > 4
            || (rect.top - watch.rect.top).abs() > 4
            || (rect.right - watch.rect.right).abs() > 4
            || (rect.bottom - watch.rect.bottom).abs() > 4;
        if moved {
            self.take_watch = Some(TakeWatch { rect, ..watch });
        }
        moved
    }

    /// Whether the pointer is near the drawn rope. The old answer included the
    /// whole drop band, which made a pointer passing over a maximized window's
    /// title bar pull the card wall back over that window. The rope is drawn at
    /// `rope_y`, so the pointer has to be near that curve.
    fn on_rope(&self, local: (i32, i32), y_off: f32) -> bool {
        let x = local.0 as f32;
        if x < 0.0 || x > self.layout.width {
            return false;
        }
        let rope = self.layout.rope_y(x) + y_off;
        // `ROPE_REACH` is the grab band: wider than the stroke, still clear of
        // the card tops, so reaching for the rope never turns the strip below
        // it - where the user's windows live - into a handle.
        (local.1 as f32 - rope).abs() <= self.layout.rope_reach()
    }

    /// Whether some other window is painted over the card layer at `point`
    /// (screen pixels). The layer is demoted to the bottom of the z-order when
    /// it is not in use, so a card may still be there and be covered by the
    /// window the user is working in. That covered place is not a handle; only
    /// a card that is really visible is. Transparent windows - our own rope layer
    /// among them - do not count: they let the pointer through to what is under
    /// them, just as they let a click through. Windows DWM has cloaked paint
    /// nothing at all, even when they cover the whole screen - the input host's
    /// CoreWindow is one - so they do not count either.
    fn covered_at(&self, point: (i32, i32)) -> bool {
        unsafe {
            let mut above = GetWindow(self.hwnd, GW_HWNDPREV);
            for _ in 0..512 {
                let Ok(w) = above else { break };
                if w.is_invalid() {
                    break;
                }
                let ex = GetWindowLongPtrW(w, GWL_EXSTYLE) as u32;
                if IsWindowVisible(w).as_bool()
                    && ex & WS_EX_TRANSPARENT.0 == 0
                    && !IsIconic(w).as_bool()
                    && window::rect_of(w).is_some_and(|r| {
                        point.0 >= r.left
                            && point.0 < r.right
                            && point.1 >= r.top
                            && point.1 < r.bottom
                    })
                    && !window::is_cloaked(w)
                {
                    return true;
                }
                above = GetWindow(w, GW_HWNDPREV);
            }
        }
        false
    }

    /// The "show the line" pill, the way into a non-resident line that is not
    /// already down.
    ///
    /// The pill comes out as soon as the pointer reaches the top edge, because it
    /// carries the card count: the user has to be able to read it before deciding.
    /// Staying on the edge is that decision, so the line opens by itself after the
    /// dwell - 0.08 s with cards on the line, 0.22 s with none, when the trip to
    /// the top edge was probably for something else. Moving the pointer over the
    /// pill opens it immediately.
    fn poll_peek(&mut self, now: Instant, cx: i32, cy: i32, cursor_moved: bool) {
        let top = self.monitor.bounds.top;
        let wait = if self.line.live_count() == 0 {
            DWELL_EMPTY
        } else {
            DWELL
        };
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
            // The wait that opens the line counts from the moment the pointer is
            // on the button. A pointer that moves while it is there is the
            // clearest possible "I mean this button"; it opens at once, without
            // the click or the old 0.18 s pause. A pointer that simply found
            // itself on the pill when it appeared still gets the short dwell as
            // a fallback, so grazing the top edge cannot yank the line down.
            if self.peek_hover && self.drag.is_none() && self.card_press.is_none() {
                let since = *self.peek_hot.get_or_insert(now);
                if cursor_moved || now.duration_since(since) >= wait {
                    self.open_from_peek();
                    return;
                }
            } else {
                self.peek_hot = None;
            }
            // The wait is on the button, not on the edge: a pointer that came to
            // the pill and never settled on it has no clock running against it.
            if self.peek_hot.is_none()
                && self
                    .peek_since
                    .is_some_and(|t| now.duration_since(t) >= PEEK_LIFE)
            {
                self.close_peek();
            }
            return;
        }
        let armed = self.hover_ready
            && cx >= self.monitor.bounds.left
            && cx < self.monitor.bounds.right
            && cy <= top + 2
            && self.drag.is_none()
            && self.card_press.is_none();
        if armed {
            self.peek = true;
            self.peek_since = Some(now);
            self.peek_hot = None;
            self.peek_away = None;
            self.peek_hover = false;
            crate::log!("line peek shown");
            self.invalidate();
        }
    }

    fn close_peek(&mut self) {
        if !self.peek {
            return;
        }
        self.peek = false;
        self.peek_hover = false;
        self.peek_since = None;
        self.peek_hot = None;
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
        self.peek_hot = None;
        self.peek_away = None;
        self.revealed = true;
        self.dismissed = false;
        self.pinned = false;
        // The pill was a way in, not a request to keep the line on the screen
        // forever: the visit it opens is the short one.
        self.start_visit();
        self.hold_until = None;
        crate::log!("line shown: peek button");
        self.invalidate();
    }

    fn poll_drag(&mut self, cx: i32, cy: i32, down: bool) {
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
        if !self.revealed {
            // A line that is not on screen has nothing to hang a window on.
            // It used to come back down whenever a window was dragged into the
            // place it had occupied, which made merely moving a window to the
            // top open the line and hang it. The pill and the show/hide
            // shortcut are the ways back.
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
                    let on_line =
                        rect.is_some_and(|rect| self.layout.window_on_line(rect.top, screen_top));
                    let where_ = format!(
                        " (top={} edge={} band={})",
                        rect.map(|rect| rect.top).unwrap_or(screen_top),
                        screen_top,
                        self.layout.hang_depth()
                    );
                    if zoomed
                        || rect.is_some_and(|rect| self.layout.window_flush(rect.top, screen_top))
                    {
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
                        crate::log!(
                            "drag {:#x}: released on the line at y={cy}{where_}",
                            watch.hwnd
                        );
                        self.request_hang(watch.hwnd, HangSource::Drag);
                    } else {
                        crate::log!(
                            "drag {:#x}: released off the line at y={cy}{where_}",
                            watch.hwnd
                        );
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
                        && window::rect_of(*fg).is_some_and(|r| {
                            cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom
                        })
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
                        crate::log!(
                            "drag: the move loop is dragging a window that cannot be hung ({why})"
                        );
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
            let Some(started) = window::visible_rect(hwnd) else {
                return;
            };
            crate::log!(
                "drag {raw:#x}: following {} class={}{}",
                window::describe(raw),
                window::class_name(hwnd),
                if moving.is_some() { " (move loop)" } else { "" }
            );
            self.drag = Some(DragWatch {
                hwnd: raw,
                started,
                moved: false,
                told_tucked: false,
            });
            self.press_dragged = true;
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
                    crate::log!(
                        "drag {:#x}: the move loop names {raw:#x}; following that",
                        watch.hwnd
                    );
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
        let travelled = self
            .press_at
            .is_some_and(|(px, py)| (cx - px).abs() > 8 || (cy - py).abs() > 8);
        let mut moved = false;
        let mut rect = None;
        if let Some(watch) = self.drag.as_mut() {
            rect = window::visible_rect(window::hwnd_of(watch.hwnd));
            let rect_moved = rect.is_some_and(|rect| {
                (rect.left - watch.started.left).abs() > 8
                    || (rect.top - watch.started.top).abs() > 8
            });
            if travelled && (moving.is_some() || rect_moved) && !watch.moved {
                crate::log!(
                    "drag {:#x}: the pointer has moved; the line is armed",
                    watch.hwnd
                );
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
        // A line that is not on screen never reaches here: `poll_drag` returns
        // before it starts watching a window. So a drag can only hang when the
        // line is actually visible, which is what stops a window moved past the
        // top edge from opening the line and hanging itself by accident.
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
        if near && !self.settings.always_on {
            // Keep a short visit alive while a window is being aimed at the
            // rope; the deadline starts again once the gesture is over.
            let until = Instant::now() + VISIT_GRACE;
            self.transient_until = Some(
                self.transient_until
                    .map_or(until, |current| current.max(until)),
            );
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
            if hot {
                "the window is on the line, letting go will hang it"
            } else {
                "off the line"
            }
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
                    crate::log!(
                        "card {id}: window {hwnd} moved but is still off screen; restoring it"
                    );
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
        self.line
            .cards
            .iter()
            .find(|c| c.hwnd == hwnd && !c.away && !c.leaving)
            .map(|c| c.id)
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
                self.stuck_foreground = Some(StuckFocus {
                    last_try: now,
                    ..stuck
                });
            }
            _ => {
                crate::log!("foreground: hwnd={raw} is off screen but kept the focus; nothing else would take it");
                self.stuck_foreground = Some(StuckFocus {
                    hwnd: raw,
                    since: now,
                    last_try: now,
                });
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
        let Some(card) = self.line.get(id) else {
            return;
        };
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
            (
                placement,
                park::hung_up_in_place(placement),
                placement.normal,
            )
        } else {
            let previous = window::foreground();
            let before = window::rect_of(h).unwrap_or(card.window_rect);
            let result =
                park::plan(h).and_then(|plan| park::execute(h, &plan).map(|parked| (plan, parked)));
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
                            self.last_foreground =
                                window::foreground().map(window::raw_of).unwrap_or(0);
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
                crate::log!(
                    "rehang {id}: hwnd={hwnd} was already minimized; hung it up where it stands"
                );
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
            // Put away on purpose. The line is inert until the shortcut, the
            // pill or a hang brings it back: a window over the old spot neither
            // lights it nor hangs.
            self.tuck_line("the user put it away");
            return;
        }
        // Asked for on purpose. Resident mode stays down until the next
        // shortcut; a non-resident line gets its short visit.
        self.revealed = true;
        self.pinned = self.settings.always_on;
        self.dismissed = false;
        self.hold_until = None;
        self.peek = false;
        self.peek_hover = false;
        self.start_visit();
        let (x, y) = monitor::cursor();
        let at = monitor::at_point(x, y);
        if at.handle != self.monitor.handle {
            self.place(at);
        }
        crate::log!(
            "line toggled: revealed={} pinned={}",
            self.revealed,
            self.pinned
        );
        self.invalidate();
    }

    // MARK: Settings

    /// Shows `tag` on the card without writing the journal. The editor calls
    /// this on every keystroke, so the label on the card behind the popover is
    /// the preview: what the user types is what he will see.
    pub(crate) fn preview_card_tag(&mut self, id: u64, tag: &str) {
        let tag = settings::clean_tag(tag);
        let Some(card) = self.line.get_mut(id) else {
            return;
        };
        if card.tag == tag {
            return;
        }
        card.tag = tag;
        card.tag_version = card.tag_version.wrapping_add(1);
        self.painter.forget(id);
        self.invalidate();
    }

    /// Puts a short label on a card and makes it the one that survives a
    /// restart. The tag is user content, so it is cleaned only of the characters
    /// that would break the journal.
    pub(crate) fn set_card_tag(&mut self, id: u64, tag: &str) {
        let tag = settings::clean_tag(tag);
        let Some(card) = self.line.get_mut(id) else {
            return;
        };
        let changed = card.tag != tag;
        if changed {
            card.tag = tag.clone();
            card.tag_version = card.tag_version.wrapping_add(1);
            self.painter.forget(id);
            crate::log!("card {id}: tag is now {tag:?}");
        }
        // A live preview has already put the value in the card, so "unchanged"
        // here does not mean the journal already has it: always commit.
        self.write_journal();
        self.invalidate();
    }

    /// Opens the tag editor under the card. The cards keep the top: the whole
    /// point of the editor is the label appearing on the card while the user
    /// types, and a modal box in the middle of the screen showed neither.
    fn open_tag_dialog(&mut self, id: u64, initial: &str) {
        if self.tag_card == Some(id) {
            crate::tag_ui::raise();
            return;
        }
        crate::tag_ui::commit_open();
        self.tag_card = Some(id);
        self.set_topmost(true);
        self.keep_alive(Instant::now(), VISIT_AFTER_TAG);
        let anchor = self.card_rect(id).map(|rect| WRect {
            left: rect.left + self.origin.0,
            top: rect.top + self.origin.1,
            right: rect.right + self.origin.0,
            bottom: rect.bottom + self.origin.1,
        });
        let app = self as *mut App;
        crate::tag_ui::open(self.hwnd, app, id, initial, self.monitor, anchor);
    }

    /// The editor closed. The line keeps the top for a moment so the new label
    /// is actually read, then the ordinary visit clock takes over.
    pub(crate) fn tag_dialog_closed(&mut self) {
        if self.tag_card.take().is_none() {
            return;
        }
        let now = Instant::now();
        self.hold_until = Some(now + VISIT_AFTER_TAG);
        if self.revealed {
            self.keep_alive(now, VISIT_AFTER_TAG);
        } else {
            self.start_visit();
        }
        self.invalidate();
    }

    fn open_settings(&mut self) {
        if !self.settings_wnd.is_invalid() && unsafe { IsWindow(Some(self.settings_wnd)).as_bool() }
        {
            unsafe {
                let _ = ShowWindow(self.settings_wnd, SW_RESTORE);
                let _ = SetForegroundWindow(self.settings_wnd);
            }
            return;
        }
        let app = self as *mut App;
        self.settings_wnd = settings_ui::open(
            self.window_icon,
            self.tray_icon,
            &self.settings,
            self.monitor,
            app,
        )
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
        self.relayout();
        self.revealed = true;
        self.dismissed = false;
        self.pinned = self.settings.always_on;
        let until = Instant::now() + Duration::from_millis(3500);
        self.hold_until = Some(until);
        if !self.settings.always_on {
            self.transient_until = Some(until);
        }
        self.invalidate();
    }

    /// Recompute the geometry for the monitor we are on after a setting that
    /// moves the rope, without resetting the panel first. `place` starts from
    /// the bare strip, so calling it on every drag of the settings preview
    /// shrank the panel to below the cards and `ensure_panel` grew it back on
    /// the next frame - which is the flashing the preview used to cause.
    fn relayout(&mut self) {
        self.layout = self
            .settings
            .layout_for(self.monitor.bounds.width() as f32, self.monitor.scale);
        self.ensure_panel();
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
        always_on: bool,
        drag_hang: bool,
    ) -> Result<(), String> {
        self.apply_hotkeys(hang.0, hang.1, toggle.0, toggle.1)?;
        self.settings.hang_mods = hang.0;
        self.settings.hang_vk = hang.1;
        self.settings.toggle_mods = toggle.0;
        self.settings.toggle_vk = toggle.1;
        self.settings.sag_percent = if sag_percent < 0.0 {
            SAG_AUTO
        } else {
            sag_percent.clamp(0.0, SAG_PERCENT_MAX)
        };
        self.settings.always_on = always_on;
        self.settings.drag_hang = drag_hang;
        self.settings.save();
        // Applying the mode immediately is the difference between a setting and
        // a promise: turning "keep the line on screen" on should bring it down,
        // and turning it off should hand the line over to the short-visit rules.
        if always_on {
            self.revealed = true;
            self.pinned = true;
            self.dismissed = false;
            self.transient_until = None;
            self.peek = false;
            self.peek_hover = false;
        } else {
            self.pinned = false;
            if self.revealed {
                self.start_visit();
            } else {
                self.transient_until = None;
            }
        }
        crate::log!(
            "settings saved: hang={} toggle={} sag={} always_on={} drag_hang={}",
            self.settings.hang_label(),
            self.settings.toggle_label(),
            if self.settings.sag_is_auto() {
                "auto".to_string()
            } else {
                format!("{:.1}%", self.settings.sag_percent)
            },
            always_on,
            drag_hang
        );
        // A different sag changes the strip height and where the cards sit.
        self.relayout();
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
            return Err(format!(
                "{}: {}",
                t("settings_hotkey_failed"),
                failed.join(", ")
            ));
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
        self.sync_rope();
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
            if let Some((hwnd, parked_at)) = self.hang_undrawn.take() {
                crate::log!(
                    "card {hwnd} drawn {} ms after the park",
                    now.duration_since(parked_at).as_millis()
                );
            }
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
                // Coming back on screen is the start of a use, so the layer
                // takes the top again: the pill has to be reachable above
                // whatever window happens to touch the top edge.
                self.topmost = true;
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                if !self.rope_hwnd.is_invalid() {
                    let _ = ShowWindow(self.rope_hwnd, SW_HIDE);
                }
                self.rope_shown = false;
            }
        }
        self.sync_rope();
    }

    /// The cards are a layer you reach into, not a layer that sits on your work.
    /// While the pointer is on the line, or a card or a window is in the hand,
    /// the layer is topmost so it can be seen and hit. A click that lands off the
    /// line, or another window taking the foreground, hands the top of the screen
    /// back.
    ///
    /// Handing it back is two moves, not one. Clearing the topmost style alone
    /// leaves the layer at the *head* of the normal band, which is above the
    /// window that was just clicked into - the cards stay exactly where they were
    /// and the user cannot see what he is working in. Going to the bottom puts
    /// them behind it, which is the only reading of "do not cover my work".
    fn set_topmost(&mut self, on: bool) {
        if self.topmost == on {
            return;
        }
        self.topmost = on;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            if !on {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_BOTTOM),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
        // The editor is a small topmost window of ours; taking the top for the
        // cards must put it back above the panel, or the live preview it exists
        // for is hidden under the card it belongs to.
        if on {
            crate::tag_ui::raise();
        }
        // Cards give the top back, the rope does not. Its own layer keeps the
        // line visible above the window just clicked into; the next pointer
        // move onto it raises the cards again.
        self.sync_rope();
        crate::log!(
            "card layer {}",
            if on {
                "takes the top"
            } else {
                "sinks behind the work"
            }
        );
        self.invalidate();
    }

    /// Shows the rope's always-on-top layer exactly while the card layer is not
    /// topmost. When the cards have the top, they draw the rope themselves;
    /// showing both would composite the same line twice.
    fn needs_rope(&self) -> bool {
        !self.rope_hwnd.is_invalid() && self.revealed && !self.topmost && self.slide > 0.001
    }

    fn sync_rope(&mut self) {
        let show = self.needs_rope();
        if show != self.rope_shown {
            self.rope_shown = show;
            unsafe {
                if show {
                    let _ = ShowWindow(self.rope_hwnd, SW_SHOWNOACTIVATE);
                    let _ = SetWindowPos(
                        self.rope_hwnd,
                        Some(HWND_TOPMOST),
                        self.panel.left,
                        self.panel.top,
                        self.panel.width(),
                        self.panel.height(),
                        SWP_NOACTIVATE,
                    );
                } else {
                    let _ = ShowWindow(self.rope_hwnd, SW_HIDE);
                }
            }
        } else if show {
            unsafe {
                let _ = SetWindowPos(
                    self.rope_hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// Where a resize or move of the layer should sit in the z-order: wherever
    /// `set_topmost` last put it. Passing `HWND_TOPMOST` here would silently
    /// undo a demotion the user asked for by clicking elsewhere - and passing
    /// `HWND_NOTOPMOST` would undo it too, because that one means "to the head
    /// of the normal band", which is above the window he is working in. The line
    /// changes height whenever a card settles, so this is not a rare path.
    fn z_for_move(&self) -> (Option<HWND>, SET_WINDOW_POS_FLAGS) {
        if self.topmost {
            (Some(HWND_TOPMOST), SWP_NOACTIVATE)
        } else {
            (None, SWP_NOACTIVATE | SWP_NOZORDER)
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
        self.layout = self
            .settings
            .layout_for(self.monitor.bounds.width() as f32, self.monitor.scale);
        self.panel = self.strip();
        self.origin = (self.panel.left, self.panel.top);
        let (z, flags) = self.z_for_move();
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                z,
                self.panel.left,
                self.panel.top,
                self.panel.width(),
                self.panel.height(),
                flags,
            );
        }
        self.move_rope();
        self.invalidate();
    }

    /// Keeps the rope layer over the same strip as the cards. It is click-through
    /// and only visible while the cards are sunk, so this is the entire cost of
    /// the second window when the line is being used normally.
    fn move_rope(&mut self) {
        if self.rope_hwnd.is_invalid() {
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                self.rope_hwnd,
                Some(HWND_TOPMOST),
                self.panel.left,
                self.panel.top,
                self.panel.width(),
                self.panel.height(),
                SWP_NOACTIVATE,
            );
        }
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
                rect.bottom = rect
                    .bottom
                    .max(self.monitor.bounds.top + bottom.ceil() as i32 + 10);
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
        let (z, flags) = self.z_for_move();
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                z,
                want.left,
                want.top,
                want.width(),
                want.height(),
                flags,
            );
        }
        self.move_rope();
        self.sync_rope();
        // Moving the panel re-asserts its z-order, which can put it over the
        // editor. The editor follows the card; it never goes under it.
        if self.topmost && self.tag_card.is_some() {
            crate::tag_ui::raise();
        }
    }

    fn redraw(&mut self) {
        // A demotion can land while the line is still sliding in, or the line
        // can start sliding out while the cards are demoted. Keep the rope's
        // companion layer in step with the state that decides whether it should
        // exist, instead of only at the moment `set_topmost` was called.
        if self.needs_rope() != self.rope_shown {
            self.sync_rope();
        }
        if !self.compose() {
            return;
        }
        if self.dump_request {
            self.dump_request = false;
            if let Some(dir) = &self.dump_dir {
                let _ = std::fs::create_dir_all(dir);
                self.dump_seq += 1;
                let path = dir.join(format!(
                    "frame-{}-{:02}.png",
                    std::process::id(),
                    self.dump_seq
                ));
                if let Some(canvas) = self.canvas.as_ref() {
                    let _ = canvas.save_png(&path);
                }
                self.dump_hits(dir);
            }
        }
        self.present();
        if self.rope_shown {
            self.present_rope();
        }
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
        if self
            .canvas
            .as_ref()
            .is_none_or(|c| c.width() != w || c.height() != h)
        {
            let Some(canvas) = Pixmap::new(w, h) else {
                return false;
            };
            self.canvas = Some(canvas);
        }
        let Some(mut canvas) = self.canvas.take() else {
            return false;
        };
        let frame = if self.peek && self.slide <= 0.001 {
            let hot = self.peek_hover;
            let cards = self.line.live_count();
            self.painter
                .compose_peek_into(&mut canvas, &self.layout, hot, cards)
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
                rope: self.topmost,
            };
            self.painter
                .compose_into(&mut canvas, &self.line, &self.layout, &ui)
        };
        self.canvas = Some(canvas);
        self.hits = frame.hits;
        true
    }

    fn present(&mut self) {
        let Some(canvas) = self.canvas.take() else {
            return;
        };
        present_layer(
            self.hwnd,
            &canvas,
            &mut self.dib,
            self.origin,
            &mut self.present_failed,
        );
        self.canvas = Some(canvas);
    }

    /// The rope-only companion layer, painted only while the cards are sunk.
    /// It is a full-panel layered window like the main one, so the rope keeps
    /// exactly the same curve and slide offset instead of being a second,
    /// subtly different drawing.
    fn present_rope(&mut self) {
        if self.rope_hwnd.is_invalid() {
            return;
        }
        let w = self.panel.width().max(1) as u32;
        let h = self.panel.height().max(1) as u32;
        if self
            .rope_canvas
            .as_ref()
            .is_none_or(|c| c.width() != w || c.height() != h)
        {
            let Some(canvas) = Pixmap::new(w, h) else {
                return;
            };
            self.rope_canvas = Some(canvas);
        }
        let Some(mut canvas) = self.rope_canvas.take() else {
            return;
        };
        let hot = self.band_hot && self.drag.is_some();
        let y_off = (self.slide - 1.0) * self.layout.panel_height();
        self.painter
            .compose_rope_into(&mut canvas, &self.layout, y_off, hot);
        present_layer(
            self.rope_hwnd,
            &canvas,
            &mut self.rope_dib,
            self.origin,
            &mut self.present_failed,
        );
        self.rope_canvas = Some(canvas);
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
        self.hits
            .iter()
            .rev()
            .find(|(hit, _)| *hit == id)
            .map(|(_, rect)| *rect)
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
                let zoom_y = crate::line::zoom_y_from_bottom(
                    point.1 as f32,
                    press.body_top,
                    press.height_unit,
                );
                // The top stays on the pin, so the card only moves its bottom;
                // the line does not need re-packing.
                changed |= self.line.set_zoom_y(press.id, zoom_y);
            }
            if edge.resizes_width() {
                let zoom =
                    crate::line::zoom_from_edge(point.0 as f32, press.centre, self.layout.scale);
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
                self.press_dragged = true;
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
        self.press_dragged = true;
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
        let Some(press) = self.card_press.take() else {
            return;
        };
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
fn present_layer(
    hwnd: HWND,
    canvas: &Pixmap,
    dibs: &mut Option<Dib>,
    origin: (i32, i32),
    failed: &mut bool,
) {
    let (w, h) = (canvas.width() as i32, canvas.height() as i32);
    // Grow the DIB only when the frame no longer fits in it. A flight makes
    // the panel grow and shrink with every frame; allocating the GDI bitmap
    // each time is a cost the frame rate pays for no visible reason.
    let reuse = dibs.as_ref().is_some_and(|d| {
        let area = (d.w * d.h) as i64;
        let want = (w * h) as i64;
        d.w >= w && d.h >= h && area <= want * 5 / 2
    });
    if !reuse {
        *dibs = Dib::new(w, h);
    }
    let Some(dib) = dibs.as_ref() else { return };
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
    let origin = POINT {
        x: origin.0,
        y: origin.1,
    };
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
            hwnd,
            None,
            Some(&origin),
            Some(&size),
            Some(dib.hdc),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        ) {
            if !*failed {
                *failed = true;
                crate::log!("UpdateLayeredWindow failed (nothing will show): {error}");
            }
        }
    }
}

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
        tag: card.tag.clone(),
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

unsafe extern "system" fn rope_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // The rope is a visual reminder, not a target. Every click belongs
            // to the window the user is working in, even when the pointer is on
            // the rope; the poll watches the pointer and raises the card layer.
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
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
