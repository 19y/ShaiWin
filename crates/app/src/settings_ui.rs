//! The settings window.
//!
//! A paper panel, hand-painted: two group headings, a hairline rule between
//! them, the rope, and two switches. The rope is the thing worth remembering,
//! so it is the only warm colour in the window, and it is the only control you
//! operate by dragging rather than by reading a number.
//!
//! The curve is deliberately not a slider. "1.4% of the monitor width" is not
//! something a person has an opinion about; bending a rope is. Pulling the
//! little rope in this window bends the real one at the top of the screen at
//! the same moment, so the judgement happens where the line is used. A
//! percentage never had to be shown to anybody.
//!
//! The shortcut boxes listen for a combination and paint the result, because
//! the stock one brings its own grey box, its own font and a blank field that
//! says nothing about what to do with it. Reading the modifiers off the keyboard
//! at the moment a key arrives is the whole trick, and `Key::held_mods` does it.
//!
//! Nothing in the window is left to a stock control's own drawing, not even the
//! buttons: the built-in ones set their own colours and would not sit on this
//! paper. Labels are left, controls are right on one vertical line, and the
//! measure in `Layout` is the single source of both, so the panel reads as a
//! list of settings instead of a scatter of widgets.
//!
//! The window is created on demand and destroyed when it closes; only one may
//! exist at a time. It talks back to [`App`] through the pointer handed to it
//! by `open`, so saving is a direct call, not a file round-trip.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use shaiwin_core::layout::SAG_PERCENT_MAX;
use shaiwin_platform_win::monitor::Monitor;
use tiny_skia::{Color, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
    InvalidateRect, LineTo, MoveToEx, RoundRect, SelectObject, SetBkMode, SetTextColor,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, DEFAULT_CHARSET, DEFAULT_GUI_FONT,
    DEFAULT_PITCH, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_TOP, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC,
    HFONT, HGDIOBJ, NULL_BRUSH, OUT_DEFAULT_PRECIS, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
    VK_HOME, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE,
    VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, GetWindowLongPtrW,
    GetWindowRect, IsDialogMessageW, KillTimer, LoadCursorW, MessageBoxW, RegisterClassExW,
    SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, ShowWindow, BM_CLICK,
    BN_CLICKED, CS_DBLCLKS, DLGC_BUTTON, DLGC_WANTALLKEYS, DLGC_WANTARROWS, DLGC_WANTCHARS,
    DM_GETDEFID, GWLP_USERDATA, HICON, HMENU, ICON_BIG, ICON_SMALL, IDC_ARROW, MB_ICONWARNING,
    MB_OK, MSG, SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_ERASEBKGND, WM_GETDLGCODE, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WM_SETFOCUS,
    WM_SETICON, WM_TIMER, WNDCLASSEXW, WNDCLASS_STYLES, WNDPROC, WS_CAPTION, WS_CHILD,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::app::App;
use crate::i18n::t;
use crate::settings;

const CLASS: PCWSTR = w!("ShaiWin.Settings");
const SWITCH_CLASS: PCWSTR = w!("ShaiWin.Settings.Switch");
const ROPE_CLASS: PCWSTR = w!("ShaiWin.Settings.Rope");
const KEY_CLASS: PCWSTR = w!("ShaiWin.Settings.Key");
const BUTTON_CLASS: PCWSTR = w!("ShaiWin.Settings.Button");

const SAVE_ID: i32 = 1;
const CANCEL_ID: i32 = 2;
const HANG_ID: i32 = 101;
const TOGGLE_ID: i32 = 102;
const ROPE_ID: i32 = 103;
const ALWAYS_ID: i32 = 104;
const DRAG_ID: i32 = 105;

// --- the panel -----------------------------------------------------------------
//
// One warm accent, and it belongs to the rope. Everything else is ink on paper
// with a rule between the groups, because a group of settings is a sequence,
// not a grid of boxes.
const PAPER: (u8, u8, u8) = (0xF7, 0xF8, 0xFA);
const FIELD: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
const INK: (u8, u8, u8) = (0x1A, 0x1D, 0x23);
const MUTED: (u8, u8, u8) = (0x6C, 0x74, 0x80);
const RULE_COLOUR: (u8, u8, u8) = (0xE2, 0xE5, 0xEA);
const EDGE: (u8, u8, u8) = (0xD3, 0xD8, 0xDF);
const ROPE_IDLE: (u8, u8, u8) = (0xA9, 0xB0, 0xBA);
const SLATE: (u8, u8, u8) = (0x26, 0x2C, 0x38);
const FLAX: (u8, u8, u8) = (0xDD, 0x9A, 0x4E);
const SWITCH_OFF: (u8, u8, u8) = (0xDD, 0xE1, 0xE6);

// --- the measure ---------------------------------------------------------------
//
// Every constant here is added into `LOGICAL_H` and walked again by `measure`,
// which is how the window is never a pixel too tall for its own rows.
const LOGICAL_W: f32 = 520.0;
const PAD: f32 = 24.0;
const PAD_BOTTOM: f32 = 22.0;
const HEAD_H: f32 = 16.0;
const HEAD_GAP: f32 = 12.0;
const ROW_H: f32 = 34.0;
const ROW_GAP: f32 = 6.0;
const GROUP_GAP: f32 = 16.0;
const RULE_H: f32 = 1.0;
const RULE_GAP: f32 = 16.0;
const ROPE_H: f32 = 88.0;
const ROPE_GAP: f32 = 8.0;
const NOTE_H: f32 = 18.0;
const ITEM_H: f32 = 40.0;
const TITLE_H: f32 = 22.0;
const ITEM_GAP: f32 = 10.0;
const FOOT_GAP: f32 = 32.0;
const BUTTON_W: f32 = 96.0;
const BUTTON_H: f32 = 32.0;
const BUTTON_GAP: f32 = 10.0;
const FIELD_W: f32 = 208.0;
const FIELD_H: f32 = 28.0;
/// The box the × of a shortcut field lives in: wide enough to hit without
/// aiming, narrow enough that the combination still reads.
const CLEAR_W: f32 = 26.0;
const SWITCH_W: f32 = 40.0;
const SWITCH_H: f32 = 22.0;
/// The switch's hit box is bigger than the track it shows. 40x22 is a small
/// thing to aim at twice a day; the extra is padding around the same shape, not
/// a second shape.
const SWITCH_BOX_W: f32 = 48.0;
const SWITCH_BOX_H: f32 = 26.0;
const GAP_LABEL: f32 = 14.0;

const LOGICAL_H: f32 = PAD
    + HEAD_H
    + HEAD_GAP
    + ROW_H
    + ROW_GAP
    + ROW_H
    + GROUP_GAP
    + RULE_H
    + RULE_GAP
    + HEAD_H
    + HEAD_GAP
    + ROPE_H
    + ROPE_GAP
    + NOTE_H
    + GROUP_GAP
    + RULE_H
    + RULE_GAP
    + ITEM_H
    + ITEM_GAP
    + ITEM_H
    + FOOT_GAP
    + BUTTON_H
    + PAD_BOTTOM;

/// Where the rope is tied inside the tile, and how far it may hang. The bottom
/// leaves room for a card to hang from the deepest curve, because a tile whose
/// rope eats its own cards teaches nothing.
const ROPE_RAIL: f32 = 18.0;
const ROPE_TOP: f32 = ROPE_RAIL;
const ROPE_BOTTOM: f32 = ROPE_H - 31.0;
const ROPE_ROOM: f32 = ROPE_BOTTOM - ROPE_TOP;
/// The tile is a hand, not a screen. One logical pixel of pointer travel moves
/// the curve by a tenth of a percent, which is fine enough to land on a value
/// you can name and slow enough that the tile never runs out of rope. The old
/// control mapped pixels to percent 1:1, so it stuck to its own ceiling at 7.8%
/// with half the tile still empty under the cursor.
const SAG_PX_PER_PERCENT: f32 = 10.0;
/// Arrow keys and the wheel move by this; Page Up/Down by a whole percent.
const SAG_STEP_FINE: f32 = 0.1;
const SAG_STEP_COARSE: f32 = 1.0;
/// How far the pointer has to travel before a press becomes a pull. Windows
/// reports a mouse that barely moved, and pulling from that would replace
/// `自动` with a number nobody chose.
const DRAG_START: f32 = 3.0;

const F_TITLE: usize = 0;
const F_NOTE: usize = 1;
const F_HEAD: usize = 2;
const F_BUTTON: usize = 3;

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
    monitor: Monitor,
    fonts: [HFONT; 4],
    layout: Layout,
    /// The panel itself, so an error can be shown against the window the user is
    /// looking at rather than against whichever control happened to be first.
    hwnd: HWND,
    hang: HWND,
    toggle: HWND,
    rope: HWND,
    always: HWND,
    drag_hang: HWND,
    save: HWND,
    cancel: HWND,
    /// The curve as it was when the window opened, for putting it back if the
    /// rope was pulled and then cancelled. May be [`settings::SAG_AUTO`], which
    /// is not a percentage at all, so the tile only shows the shape it equals.
    initial_sag: f32,
    previewed: bool,
    saved: bool,
}

/// Every text band and control slot, in device pixels. `measure` fills it in
/// once; the painter and the child windows read it, so nothing is positioned
/// twice.
#[derive(Clone, Copy, Default)]
struct Layout {
    head_keys: RECT,
    head_line: RECT,
    label_hang: RECT,
    label_toggle: RECT,
    field_hang: RECT,
    field_toggle: RECT,
    rule_a: RECT,
    rule_b: RECT,
    rope: RECT,
    sag_note: RECT,
    always_title: RECT,
    always_note: RECT,
    switch_always: RECT,
    drag_title: RECT,
    drag_note: RECT,
    switch_drag: RECT,
    save: RECT,
    cancel: RECT,
}

fn band(x0: f32, x1: f32, y: f32, h: f32) -> RECT {
    RECT {
        left: x0.round() as i32,
        top: y.round() as i32,
        right: x1.round() as i32,
        bottom: (y + h).round() as i32,
    }
}

/// The one place the rows are laid out, top to bottom. `LOGICAL_H` is this walk
/// with the same constants, so the window is exactly as tall as its content.
#[allow(clippy::field_reassign_with_default)] // each band needs the running `y`
fn measure(client_w: i32, s: f32) -> Layout {
    let x0 = PAD * s;
    let x1 = client_w as f32 - PAD * s;
    // Controls are right-aligned on one vertical line.
    let field_x = x1 - FIELD_W * s;
    let label_x1 = field_x - GAP_LABEL * s;
    let switch_x = x1 - SWITCH_BOX_W * s;
    let mut y = PAD * s;
    let mut l = Layout::default();

    l.head_keys = band(x0, x1, y, HEAD_H * s);
    y += (HEAD_H + HEAD_GAP) * s;
    l.label_hang = band(x0, label_x1, y, ROW_H * s);
    l.field_hang = band(field_x, x1, y + (ROW_H - FIELD_H) / 2.0 * s, FIELD_H * s);
    y += (ROW_H + ROW_GAP) * s;
    l.label_toggle = band(x0, label_x1, y, ROW_H * s);
    l.field_toggle = band(field_x, x1, y + (ROW_H - FIELD_H) / 2.0 * s, FIELD_H * s);
    y += ROW_H * s + GROUP_GAP * s;
    l.rule_a = band(x0, x1, y, RULE_H * s);
    y += (RULE_H + RULE_GAP) * s;

    l.head_line = band(x0, x1, y, HEAD_H * s);
    y += (HEAD_H + HEAD_GAP) * s;
    l.rope = band(x0, x1, y, ROPE_H * s);
    y += (ROPE_H + ROPE_GAP) * s;
    l.sag_note = band(x0, x1, y, NOTE_H * s);
    y += NOTE_H * s + GROUP_GAP * s;
    l.rule_b = band(x0, x1, y, RULE_H * s);
    y += (RULE_H + RULE_GAP) * s;

    l.always_title = band(x0, label_x1, y, TITLE_H * s);
    l.always_note = band(x0, x1, y + TITLE_H * s, NOTE_H * s);
    l.switch_always = band(
        switch_x,
        x1,
        y + (TITLE_H - SWITCH_BOX_H) / 2.0 * s,
        SWITCH_BOX_H * s,
    );
    y += (ITEM_H + ITEM_GAP) * s;
    l.drag_title = band(x0, label_x1, y, TITLE_H * s);
    l.drag_note = band(x0, x1, y + TITLE_H * s, NOTE_H * s);
    l.switch_drag = band(
        switch_x,
        x1,
        y + (TITLE_H - SWITCH_BOX_H) / 2.0 * s,
        SWITCH_BOX_H * s,
    );
    y += ITEM_H * s + FOOT_GAP * s;

    let w = BUTTON_W * s;
    l.save = band(x1 - w, x1, y, BUTTON_H * s);
    l.cancel = band(
        x1 - w - BUTTON_GAP * s - w,
        x1 - w - BUTTON_GAP * s,
        y,
        BUTTON_H * s,
    );
    l
}

// --- colour helpers -------------------------------------------------------------

/// GDI keeps its colours blue first.
const fn rgb(c: (u8, u8, u8)) -> COLORREF {
    COLORREF(c.0 as u32 | ((c.1 as u32) << 8) | ((c.2 as u32) << 16))
}

fn solid(c: (u8, u8, u8)) -> HBRUSH {
    unsafe { CreateSolidBrush(rgb(c)) }
}

fn paint(c: (u8, u8, u8), a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, a);
    p.anti_alias = true;
    p
}

/// Rounded rectangles everywhere agree on the radius, in logical pixels: the
/// tile, the buttons and the switch track are the same family of shape.
const RADIUS: f32 = 8.0;

// --- registration ---------------------------------------------------------------

pub fn register_class(hinst: HINSTANCE, big: HICON, small: HICON) {
    unsafe {
        // No common control is created here, so no InitCommonControlsEx: the
        // panel is five classes of our own.
        register(CLASS, hinst, big, small, Some(wndproc), WNDCLASS_STYLES(0));
        register(
            SWITCH_CLASS,
            hinst,
            big,
            small,
            Some(switch_wndproc),
            WNDCLASS_STYLES(0),
        );
        // Only the tile cares about a second click: double-click is how you put
        // the curve back to automatic.
        register(
            ROPE_CLASS,
            hinst,
            big,
            small,
            Some(rope_wndproc),
            CS_DBLCLKS,
        );
        register(
            KEY_CLASS,
            hinst,
            big,
            small,
            Some(key_wndproc),
            WNDCLASS_STYLES(0),
        );
        register(
            BUTTON_CLASS,
            hinst,
            big,
            small,
            Some(button_wndproc),
            WNDCLASS_STYLES(0),
        );
    }
}

unsafe fn register(
    name: PCWSTR,
    hinst: HINSTANCE,
    big: HICON,
    small: HICON,
    proc: WNDPROC,
    style: WNDCLASS_STYLES,
) {
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style,
            lpfnWndProc: proc,
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big,
            hIconSm: small,
            // Every window here paints its own background; the class brush is
            // only what would show for the instant before the first WM_PAINT.
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut c_void),
            lpszClassName: name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&class);
    }
}

/// The percentage that reproduces the shipped curve on this monitor. The tile
/// draws that when the pick is `Auto`, because a flat line would teach nothing
/// about what `自动` actually looks like.
fn sag_auto_on(monitor: &Monitor) -> f32 {
    let width = monitor.bounds.width().max(1) as f32;
    shaiwin_core::layout::classic_sag(width, monitor.scale) / width * 100.0
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

        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let ptr = Box::into_raw(Box::new(State {
            app,
            window_icon,
            tray_icon,
            scale,
            monitor,
            fonts: make_fonts(scale),
            layout: measure(client.right, scale),
            hwnd,
            hang: HWND::default(),
            toggle: HWND::default(),
            rope: HWND::default(),
            always: HWND::default(),
            drag_hang: HWND::default(),
            save: HWND::default(),
            cancel: HWND::default(),
            initial_sag: settings.sag_percent,
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
            let l = self.layout;
            // Tab order follows the reading order, so the children are created
            // top to bottom: Windows walks them in creation order.
            self.hang = self.key(
                hwnd,
                l.field_hang,
                HANG_ID,
                settings.hang_mods,
                settings.hang_vk,
            );
            self.toggle = self.key(
                hwnd,
                l.field_toggle,
                TOGGLE_ID,
                settings.toggle_mods,
                settings.toggle_vk,
            );
            self.rope = self.rope(hwnd, l.rope, settings);
            let reduced = settings.reduced_motion;
            self.always = self.switch(
                hwnd,
                l.switch_always,
                ALWAYS_ID,
                settings.always_on,
                reduced,
            );
            self.drag_hang = self.switch(hwnd, l.switch_drag, DRAG_ID, settings.drag_hang, reduced);
            self.save = self.button(hwnd, t("settings_save"), SAVE_ID, l.save, true);
            self.cancel = self.button(hwnd, t("settings_cancel"), CANCEL_ID, l.cancel, false);

            if !self.window_icon.is_invalid() {
                let _ = send(
                    hwnd,
                    WM_SETICON,
                    ICON_BIG as usize,
                    self.window_icon.0 as isize,
                );
            }
            if !self.tray_icon.is_invalid() {
                let _ = send(
                    hwnd,
                    WM_SETICON,
                    ICON_SMALL as usize,
                    self.tray_icon.0 as isize,
                );
            }
        }
    }

    unsafe fn child(
        &self,
        parent: HWND,
        class: PCWSTR,
        rect: RECT,
        id: i32,
        style: WINDOW_STYLE,
    ) -> HWND {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                Some(parent),
                Some(HMENU(id as usize as *mut c_void)),
                None,
                None,
            )
            .unwrap_or_default()
        }
    }

    unsafe fn rope(&mut self, parent: HWND, rect: RECT, settings: &settings::Settings) -> HWND {
        unsafe {
            let hwnd = self.child(parent, ROPE_CLASS, rect, ROPE_ID, WINDOW_STYLE(0));
            let shown = settings.sag_percent.clamp(0.0, SAG_PERCENT_MAX);
            let ctrl = Box::into_raw(Box::new(Rope {
                hwnd,
                state: self as *mut State,
                pick: if settings.sag_is_auto() {
                    Pick::Auto
                } else {
                    Pick::Picked(shown)
                },
                auto: sag_auto_on(&self.monitor),
                dragging: false,
                press: 0.0,
                grab: 0.0,
                focused: false,
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctrl as isize);
            hwnd
        }
    }

    unsafe fn switch(
        &mut self,
        parent: HWND,
        rect: RECT,
        id: i32,
        on: bool,
        reduced: bool,
    ) -> HWND {
        unsafe {
            let hwnd = self.child(parent, SWITCH_CLASS, rect, id, WINDOW_STYLE(0));
            let ctrl = Box::into_raw(Box::new(Switch {
                hwnd,
                state: self as *mut State,
                on,
                pressed: false,
                hover: false,
                focused: false,
                anim: if on { 1.0 } else { 0.0 },
                reduced,
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctrl as isize);
            hwnd
        }
    }

    unsafe fn key(&mut self, parent: HWND, rect: RECT, id: i32, mods: u32, vk: u32) -> HWND {
        unsafe {
            let hwnd = self.child(parent, KEY_CLASS, rect, id, WINDOW_STYLE(0));
            let ctrl = Box::into_raw(Box::new(Key {
                hwnd,
                state: self as *mut State,
                mods,
                vk,
                capturing: false,
                pending: 0,
                focused: false,
                before: (mods, vk),
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctrl as isize);
            hwnd
        }
    }

    unsafe fn button(
        &mut self,
        parent: HWND,
        text: &str,
        id: i32,
        rect: RECT,
        primary: bool,
    ) -> HWND {
        unsafe {
            let hwnd = self.child(parent, BUTTON_CLASS, rect, id, WINDOW_STYLE(0));
            let ctrl = Box::into_raw(Box::new(Button {
                hwnd,
                parent,
                state: self as *mut State,
                id,
                text: text.to_string(),
                primary,
                pressed: false,
                focused: false,
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctrl as isize);
            hwnd
        }
    }

    // --- painting the panel -----------------------------------------------------

    unsafe fn paint(&self, hwnd: HWND) {
        unsafe {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.is_invalid() {
                return;
            }
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let paper = solid(PAPER);
            FillRect(hdc, &client, paper);
            let rule = solid(RULE_COLOUR);
            FillRect(hdc, &self.layout.rule_a, rule);
            FillRect(hdc, &self.layout.rule_b, rule);

            let l = self.layout;
            self.text(
                hdc,
                t("settings_group_keys"),
                F_HEAD,
                MUTED,
                l.head_keys,
                DT_TOP | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_group_line"),
                F_HEAD,
                MUTED,
                l.head_line,
                DT_TOP | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_hang"),
                F_TITLE,
                INK,
                l.label_hang,
                DT_VCENTER | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_toggle"),
                F_TITLE,
                INK,
                l.label_toggle,
                DT_VCENTER | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_sag_note"),
                F_NOTE,
                MUTED,
                l.sag_note,
                DT_TOP | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_always"),
                F_TITLE,
                INK,
                l.always_title,
                DT_TOP | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_always_note"),
                F_NOTE,
                MUTED,
                l.always_note,
                DT_TOP | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_drag"),
                F_TITLE,
                INK,
                l.drag_title,
                DT_TOP | DT_LEFT,
            );
            self.text(
                hdc,
                t("settings_drag_note"),
                F_NOTE,
                MUTED,
                l.drag_note,
                DT_TOP | DT_LEFT,
            );

            let _ = DeleteObject(paper.into());
            let _ = DeleteObject(rule.into());
            let _ = EndPaint(hwnd, &ps);
        }
    }

    unsafe fn text(
        &self,
        hdc: HDC,
        text: &str,
        font: usize,
        colour: (u8, u8, u8),
        rect: RECT,
        align: DRAW_TEXT_FORMAT,
    ) {
        unsafe {
            if text.is_empty() || rect.right <= rect.left {
                return;
            }
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            let old_font = SelectObject(hdc, HGDIOBJ(self.fonts[font].0));
            let _ = SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, rgb(colour));
            let mut r = rect;
            DrawTextW(
                hdc,
                &mut wide,
                &mut r,
                align | DT_NOPREFIX | DT_END_ELLIPSIS | DT_SINGLELINE,
            );
            let _ = SelectObject(hdc, old_font);
        }
    }

    // --- results ----------------------------------------------------------------

    fn checked(&self, hwnd: HWND) -> bool {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Switch;
            !ptr.is_null() && (*ptr).on
        }
    }

    /// The combination a shortcut box is holding, whether or not the user ever
    /// clicked it. A box still listening when Save is pressed has not chosen
    /// anything, so it reports the value it came in with.
    fn key_mods_vk(&self, hwnd: HWND) -> (u32, u32) {
        unsafe {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Key;
            if ptr.is_null() {
                return (0, 0);
            }
            ((*ptr).mods, (*ptr).vk)
        }
    }

    /// Tab and Shift-Tab out of a shortcut box. While a box is listening it takes
    /// every key, so the dialog manager never gets the Tab that ends the listening.
    fn focus_stop(&self, current: HWND, backward: bool) {
        let stops = [
            self.hang,
            self.toggle,
            self.rope,
            self.always,
            self.drag_hang,
            self.save,
            self.cancel,
        ];
        let Some(at) = stops.iter().position(|stop| *stop == current) else {
            return;
        };
        let next = if backward {
            stops[(at + stops.len() - 1) % stops.len()]
        } else {
            stops[(at + 1) % stops.len()]
        };
        if !next.is_invalid() {
            unsafe {
                let _ = SetFocus(Some(next));
            }
        }
    }

    /// The curve the tile is showing. `自动` reports itself as `SAG_AUTO` rather
    /// than as the percentage that happens to match it on this monitor, so
    /// opening the panel and saving it cannot quietly freeze the shipped curve.
    fn sag(&self) -> f32 {
        unsafe {
            let ptr = GetWindowLongPtrW(self.rope, GWLP_USERDATA) as *mut Rope;
            if ptr.is_null() {
                return self.initial_sag;
            }
            (*ptr).pick.value()
        }
    }

    unsafe fn save(&mut self) -> bool {
        unsafe {
            let (hang_mods, hang_vk) = self.key_mods_vk(self.hang);
            let (toggle_mods, toggle_vk) = self.key_mods_vk(self.toggle);
            let bare = |vk: u32| vk != 0 && !(0x70..=0x87).contains(&vk);
            if (hang_vk != 0 && hang_mods == 0 && bare(hang_vk))
                || (toggle_vk != 0 && toggle_mods == 0 && bare(toggle_vk))
            {
                message(self.hwnd, t("settings_no_mods"));
                return false;
            }
            let sag = self.sag();
            match (*self.app).apply_settings(
                (hang_mods, hang_vk),
                (toggle_mods, toggle_vk),
                sag,
                self.checked(self.always),
                // The switch reads "拖到屏幕顶端就挂上", which is the stored
                // value itself: what you see is what the setting says.
                self.checked(self.drag_hang),
            ) {
                Ok(()) => {
                    self.saved = true;
                    true
                }
                Err(error) => {
                    message(self.hwnd, &error);
                    false
                }
            }
        }
    }

    unsafe fn preview(&mut self, percent: f32) {
        unsafe {
            self.previewed = true;
            (*self.app).preview_sag(percent);
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
                if id == SAVE_ID && notification == BN_CLICKED {
                    if state.save() {
                        let _ = DestroyWindow(hwnd);
                    }
                    return LRESULT(0);
                }
                // Escape reaches the parent as a bare IDCANCEL, with no click
                // notification at all, so both spellings mean the same thing.
                if id == CANCEL_ID && (notification == BN_CLICKED || notification == 0) {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                LRESULT(0)
            }
            DM_GETDEFID => {
                // Enter looks for the default button by asking the parent this.
                // The high word says it is live; the low word is the control id,
                // which is also how the click comes back to WM_COMMAND above.
                LRESULT(SAVE_ID as isize | 0x8000)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                state.paint(hwnd);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                SETTINGS_HWND.store(0, Ordering::Relaxed);
                // A rope pulled and never saved was only a preview.
                if state.previewed && !state.saved {
                    (*state.app).preview_sag(state.initial_sag);
                }
                (*state.app).settings_window_closed();
                for font in state.fonts.iter() {
                    if !font.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                // Windows hands the wheel to the focused window, which is rarely
                // the one under the cursor. Over the tile that would mean the
                // wheel only worked after a click, so the panel routes it itself.
                let mut over = false;
                let mut pt = POINT::default();
                let mut bounds = RECT::default();
                if !state.rope.is_invalid()
                    && GetCursorPos(&mut pt).is_ok()
                    && GetWindowRect(state.rope, &mut bounds).is_ok()
                {
                    over = pt.x >= bounds.left
                        && pt.x < bounds.right
                        && pt.y >= bounds.top
                        && pt.y < bounds.bottom;
                }
                if over {
                    SendMessageW(state.rope, WM_MOUSEWHEEL, Some(wp), Some(lp));
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// --- the switch ------------------------------------------------------------------

/// The on/off control. A checkbox is a square with a tick in it, which reads as
/// "a thing on a list"; this is a state of the app, and a track with a knob on
/// one side or the other says which state in one glance.
///
/// Every pixel of it is rasterised by tiny-skia, the same as the rope tile. GDI's
/// `Ellipse` has no anti-aliasing, and a stair-stepped circle sitting next to a
/// smooth one is most of what made the old switch look cheap.
struct Switch {
    hwnd: HWND,
    state: *mut State,
    on: bool,
    pressed: bool,
    hover: bool,
    focused: bool,
    /// Knob position: 0 is the left, 1 the right. It slides, because a control
    /// that teleports does not tell you which side it just moved to - unless the
    /// panel was opened with `reduced_motion`, in which case it teleports.
    anim: f32,
    reduced: bool,
}

/// One timer per switch, and only while the knob is moving.
const SWITCH_TIMER: usize = 0x5f_11;

unsafe extern "system" fn switch_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Switch;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ctrl = &mut *ptr;
        match msg {
            WM_PAINT => {
                ctrl.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_GETDLGCODE => LRESULT((DLGC_BUTTON | DLGC_WANTCHARS) as isize),
            WM_LBUTTONDOWN => {
                let _ = SetFocus(Some(hwnd));
                ctrl.pressed = true;
                let _ = SetCapture(hwnd);
                ctrl.refresh();
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                ctrl.pressed = false;
                if hit(lp, hwnd) {
                    ctrl.flip();
                }
                ctrl.refresh();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if !ctrl.hover {
                    ctrl.hover = true;
                    ctrl.refresh();
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                ctrl.hover = false;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_TIMER => {
                if wp.0 == SWITCH_TIMER && !ctrl.slide() {
                    let _ = KillTimer(Some(hwnd), SWITCH_TIMER);
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if wp.0 as u32 == VK_SPACE.0 as u32 {
                    ctrl.flip();
                    ctrl.refresh();
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_SETFOCUS => {
                ctrl.focused = true;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                ctrl.focused = false;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let _ = KillTimer(Some(hwnd), SWITCH_TIMER);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn hit(lp: LPARAM, hwnd: HWND) -> bool {
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        let x = lp.0 as i16 as i32;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
        x >= 0 && x < r.right && y >= 0 && y < r.bottom
    }
}

impl Switch {
    fn refresh(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// The only thing that turns the state into a new value: everything else in
    /// this file ends up here, so a click, a space bar and a `Save` that re-reads
    /// the settings all move the knob the same way.
    fn flip(&mut self) {
        self.on = !self.on;
        if self.reduced {
            self.anim = if self.on { 1.0 } else { 0.0 };
            self.refresh();
            return;
        }
        unsafe {
            let _ = SetTimer(Some(self.hwnd), SWITCH_TIMER, 16, None);
        }
    }

    /// One frame of the slide. `false` means it has arrived.
    fn slide(&mut self) -> bool {
        let target = if self.on { 1.0 } else { 0.0 };
        let d = target - self.anim;
        if d.abs() < 0.02 {
            self.anim = target;
            self.refresh();
            return false;
        }
        // Exponential, not linear: it starts with the click and settles, which is
        // what a thing with mass does.
        self.anim += d * 0.35;
        self.refresh();
        true
    }

    unsafe fn paint(&self) {
        unsafe {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if hdc.is_invalid() {
                return;
            }
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let scale = (*self.state).scale;
            let paper = solid(PAPER);
            FillRect(hdc, &r, paper);
            let _ = DeleteObject(paper.into());

            let w = (r.right - r.left).max(0) as u32;
            let h = (r.bottom - r.top).max(0) as u32;
            if let Some((bytes, bw, bh)) = switch_art(
                w,
                h,
                scale,
                self.anim,
                self.pressed,
                self.hover,
                self.focused,
            ) {
                blit(hdc, 0, 0, &bytes, bw, bh);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }
}

/// The switch, anti-aliased. The track carries the state and the knob stays the
/// same object on either end of it, so `on` and `off` weigh the same; the old
/// one swapped a pale pill for a block of ink, which made the enabled rows shout.
fn switch_art(
    w: u32,
    h: u32,
    scale: f32,
    anim: f32,
    pressed: bool,
    hover: bool,
    focused: bool,
) -> Option<(Vec<u8>, u32, u32)> {
    let mut pix = Pixmap::new(w, h)?;
    pix.fill(Color::TRANSPARENT);
    let s = scale;
    let tw = SWITCH_W * s;
    let th = SWITCH_H * s;
    // The track hugs the right edge of the box so it lines up with the field
    // column; the padding is all on the side the label is on.
    let right = w as f32;
    let left = (right - tw).max(0.0);
    let top = (h as f32 - th) / 2.0;
    let inset = 0.5 * s;

    let fill = mix(SWITCH_OFF, FLAX, anim.clamp(0.0, 1.0));
    let edge = mix(fill, INK, if hover || focused { 0.45 } else { 0.18 });
    let track = rrect(
        left + inset,
        top + inset,
        tw - inset * 2.0,
        th - inset * 2.0,
        th / 2.0 - inset,
    )?;
    pix.fill_path(
        &track,
        &paint(fill, if pressed { 235 } else { 255 }),
        tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    stroke(
        &mut pix,
        &track,
        paint(edge, 255),
        1.0 * s,
        Transform::identity(),
    );

    let radius = (th / 2.0 - 3.0 * s) * if pressed { 0.88 } else { 1.0 };
    let cx = left + th / 2.0 + anim * (tw - th);
    let cy = top + th / 2.0;
    let knob = circle_path(cx, cy, radius)?;
    pix.fill_path(
        &knob,
        &paint(FIELD, 255),
        tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    stroke(
        &mut pix,
        &knob,
        paint((0, 0, 0), 36),
        1.0 * s,
        Transform::identity(),
    );

    if focused {
        let o = 2.5 * s;
        if let Some(ring) = rrect(left - o, top - o, tw + o * 2.0, th + o * 2.0, th / 2.0 + o) {
            stroke(
                &mut pix,
                &ring,
                paint(FLAX, 255),
                1.4 * s,
                Transform::identity(),
            );
        }
    }

    Some((pix.data().to_vec(), w, h))
}

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let at = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (at(a.0, b.0), at(a.1, b.1), at(a.2, b.2))
}

// --- the rope ---------------------------------------------------------------------

/// The tile you pull: a miniature of the line, with the knob on it so the bend
/// has something to move.
struct Rope {
    hwnd: HWND,
    state: *mut State,
    pick: Pick,
    /// What `自动` resolves to on this monitor, so the tile can show that curve
    /// instead of a flat line.
    auto: f32,
    dragging: bool,
    /// Where the pointer was when the button went down, and the offset that
    /// makes the pull relative to it rather than an absolute jump. Both are in
    /// logical pixels of the tile.
    press: f32,
    grab: f32,
    focused: bool,
}

/// What the tile is showing. `Auto` is a choice, not the absence of one: it is
/// the curve the app picks from the width of the monitor, and the tile has to be
/// able to get back to it after the user has pulled it somewhere else.
#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Auto,
    Picked(f32),
}

impl Pick {
    /// The number that goes into the settings file.
    fn value(self) -> f32 {
        match self {
            Pick::Auto => settings::SAG_AUTO,
            Pick::Picked(percent) => percent,
        }
    }
}

unsafe extern "system" fn rope_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Rope;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ctrl = &mut *ptr;
        let scale = (*ctrl.state).scale;
        // The pointer messages pack x in the low word of lParam and y above it.
        let y_of = |lp: LPARAM| ((lp.0 >> 16) as i16 as f32) / scale;
        match msg {
            WM_PAINT => {
                ctrl.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_GETDLGCODE => LRESULT((DLGC_BUTTON | DLGC_WANTARROWS | DLGC_WANTCHARS) as isize),
            WM_LBUTTONDOWN => {
                let _ = SetFocus(Some(hwnd));
                ctrl.dragging = true;
                ctrl.press = y_of(lp);
                ctrl.grab = ctrl.press - ctrl.shown() * SAG_PX_PER_PERCENT;
                let _ = SetCapture(hwnd);
                ctrl.refresh();
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                // Two clicks is the only gesture that does not fight the drag.
                ctrl.choose_auto();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // A click is how you give the tile focus, and a real mouse
                // wobbles a pixel or two while you make it, so the pull only
                // starts once the pointer has clearly gone somewhere.
                if ctrl.dragging && (y_of(lp) - ctrl.press).abs() >= DRAG_START {
                    ctrl.pull(pulled(ctrl.grab, y_of(lp)));
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                // The last move already committed the curve; releasing only ends
                // the drag and re-arms the readout that the drag was holding up.
                if ctrl.dragging {
                    ctrl.dragging = false;
                    let _ = ReleaseCapture();
                    ctrl.refresh();
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                // A notch is the same distance as a key press; Shift is the
                // coarse gear for anybody who wants to cover the range quickly.
                let notch = (wp.0 >> 16) as i16 as f32 / WHEEL_NOTCH;
                let step = if GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 {
                    SAG_STEP_COARSE / 2.0
                } else {
                    SAG_STEP_FINE
                };
                ctrl.nudge(notch * step);
                LRESULT(0)
            }
            WM_TIMER => {
                // The tile has no timer of its own; see `Rope::settled`.
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_KEYDOWN => {
                let key = wp.0 as u32;
                if key == VK_LEFT.0 as u32 || key == VK_UP.0 as u32 {
                    ctrl.nudge(-SAG_STEP_FINE);
                    LRESULT(0)
                } else if key == VK_RIGHT.0 as u32 || key == VK_DOWN.0 as u32 {
                    ctrl.nudge(SAG_STEP_FINE);
                    LRESULT(0)
                } else if key == VK_PRIOR.0 as u32 {
                    ctrl.nudge(-SAG_STEP_COARSE);
                    LRESULT(0)
                } else if key == VK_NEXT.0 as u32 {
                    ctrl.nudge(SAG_STEP_COARSE);
                    LRESULT(0)
                } else if key == VK_HOME.0 as u32 {
                    ctrl.pull(0.0);
                    LRESULT(0)
                } else if key == VK_END.0 as u32 {
                    ctrl.pull(SAG_PERCENT_MAX);
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_SETFOCUS => {
                ctrl.focused = true;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                ctrl.focused = false;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// How far the rope hangs inside the tile, in logical pixels. The whole range the
/// line understands is spread over the tile's travel: the tile is a preview of
/// the shape, and the number above it is the precision.
fn rail_sag(percent: f32) -> f32 {
    (percent / SAG_PERCENT_MAX * ROPE_ROOM).clamp(0.0, ROPE_ROOM)
}

/// Where a pull points. `grab` is the pointer position that would mean the curve
/// it was taken from, so the press point itself is a no move; `Rope::pull` is the
/// one that clamps to what the line can hold.
fn pulled(grab: f32, y: f32) -> f32 {
    (y - grab) / SAG_PX_PER_PERCENT
}

/// One wheel notch, in the units Windows reports it in.
const WHEEL_NOTCH: f32 = 120.0;

impl Rope {
    /// The curve on screen right now, whether or not the user chose it.
    fn shown(&self) -> f32 {
        match self.pick {
            Pick::Auto => self.auto,
            Pick::Picked(percent) => percent,
        }
    }

    /// The pointer took the rope somewhere. Relative to where it was grabbed, so
    /// the curve cannot hit a wall under the cursor.
    unsafe fn pull(&mut self, percent: f32) {
        self.pick = Pick::Picked(percent.clamp(0.0, SAG_PERCENT_MAX));
        self.commit();
    }

    unsafe fn nudge(&mut self, delta: f32) {
        self.pull(self.shown() + delta);
    }

    unsafe fn choose_auto(&mut self) {
        self.pick = Pick::Auto;
        self.refresh();
        unsafe {
            (*self.state).preview(settings::SAG_AUTO);
        }
    }

    unsafe fn commit(&mut self) {
        self.refresh();
        unsafe {
            (*self.state).preview(self.shown());
        }
    }

    /// How loud the number is: ink while the hand or the keyboard is on the
    /// tile, the panel's grey the moment it is not. This used to be a fade
    /// driven by a 60 ms timer, and while the tile held focus the timer never
    /// stopped - the whole panel flashed for the rest of the session. A readout
    /// that costs repaints to look polite is not worth the repaints.
    fn settled(&self) -> f32 {
        if self.dragging || self.focused {
            0.0
        } else {
            1.0
        }
    }

    fn refresh(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    unsafe fn paint(&self) {
        unsafe {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if hdc.is_invalid() {
                return;
            }
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let scale = (*self.state).scale;

            let field = solid(FIELD);
            let edge = CreatePen(PS_SOLID, 1, rgb(EDGE));
            let old_brush = SelectObject(hdc, field.into());
            let old_pen = SelectObject(hdc, edge.into());
            let radius = (RADIUS * 2.0 * scale) as i32;
            let _ = RoundRect(hdc, 0, 0, r.right, r.bottom, radius, radius);
            let _ = SelectObject(hdc, old_brush);
            let _ = SelectObject(hdc, old_pen);
            let _ = DeleteObject(field.into());
            let _ = DeleteObject(edge.into());

            let live = self.dragging || self.focused;
            let w = (r.right - r.left).max(0) as u32;
            let h = (r.bottom - r.top).max(0) as u32;
            if let Some((bytes, bw, bh)) = rope_art(w, h, scale, rail_sag(self.shown()), live) {
                blit(hdc, 0, 0, &bytes, bw, bh);
            }
            // The number is the precision; the rope is only the shape. While the
            // hand is on it the number is ink, and when the hand leaves it walks
            // down to the muted grey the rest of the panel uses.
            let shown = self.shown();
            let label = match self.pick {
                Pick::Auto => format!("{} {:.1}%", t("settings_auto"), shown),
                Pick::Picked(_) => format!("{:.1}%", shown),
            };
            let s = scale;
            let rect = RECT {
                left: (10.0 * s).round() as i32,
                top: (h as f32 - 26.0 * s).round() as i32,
                right: (w as f32 - 10.0 * s).round() as i32,
                bottom: (h as f32 - 6.0 * s).round() as i32,
            };
            let ink = mix(INK, MUTED, self.settled());
            (*self.state).text(hdc, &label, F_NOTE, ink, rect, DT_RIGHT | DT_VCENTER);
            if self.focused {
                focus_ring(hdc, &r, radius, scale);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }
}

/// A miniature of the line: two pegs, the rope between them, and the knob that
/// pulls it. Drawn at the tile's device size, then blitted over the rounded field.
fn rope_art(
    w: u32,
    h: u32,
    scale: f32,
    sag_logical: f32,
    live: bool,
) -> Option<(Vec<u8>, u32, u32)> {
    let mut pix = Pixmap::new(w, h)?;
    pix.fill(Color::TRANSPARENT);
    let s = scale;
    let inset = 18.0 * s;
    let x0 = inset.min(w as f32 * 0.1);
    let x1 = (w as f32 - inset).max(x0 + 1.0);
    let rail = ROPE_RAIL * s;
    let sag = sag_logical * s;
    let rope = {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, rail);
        pb.quad_to((x0 + x1) / 2.0, rail + 2.0 * sag, x1, rail);
        pb.finish()?
    };
    stroke(
        &mut pix,
        &rope,
        paint((0, 0, 0), 46),
        3.0 * s,
        Transform::from_translate(0.0, 1.2 * s),
    );
    let core = if live { FLAX } else { ROPE_IDLE };
    stroke(
        &mut pix,
        &rope,
        paint(core, 255),
        2.0 * s,
        Transform::identity(),
    );
    stroke(
        &mut pix,
        &rope,
        paint((0xFF, 0xFF, 0xFF), 128),
        0.7 * s,
        Transform::from_translate(0.0, -0.6 * s),
    );

    // The two places the rope is tied to.
    for x in [x1, x0] {
        if let Some(circle) = circle_path(x, rail, 2.4 * s) {
            pix.fill_path(
                &circle,
                &paint(if live { FLAX } else { ROPE_IDLE }, 255),
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    // The knob is the answer to "how do I change this?" without a word: a flat
    // rope on its own looks like a picture of a rope, and a handle in the middle
    // of it looks like something you pull.
    let kx = (x0 + x1) / 2.0;
    let ky = rail + sag;
    if let Some(shadow) = circle_path(kx, ky + 1.0 * s, 5.4 * s) {
        pix.fill_path(
            &shadow,
            &paint((0, 0, 0), 28),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(body) = circle_path(kx, ky, 5.0 * s) {
        pix.fill_path(
            &body,
            &paint(FIELD, 255),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
        stroke(
            &mut pix,
            &body,
            paint(if live { FLAX } else { ROPE_IDLE }, 255),
            1.4 * s,
            Transform::identity(),
        );
    }
    for dy in [-1.5 * s, 1.5 * s] {
        let mut grip = PathBuilder::new();
        grip.move_to(kx - 2.0 * s, ky + dy);
        grip.line_to(kx + 2.0 * s, ky + dy);
        if let Some(path) = grip.finish() {
            stroke(
                &mut pix,
                &path,
                paint(if live { FLAX } else { ROPE_IDLE }, 255),
                1.0 * s,
                Transform::identity(),
            );
        }
    }

    Some((pix.data().to_vec(), w, h))
}

fn stroke(
    canvas: &mut Pixmap,
    path: &tiny_skia::Path,
    colour: Paint<'static>,
    width: f32,
    transform: Transform,
) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        ..Default::default()
    };
    canvas.stroke_path(path, &colour, &stroke, transform, None);
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w, y, x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h, x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x, y + h, x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y, x, y, x + r, y);
    pb.close();
    pb.finish()
}

fn circle_path(cx: f32, cy: f32, r: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let k = 0.552_284_7 * r;
    pb.move_to(cx - r, cy);
    pb.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    pb.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    pb.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    pb.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    pb.close();
    pb.finish()
}

/// Copy a premultiplied RGBA raster onto the window. GDI's 32-bit DIB sections
/// are blue-first, so the pairs swap on the way in.
unsafe fn blit(hdc: HDC, x: i32, y: i32, pixels: &[u8], w: u32, h: u32) {
    unsafe {
        if w == 0 || h == 0 || pixels.len() < (w as usize * h as usize * 4) {
            return;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            return;
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            return;
        }
        let dest = bits as *mut u8;
        let mut buffer = pixels.to_vec();
        for chunk in buffer.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        std::ptr::copy_nonoverlapping(buffer.as_ptr(), dest, buffer.len());
        let mem = CreateCompatibleDC(Some(hdc));
        let old = SelectObject(mem, bitmap.into());
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = AlphaBlend(
            hdc, x, y, w as i32, h as i32, mem, 0, 0, w as i32, h as i32, blend,
        );
        let _ = SelectObject(mem, old);
        let _ = DeleteDC(mem);
        let _ = DeleteObject(bitmap.into());
    }
}

/// The keyboard's answer to a hover: the same warm line the rope uses, on the
/// shape the control already has. It goes inside the edge, not outside it — a
/// control's window is a rectangle, so a ring drawn beyond its rounded plate
/// lands in the corner notches and reads as damage rather than focus.
unsafe fn focus_ring(hdc: HDC, r: &RECT, radius: i32, scale: f32) {
    unsafe {
        let pen = CreatePen(PS_SOLID, 1.max((1.4 * scale) as i32), rgb(FLAX));
        let old_pen = SelectObject(hdc, pen.into());
        let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let o = (2.0 * scale).round() as i32;
        let _ = RoundRect(
            hdc,
            o,
            o,
            r.right - o,
            r.bottom - o,
            (radius - o * 2).max(1),
            (radius - o * 2).max(1),
        );
        let _ = SelectObject(hdc, old_pen);
        let _ = SelectObject(hdc, old_brush);
        let _ = DeleteObject(pen.into());
    }
}

// --- the shortcut box ---------------------------------------------------------------

/// A combination the app can register as a hotkey. The box is only listening
/// while it is showing the keys you are holding; anything else is the value that
/// was already there, so clicking it and walking away changes nothing.
struct Key {
    hwnd: HWND,
    state: *mut State,
    mods: u32,
    vk: u32,
    capturing: bool,
    /// The modifiers held at this moment while listening.
    pending: u32,
    focused: bool,
    /// What was in the box when it started listening, for Escape and Tab.
    before: (u32, u32),
}

unsafe extern "system" fn key_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Key;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ctrl = &mut *ptr;
        match msg {
            WM_PAINT => {
                ctrl.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_GETDLGCODE => LRESULT(if ctrl.capturing {
                // Everything, including the keys the dialog manager would
                // otherwise spend on navigation - while this box is open,
                // Ctrl+Right is a candidate shortcut, not a way out.
                DLGC_WANTALLKEYS | DLGC_WANTARROWS | DLGC_WANTCHARS
            } else {
                DLGC_BUTTON | DLGC_WANTCHARS
            } as isize),
            // A focused button is clicked by the space bar by whoever runs the
            // dialog manager, so the keyboard and the mouse come through here.
            BM_CLICK => {
                ctrl.begin();
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let _ = SetFocus(Some(hwnd));
                if ctrl.vk != 0 && ctrl.clear_hit(lp) {
                    // The × is the whole of "how do I switch this shortcut off".
                    ctrl.mods = 0;
                    ctrl.vk = 0;
                    ctrl.leave();
                } else if !ctrl.capturing {
                    ctrl.begin();
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let key = wp.0 as u32;
                if ctrl.capturing {
                    ctrl.key_down(key);
                    LRESULT(0)
                } else if key == VK_SPACE.0 as u32 {
                    ctrl.begin();
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_KEYUP => {
                // Letting go of Ctrl takes the "Ctrl …" back off the box.
                if ctrl.capturing && is_modifier(wp.0 as u32) {
                    ctrl.pending = held_mods();
                    ctrl.refresh();
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_SETFOCUS => {
                ctrl.focused = true;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                ctrl.focused = false;
                if !ctrl.capturing {
                    ctrl.refresh();
                } else {
                    // Clicking away is not choosing a shortcut.
                    ctrl.restore();
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

impl Key {
    /// Start listening. The modifiers already under the fingers are shown at
    /// once, so a click held through Ctrl reads as "Ctrl, and then?".
    unsafe fn begin(&mut self) {
        self.before = (self.mods, self.vk);
        self.capturing = true;
        self.pending = held_mods();
        self.refresh();
    }

    fn leave(&mut self) {
        self.capturing = false;
        self.pending = 0;
        self.refresh();
    }

    fn restore(&mut self) {
        let (mods, vk) = self.before;
        self.mods = mods;
        self.vk = vk;
        self.leave();
    }

    unsafe fn key_down(&mut self, key: u32) {
        if key == VK_ESCAPE.0 as u32 {
            self.restore();
        } else if key == VK_TAB.0 as u32 {
            // Tab stays navigation, so a box never traps the keyboard; the price
            // is that Ctrl+Tab cannot be recorded here.
            let backward = held_mods() & settings::MOD_SHIFT != 0;
            self.restore();
            (*self.state).focus_stop(self.hwnd, backward);
        } else if key == VK_BACK.0 as u32 || key == VK_DELETE.0 as u32 {
            self.mods = 0;
            self.vk = 0;
            self.leave();
        } else if is_modifier(key) {
            self.pending = held_mods();
            self.refresh();
        } else {
            self.mods = held_mods();
            self.vk = key;
            self.leave();
        }
    }

    /// The × band at the right end of the field.
    unsafe fn clear_hit(&self, lp: LPARAM) -> bool {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let x = lp.0 as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            x >= r.right - (CLEAR_W * (*self.state).scale).round() as i32
                && y >= r.top
                && y < r.bottom
        }
    }

    fn refresh(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    unsafe fn paint(&self) {
        unsafe {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if hdc.is_invalid() {
                return;
            }
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let scale = (*self.state).scale;

            let field = solid(FIELD);
            let edge = CreatePen(
                PS_SOLID,
                1,
                rgb(if self.capturing || self.focused {
                    FLAX
                } else {
                    EDGE
                }),
            );
            let old_brush = SelectObject(hdc, field.into());
            let old_pen = SelectObject(hdc, edge.into());
            let radius = (RADIUS * 2.0 * scale) as i32;
            let _ = RoundRect(hdc, 0, 0, r.right, r.bottom, radius, radius);
            let _ = SelectObject(hdc, old_brush);
            let _ = SelectObject(hdc, old_pen);
            let _ = DeleteObject(field.into());
            let _ = DeleteObject(edge.into());

            let clear = self.vk != 0 && !self.capturing;
            let shown = key_text(self.capturing, self.pending, self.mods, self.vk);
            let chosen = self.vk != 0 || (self.capturing && self.pending != 0);
            let text_box = RECT {
                left: (9.0 * scale).round() as i32,
                top: r.top,
                right: r.right
                    - if clear {
                        (CLEAR_W * scale).round() as i32
                    } else {
                        (9.0 * scale).round() as i32
                    },
                bottom: r.bottom,
            };
            (*self.state).text(
                hdc,
                &shown,
                F_TITLE,
                if chosen { INK } else { MUTED },
                text_box,
                DT_VCENTER | DT_LEFT,
            );

            if clear {
                // Drawn, not typed: a multiply of a font's × is a different shape
                // at every scale, and this one is two strokes.
                let arm = 3.5 * scale;
                let cx = (r.right as f32 - CLEAR_W * scale / 2.0).round();
                let cy = (r.bottom as f32 / 2.0).round();
                let cross = CreatePen(PS_SOLID, 1.max((1.3 * scale) as i32), rgb(MUTED));
                let old_pen = SelectObject(hdc, cross.into());
                for dy in [-arm, arm] {
                    let _ = MoveToEx(
                        hdc,
                        (cx - arm).round() as i32,
                        (cy + dy).round() as i32,
                        None,
                    );
                    let _ = LineTo(hdc, (cx + arm).round() as i32, (cy - dy).round() as i32);
                }
                let _ = SelectObject(hdc, old_pen);
                let _ = DeleteObject(cross.into());
            }
            if self.focused {
                focus_ring(hdc, &r, radius, scale);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }
}

/// The modifiers, in the order a label reads them.
fn mod_names(mods: u32) -> Vec<&'static str> {
    let mut names = Vec::new();
    if mods & settings::MOD_CONTROL != 0 {
        names.push("Ctrl");
    }
    if mods & settings::MOD_ALT != 0 {
        names.push("Alt");
    }
    if mods & settings::MOD_SHIFT != 0 {
        names.push("Shift");
    }
    if mods & settings::MOD_WIN != 0 {
        names.push("Win");
    }
    names
}

/// What the box says: the combination under the fingers, then the one that is
/// set, then what it takes to set one.
fn key_text(capturing: bool, pending: u32, mods: u32, vk: u32) -> String {
    if capturing {
        let names = mod_names(pending);
        if names.is_empty() {
            t("settings_key_capture").to_string()
        } else {
            format!("{} …", names.join("+"))
        }
    } else if vk == 0 {
        t("settings_key_unset").to_string()
    } else {
        settings::hotkey_label(mods, vk)
    }
}

/// The keys that are parts of a combination rather than one of their own.
fn is_modifier(key: u32) -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|vk| vk.0 as u32 == key)
}

/// The modifiers as they physically are, read off the keyboard instead of off
/// the message queue. A queue can be a key behind, and a shortcut recorded one
/// modifier short is a shortcut that does not do what its own label says.
unsafe fn held_mods() -> u32 {
    let mut mods = 0;
    if is_down(VK_CONTROL) {
        mods |= settings::MOD_CONTROL;
    }
    if is_down(VK_MENU) {
        mods |= settings::MOD_ALT;
    }
    if is_down(VK_SHIFT) {
        mods |= settings::MOD_SHIFT;
    }
    if is_down(VK_LWIN) || is_down(VK_RWIN) {
        mods |= settings::MOD_WIN;
    }
    mods
}

/// The high bit of the key state is the answer to "is it down now?".
fn is_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
}

// --- the two buttons ----------------------------------------------------------------

/// Save and Cancel. A stock button would do almost the same job, but an
/// owner-drawn one paints through its parent's `WM_DRAWITEM`, which a print pass
/// never asks for: a screenshot of this window would show two blank rectangles
/// and tell nobody whether the panel is right.
struct Button {
    hwnd: HWND,
    parent: HWND,
    state: *mut State,
    id: i32,
    text: String,
    primary: bool,
    pressed: bool,
    focused: bool,
}

unsafe extern "system" fn button_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Button;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ctrl = &mut *ptr;
        match msg {
            WM_PAINT => {
                ctrl.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_GETDLGCODE => LRESULT((DLGC_BUTTON | DLGC_WANTCHARS) as isize),
            // Enter, from whoever answers DM_GETDEFID for the panel.
            BM_CLICK => {
                ctrl.click();
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let _ = SetFocus(Some(hwnd));
                ctrl.pressed = true;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                ctrl.pressed = false;
                let inside = {
                    let mut r = RECT::default();
                    let _ = GetClientRect(hwnd, &mut r);
                    let x = lp.0 as i16 as i32;
                    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
                    x >= 0 && x < r.right && y >= 0 && y < r.bottom
                };
                if inside {
                    ctrl.click();
                } else {
                    ctrl.refresh();
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                // The space bar usually arrives as a click pair instead, from the
                // dialog manager; this is the path for when it does not.
                if wp.0 as u32 == VK_SPACE.0 as u32 {
                    ctrl.click();
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_SETFOCUS => {
                ctrl.focused = true;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                ctrl.focused = false;
                ctrl.pressed = false;
                ctrl.refresh();
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

impl Button {
    /// The same `WM_COMMAND` a real button sends, so the panel's answer to a
    /// click does not care which kind of button it came from.
    fn click(&self) {
        unsafe {
            let _ = send(
                self.parent,
                WM_COMMAND,
                (BN_CLICKED as usize) << 16 | self.id as usize,
                self.hwnd.0 as isize,
            );
        }
    }

    fn refresh(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    unsafe fn paint(&self) {
        unsafe {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if hdc.is_invalid() {
                return;
            }
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let scale = (*self.state).scale;
            let bg = if self.primary {
                if self.pressed {
                    (0x1B, 0x20, 0x2A)
                } else {
                    SLATE
                }
            } else if self.pressed {
                (0xEC, 0xEF, 0xF2)
            } else {
                FIELD
            };
            let brush = solid(bg);
            let pen = CreatePen(PS_SOLID, 1, rgb(if self.primary { SLATE } else { EDGE }));
            let old_brush = SelectObject(hdc, brush.into());
            let old_pen = SelectObject(hdc, pen.into());
            let radius = (RADIUS * 2.0 * scale) as i32;
            let _ = RoundRect(hdc, 0, 0, r.right, r.bottom, radius, radius);
            let _ = SelectObject(hdc, old_brush);
            let _ = SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush.into());
            let _ = DeleteObject(pen.into());

            (*self.state).text(
                hdc,
                &self.text,
                F_BUTTON,
                if self.primary {
                    (0xFF, 0xFF, 0xFF)
                } else {
                    INK
                },
                r,
                DT_CENTER | DT_VCENTER,
            );
            if self.focused {
                // The dotted rectangle Windows draws is invisible on a filled
                // button; a ring on the shape itself reads as focus either way.
                focus_ring(hdc, &r, radius, scale);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }
}

// --- fonts and messages --------------------------------------------------------------

/// `SendMessageW` in these bindings takes optional parameters; every message
/// this window sends has real ones.
unsafe fn send(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}

fn make_fonts(scale: f32) -> [HFONT; 4] {
    let face = if crate::i18n::is_chinese() {
        w!("Microsoft YaHei UI")
    } else {
        w!("Segoe UI")
    };
    [
        font(scale, 13.0, FW_NORMAL.0 as i32, face),
        font(scale, 11.5, FW_NORMAL.0 as i32, face),
        font(scale, 11.5, FW_SEMIBOLD.0 as i32, face),
        font(scale, 13.0, FW_SEMIBOLD.0 as i32, face),
    ]
}

fn font(scale: f32, size: f32, weight: i32, face: PCWSTR) -> HFONT {
    unsafe {
        let handle = CreateFontW(
            -(size * scale).round() as i32,
            0,
            0,
            0,
            weight,
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
        if handle.is_invalid() {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        } else {
            handle
        }
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

// --- what this file promises ---------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{IntSize, PixmapPaint};

    /// The measure is the window: `LOGICAL_H` is `measure` walked with the same
    /// constants, so the panel can never outgrow its own frame or float inside a
    /// taller one. Checked at the scales a person actually runs.
    #[test]
    fn the_window_is_exactly_as_tall_as_its_rows() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let l = measure((LOGICAL_W * scale).round() as i32, scale);
            assert_eq!(l.head_keys.top, (PAD * scale).round() as i32);
            assert_eq!(
                l.save.bottom + (PAD_BOTTOM * scale).round() as i32,
                (LOGICAL_H * scale).round() as i32,
                "the panel is not the height it claims at scale {scale}"
            );

            // Rows go down the page and never run into each other.
            let rows = [
                l.head_keys,
                l.label_hang,
                l.label_toggle,
                l.rule_a,
                l.head_line,
                l.rope,
                l.sag_note,
                l.rule_b,
                l.always_title,
                l.drag_title,
                l.save,
            ];
            for pair in rows.windows(2) {
                assert!(pair[1].top >= pair[0].bottom, "rows cross at scale {scale}");
            }
            // The control column is to the right of the label column, and the
            // buttons share one baseline.
            assert!(l.field_hang.left > l.label_hang.right);
            assert!(l.field_toggle.left > l.label_toggle.right);
            assert!(l.switch_always.left >= l.always_title.right);
            assert!(l.switch_drag.left >= l.drag_title.right);
            assert_eq!(l.save.bottom, l.cancel.bottom);
            assert!(l.cancel.right < l.save.left);
            // The note is the second line of its item, so it must still be
            // inside the band the item was given.
            assert!(l.always_note.bottom <= l.always_title.top + (ITEM_H * scale).round() as i32);
            assert!(l.drag_note.bottom <= l.drag_title.top + (ITEM_H * scale).round() as i32);
        }
    }

    /// The box tells "nothing is set" from "I am listening" from "this is what is
    /// set", and while it listens it shows the modifiers under your fingers -
    /// which is the only proof that a shortcut was heard at all.
    #[test]
    fn the_shortcut_box_says_what_it_means() {
        assert_eq!(key_text(false, 0, 0, 0), t("settings_key_unset"));
        assert_eq!(key_text(false, 0, settings::MOD_CONTROL, 0x48), "Ctrl+H");
        assert_eq!(
            key_text(true, 0, settings::MOD_CONTROL, 0x48),
            t("settings_key_capture")
        );
        assert_eq!(
            key_text(true, settings::MOD_CONTROL | settings::MOD_SHIFT, 0, 0),
            "Ctrl+Shift …"
        );
        assert_eq!(
            settings::hotkey_label(settings::MOD_WIN | settings::MOD_ALT, 0x54),
            "Alt+Win+T"
        );
    }

    /// The tile spans exactly the range the line understands: the slackest pull
    /// on the tile is the slackest curve the rope can hold, no dead travel.
    #[test]
    fn the_rope_pulls_in_the_range_the_line_understands() {
        assert_eq!(rail_sag(0.0), 0.0);
        assert_eq!(rail_sag(SAG_PERCENT_MAX), ROPE_ROOM);
        assert_eq!(
            rail_sag(1000.0),
            ROPE_ROOM,
            "the rope must not leave the tile"
        );
        // One logical pixel of drag is a tenth of a percent, so the fine step is
        // still a real change even though it moves the drawn rope under a pixel.
        assert_ne!(rail_sag(SAG_STEP_FINE), rail_sag(0.0));
    }

    /// The drag is relative to where the hand landed: pressing the rope never
    /// moves it, and one percent of curve is always ten pixels of pointer.
    #[test]
    fn a_press_grabs_the_rope_where_it_hangs() {
        let grab = |at: f32, shown: f32| at - shown * SAG_PX_PER_PERCENT;
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(near(pulled(grab(30.0, 4.0), 30.0), 4.0));
        assert!(near(pulled(grab(30.0, 4.0), 31.0), 4.1));
        assert!(near(pulled(grab(0.0, 0.0), 160.0), SAG_PERCENT_MAX));
        assert!(
            near(pulled(grab(0.0, 4.0), -1000.0), -96.0),
            "the ceiling is the rope's job, not the mapping's"
        );
    }

    /// `自动` is a value, not the absence of one, and it is the value the
    /// settings file asks for. Freezing it into a percentage on save is the bug
    /// the old control had.
    #[test]
    fn auto_is_a_choice_with_a_number_of_its_own() {
        assert_eq!(Pick::Auto.value(), settings::SAG_AUTO);
        assert_eq!(Pick::Picked(3.5).value(), 3.5);
        assert_ne!(Pick::Auto.value(), Pick::Picked(0.0).value());
    }

    /// Draws the tile at four curves into %TEMP%\shaiwin-rope-shots so a person
    /// can look at them. Nothing here touches Windows: the art is a pure function
    /// onto a raster, which is the whole point of keeping it separate.
    #[test]
    fn the_tile_renders() {
        let dir = std::env::temp_dir().join("shaiwin-rope-shots");
        let _ = std::fs::create_dir_all(&dir);
        let scale = 2.0;
        let (w, h) = (900u32, (ROPE_H * scale) as u32);
        for (percent, live) in [
            (0.0f32, false),
            (1.4, false),
            (8.0, true),
            (SAG_PERCENT_MAX, true),
        ] {
            let (bytes, aw, ah) =
                rope_art(w, h, scale, rail_sag(percent), live).expect("the tile renders");
            assert_eq!((aw, ah), (w, h));
            let rope =
                Pixmap::from_vec(bytes, IntSize::from_wh(w, h).expect("the tile has a size"))
                    .expect("the raster is a pixmap");
            // The tile paints this over a white plate, so the shot shows what shows.
            let mut out = Pixmap::new(w, h).unwrap();
            out.fill(Color::WHITE);
            out.draw_pixmap(
                0,
                0,
                rope.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            let name = format!(
                "rope-{percent:.1}pct-{}.png",
                if live { "warm" } else { "idle" }
            );
            let _ = std::fs::write(dir.join(name), out.encode_png().unwrap());
        }
    }
}
