//! Parking: moving a real window off screen without losing it.
//!
//! The window is never hidden, destroyed or reparented. It keeps its taskbar
//! button, its Alt+Tab entry and its place in the z-order, and we remember its
//! `WINDOWPLACEMENT` first. That combination is the user's escape hatch: even if
//! ShaiWin is killed, the window can be found and dragged back by hand.
//!
//! The work is split in two so the journal can be written *between* the two
//! halves:
//!
//! ```text
//! plan()      read the placement, work out where it will go   (no mutation)
//!   ...       write the journal entry                         (disk)
//! execute()   restore if maximized, move off screen           (mutation)
//! ```
//!
//! If the process dies between the two, the journal says "this window is about
//! to be parked" and startup recovery simply puts it back where it was.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowPlacement, IsIconic, IsWindow, IsZoomed, SetWindowPlacement,
    SetWindowPos, ShowWindow, HWND_TOP, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSENDCHANGING,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED,
    SW_SHOWNOACTIVATE, WINDOWPLACEMENT, WINDOWPLACEMENT_FLAGS,
};

use crate::window;
use crate::Rect;

/// How far outside the virtual desktop parked windows sit.
const PARK_GAP: i32 = 64;

/// The two numbers the journal needs to bring a window back exactly as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub flags: u32,
    pub show_cmd: u32,
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub normal: Rect,
}

/// `WINDOWPLACEMENT.showCmd` values this module cares about.
const SW_SHOWMINIMIZED_CMD: u32 = 2;
const SW_SHOWMAXIMIZED_CMD: u32 = 3;
/// `WPF_RESTORETOMAXIMIZED`: a minimized window that was maximized before.
const WPF_RESTORETOMAXIMIZED: u32 = 0x0002;

/// The show state a window should come back in. A minimized window's
/// `showCmd` is "minimized", and applying that on the way back leaves the
/// window minimized - seen by the user as "clicking the card does not bring it
/// back". What the record must keep is how it looked before it was minimized.
fn visible_show_cmd(flags: u32, show_cmd: u32) -> u32 {
    if show_cmd == SW_SHOWMINIMIZED_CMD {
        if flags & WPF_RESTORETOMAXIMIZED != 0 {
            SW_SHOWMAXIMIZED_CMD
        } else {
            1 // SW_SHOWNORMAL
        }
    } else {
        show_cmd
    }
}

impl Placement {
    pub fn capture(hwnd: HWND) -> Option<Placement> {
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return None;
            }
            let mut wp = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            if GetWindowPlacement(hwnd, &mut wp).is_err() {
                return None;
            }
            Some(Placement {
                flags: wp.flags.0,
                show_cmd: visible_show_cmd(wp.flags.0, wp.showCmd),
                min_x: wp.ptMinPosition.x,
                min_y: wp.ptMinPosition.y,
                max_x: wp.ptMaxPosition.x,
                max_y: wp.ptMaxPosition.y,
                normal: Rect {
                    left: wp.rcNormalPosition.left,
                    top: wp.rcNormalPosition.top,
                    right: wp.rcNormalPosition.right,
                    bottom: wp.rcNormalPosition.bottom,
                },
            })
        }
    }

    /// Restores size, position and the maximized/minimized state in one call.
    pub fn apply(&self, hwnd: HWND) -> bool {
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return false;
            }
            let wp = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                flags: WINDOWPLACEMENT_FLAGS(self.flags),
                showCmd: self.show_cmd,
                ptMinPosition: POINT {
                    x: self.min_x,
                    y: self.min_y,
                },
                ptMaxPosition: POINT {
                    x: self.max_x,
                    y: self.max_y,
                },
                rcNormalPosition: RECT {
                    left: self.normal.left,
                    top: self.normal.top,
                    right: self.normal.right,
                    bottom: self.normal.bottom,
                },
            };
            SetWindowPlacement(hwnd, &wp).is_ok()
        }
    }
}

/// Everything needed to move a window that has not moved yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkPlan {
    pub placement: Placement,
    /// Where the window will sit while parked.
    pub target: Rect,
    pub was_maximized: bool,
}

/// A window that is parked right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parked {
    pub placement: Placement,
    /// Where the window physically sits while it is parked. Used to notice
    /// somebody else moving it (taskbar, Alt+Tab, "Cascade windows", ...).
    pub parked: Rect,
    pub was_maximized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkError {
    Gone,
    Minimized,
    Elevated,
    Refused(u32),
    NotMoved,
}

impl ParkError {
    pub fn message(self) -> String {
        match self {
            ParkError::Gone => "That window closed before it could be hung up.".to_string(),
            ParkError::Minimized => "Restore the window before hanging it up.".to_string(),
            ParkError::Elevated => {
                "Windows refused to move that window because it runs as administrator.".to_string()
            }
            ParkError::Refused(code) => {
                format!(
                    "Windows refused to move that window: {}",
                    crate::last_error_text(code)
                )
            }
            ParkError::NotMoved => {
                "The window would not move. Some full-screen and protected windows refuse."
                    .to_string()
            }
        }
    }
}

fn virtual_bounds() -> Rect {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        Rect {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }
}

/// Where a window of this size is parked: just past the right edge of the
/// virtual desktop, aligned with the top of the monitor it came from.
pub fn parking_spot(size: (i32, i32), origin: Rect) -> (i32, i32) {
    let v = virtual_bounds();
    let x = v.right + PARK_GAP;
    let y = origin
        .top
        .clamp(v.top, (v.bottom - size.1.max(1)).max(v.top));
    (x, y)
}

pub fn plan(hwnd: HWND) -> Result<ParkPlan, ParkError> {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err(ParkError::Gone);
        }
        if IsIconic(hwnd).as_bool() {
            return Err(ParkError::Minimized);
        }
        let placement = Placement::capture(hwnd).ok_or(ParkError::Gone)?;
        let was_maximized = IsZoomed(hwnd).as_bool();
        // While maximized, GetWindowRect is the whole monitor. The restored
        // size is what the window will have once it is off screen.
        let current = if was_maximized {
            placement.normal
        } else {
            window::rect_of(hwnd).ok_or(ParkError::Gone)?
        };
        if current.is_empty() {
            return Err(ParkError::NotMoved);
        }
        let (x, y) = parking_spot((current.width(), current.height()), current);
        Ok(ParkPlan {
            placement,
            target: Rect {
                left: x,
                top: y,
                right: x + current.width(),
                bottom: y + current.height(),
            },
            was_maximized,
        })
    }
}

/// Moves a real window out of sight. The snapshot must already have been taken:
/// once parked, some applications stop repainting.
pub fn execute(hwnd: HWND, plan: &ParkPlan) -> Result<Parked, ParkError> {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err(ParkError::Gone);
        }
        if plan.was_maximized {
            // A maximized window is not moved at all: SetWindowPos drops it out
            // of the maximized state on the way, and what comes back is a window
            // that Windows *reports* as maximized at the size of its normal
            // rectangle - the "it opens twice, once small" the user sees.
            // Minimizing is the one way to take it off screen without touching
            // its state; the placement still says "maximized", so it comes back
            // full screen. The compositor's minimize animation is disabled for
            // the trip, so it simply disappears.
            let animate = !transitions(hwnd, false);
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
            if animate {
                let _ = transitions(hwnd, true);
            }
            if IsIconic(hwnd).as_bool() {
                let parked = Rect {
                    left: ICONIC,
                    top: ICONIC,
                    right: ICONIC,
                    bottom: ICONIC,
                };
                return Ok(Parked {
                    placement: plan.placement,
                    parked,
                    was_maximized: true,
                });
            }
            // Minimizing was refused (an elevated window refuses every call from
            // here); the move below is what reports the refusal properly.
        }
        let target = plan.target;
        let moved = SetWindowPos(
            hwnd,
            None,
            target.left,
            target.top,
            target.width(),
            target.height(),
            SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING | SWP_SHOWWINDOW,
        );
        if let Err(e) = moved {
            let code = (e.code().0 as u32) & 0xFFFF;
            return Err(if code == 5 {
                ParkError::Elevated
            } else {
                ParkError::Refused(code)
            });
        }
        let Some(after) = window::rect_of(hwnd) else {
            return Err(ParkError::Gone);
        };
        // Some windows clamp themselves back onto a monitor; treat that as a
        // refusal instead of pretending they are out of the way.
        if (after.left - target.left).abs() > 4 || (after.top - target.top).abs() > 4 {
            plan.placement.apply(hwnd);
            return Err(ParkError::NotMoved);
        }
        Ok(Parked {
            placement: plan.placement,
            parked: after,
            was_maximized: plan.was_maximized,
        })
    }
}

pub fn park(hwnd: HWND) -> Result<Parked, ParkError> {
    let plan = plan(hwnd)?;
    execute(hwnd, &plan)
}

/// Where Windows draws a minimized window. It is not a place on any monitor.
const ICONIC: i32 = -32000;

/// A window that is minimized is already out of sight, so hanging it up takes no
/// moving at all: it stays exactly where it stands and only its card goes back
/// on the line. Moving it instead means restoring it first, and the user sees it
/// flash onto the desktop before it disappears again.
pub fn hung_up_in_place(placement: Placement) -> Parked {
    Parked {
        placement,
        parked: Rect {
            left: ICONIC,
            top: ICONIC,
            right: ICONIC,
            bottom: ICONIC,
        },
        was_maximized: false,
    }
}

/// Whether the window went onto the line from the minimized state. Its "parked"
/// rect is the minimized position, so that is what it has to stay at.
pub fn parked_minimized(parked: &Parked) -> bool {
    parked.parked.left <= -31000 && parked.parked.top <= -31000
}

/// Whether the window is still hanging the way we left it: off screen at the
/// parked rect, or - for a window hung up while minimized - still minimized.
/// Anything else means the taskbar, Alt+Tab or a virtual-desktop switch put it
/// back where the user can see it.
pub fn still_hung(hwnd: HWND, parked: &Parked) -> bool {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            return parked_minimized(parked);
        }
        match window::rect_of(hwnd) {
            Some(now) => {
                (now.left - parked.parked.left).abs() <= 8
                    && (now.top - parked.parked.top).abs() <= 8
            }
            None => false,
        }
    }
}

/// Where the off-screen rect sits relative to a parked record. Kept for the
/// tests that describe the geometry without a real window.
pub fn is_parked_rect(now: Rect, parked: Rect) -> bool {
    (now.left - parked.left).abs() <= 8 && (now.top - parked.top).abs() <= 8
}

/// Brings a parked window back exactly where it was, and to the front.
///
/// Whatever the window did while it was on the line — minimized from the taskbar,
/// hidden by "Show desktop", moved by another program — it ends up visible, on
/// screen and where the user left it. A card that comes off the line must never
/// leave its window somewhere the user cannot find.
pub fn restore(hwnd: HWND, parked: &Parked) -> bool {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        // Journals written before the show state was normalized hold "minimized"
        // for windows the user had minimized. Applying that back is what made a
        // click on the card look like it did nothing, so normalize on the way
        // out as well as on the way in.
        let mut placement = parked.placement;
        placement.show_cmd = visible_show_cmd(placement.flags, placement.show_cmd);
        // One call sets the position, the size and the show state. Moving the
        // window on screen first and then maximizing it showed the user the
        // window twice: once at the size we put it, once at the size Windows
        // made it. Let DWM's animation go too - even a correct maximize is
        // animated on screen, and the whole point is that the window is simply
        // there when the card comes down.
        let animate = !transitions(hwnd, false);
        let applied = placement.apply(hwnd);
        // A window that is off screen (or busy) can ignore SetWindowPlacement.
        // Verify, then move it by hand: silently leaving it parked while the
        // card disappears would strand it.
        if !applied {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            if placement.show_cmd == SW_SHOWMAXIMIZED_CMD && !at_target(hwnd, &placement) {
                let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
            } else if !at_target(hwnd, &placement) {
                let target = placement.normal;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    target.left,
                    target.top,
                    target.width(),
                    target.height(),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        // The last resort for a window that refused both: put it where a monitor
        // is. Keep as much of its remembered position as the work area allows,
        // so a row of restored windows does not pile up in one spot.
        if !window::on_screen(hwnd) {
            let spot = on_screen_spot(placement.normal, hwnd);
            let _ = SetWindowPos(
                hwnd,
                None,
                spot.left,
                spot.top,
                spot.width(),
                spot.height(),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        // Raise it above the other windows before asking for the focus. The
        // focus can be refused (Windows only hands it to the foreground process)
        // but the z-order cannot, so the worst case is a window the user can see
        // and click, instead of nothing at all.
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        window::force_foreground(hwnd);
        if animate {
            let _ = transitions(hwnd, true);
        }
        at_target(hwnd, &placement) && !IsIconic(hwnd).as_bool() && window::on_screen(hwnd)
    }
}

/// Turns DWM's window transitions on or off for one window, and says whether it
/// was on to begin with.
///
/// A restored window has to arrive in one step. Every show-state change is
/// animated by the compositor otherwise: a maximize grows out of the old
/// rectangle, a restore fades in - and on a window coming back from the line
/// that animation *is* the "it opened twice" the user sees, because the frame
/// before the animation still shows the window at its old size.
fn transitions(hwnd: HWND, on: bool) -> bool {
    unsafe {
        let mut current = 0u32;
        let size = std::mem::size_of::<u32>() as u32;
        // Read first. If the window has no DWM attributes at all there is
        // nothing to restore later, so it is left untouched rather than left
        // without animations for the rest of its life.
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &mut current as *mut u32 as *mut core::ffi::c_void,
            size,
        )
        .is_err()
        {
            return false;
        }
        let was_on = current == 0;
        let value: u32 = u32::from(!on);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &value as *const u32 as *const core::ffi::c_void,
            size,
        );
        was_on
    }
}

/// The window's size, cut down to `work` if it does not fit, centred in it.
///
/// Used for windows that were hung up by dragging them onto the line: the drag
/// says "put it away for now", so the window comes back in the middle of the
/// screen it was hung from rather than wherever it happened to sit before.
pub fn centre_in(rect: Rect, work: Rect) -> Rect {
    let width = rect.width().min(work.width().max(1));
    let height = rect.height().min(work.height().max(1));
    let x = work.left + (work.width() - width) / 2;
    let y = work.top + (work.height() - height) / 2;
    Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

/// The record moved to the middle of `work`, keeping its size and its show
/// state. A window that was maximized has no meaningful centre: applying the
/// normal rectangle would bring it back as a window in the middle instead of
/// full screen, so the record is left exactly as it was.
pub fn centred_on(parked: &Parked, work: Rect) -> Parked {
    let mut next = *parked;
    if next.placement.show_cmd == SW_SHOWMAXIMIZED_CMD {
        return next;
    }
    next.placement.normal = centre_in(next.placement.normal, work);
    next
}

/// Quietly leaves a parked window minimized.
///
/// Quitting is not "give me all my windows back": a line with eight cards would
/// throw eight windows onto the desktop at once. The window goes back to its
/// remembered place with the minimized show state, so its taskbar button is
/// there and one click brings it back where it belongs. The placement is applied
/// in a single call, while the window is still off screen, so it is never drawn
/// in the normal state on the way out.
pub fn stow(hwnd: HWND, parked: &Parked) -> bool {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            // Hung up while already minimized: it is where it should be, and its
            // placement still remembers the rect to come back to.
            return true;
        }
        if minimized_placement(&parked.placement).apply(hwnd) && IsIconic(hwnd).as_bool() {
            return true;
        }
        // A window that ignores the show state in SetWindowPlacement still ends
        // up findable on the taskbar; it may just come back at the parked spot.
        let _ = ShowWindow(hwnd, SW_MINIMIZE);
        IsIconic(hwnd).as_bool()
    }
}

/// The record with "minimized" as its show state. A window that was maximized
/// keeps `WPF_RESTORETOMAXIMIZED`, so un-minimizing brings it back full screen
/// rather than as a small window in the corner.
fn minimized_placement(placement: &Placement) -> Placement {
    let mut next = *placement;
    next.flags &= !WPF_RESTORETOMAXIMIZED;
    if next.show_cmd == SW_SHOWMAXIMIZED_CMD {
        next.flags |= WPF_RESTORETOMAXIMIZED;
    }
    next.show_cmd = SW_SHOWMINIMIZED_CMD;
    next
}

/// `target` slid onto the work area of the monitor nearest to `hwnd`.
///
/// Clamping rather than centring keeps each window's own remembered position,
/// which is the difference between a row of windows coming back where they were
/// and a stack of them all landing in the middle of the screen.
fn on_screen_spot(target: Rect, hwnd: HWND) -> Rect {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Rect {
            left: 40,
            top: 40,
            right: 40 + target.width(),
            bottom: 40 + target.height(),
        };
    }
    let work = Rect {
        left: info.rcWork.left,
        top: info.rcWork.top,
        right: info.rcWork.right,
        bottom: info.rcWork.bottom,
    };
    let width = target.width().min(work.width().max(1));
    let height = target.height().min(work.height().max(1));
    let x = target
        .left
        .clamp(work.left, (work.right - width).max(work.left));
    let y = target
        .top
        .clamp(work.top, (work.bottom - height).max(work.top));
    Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

/// Whether the window currently sits where the placement says it should.
///
/// The tolerance is tight on purpose: a window left 24 px away from its place is
/// a window that ignored the placement, and accepting that is what made every
/// restored window look like it came back somewhere else.
fn at_target(hwnd: HWND, placement: &Placement) -> bool {
    unsafe {
        if placement.show_cmd == SW_SHOWMAXIMIZED_CMD {
            // `IsZoomed` alone is not enough: a window that was moved while it
            // was maximized comes back flagged as zoomed while Windows draws it
            // at its normal size. It has to actually fill the monitor.
            return IsZoomed(hwnd).as_bool() && fills_monitor(hwnd);
        }
        match window::rect_of(hwnd) {
            Some(now) => {
                (now.left - placement.normal.left).abs() <= 4
                    && (now.top - placement.normal.top).abs() <= 4
            }
            None => false,
        }
    }
}

/// Whether the window is drawn across the work area of the monitor it is on.
fn fills_monitor(hwnd: HWND) -> bool {
    let Some(now) = window::rect_of(hwnd) else {
        return false;
    };
    let work = work_area(hwnd);
    // A maximized window's frame extends a few pixels past the work area for the
    // resize border, so the comparison is allowed to be generous in that
    // direction and not at all in the other.
    now.width() >= work.width() - 16 && now.height() >= work.height() - 16
}

/// The monitor's work area, which the overlay must respect (taskbar).
pub fn work_area(hwnd: HWND) -> Rect {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            Rect {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            }
        } else {
            virtual_bounds()
        }
    }
}

/// Whether the record brings the window back maximized.
pub fn opens_maximized(placement: &Placement) -> bool {
    visible_show_cmd(placement.flags, placement.show_cmd) == SW_SHOWMAXIMIZED_CMD
}

/// Where a maximized window is drawn when its normal rect lives on `monitor`:
/// the monitor's work area. Used for the ghost of a maximized card, which has
/// to fly to the size the window will really have - flying to the normal rect
/// is what made a maximized window look like it opened small first.
pub fn maximized_rect(normal: Rect) -> Rect {
    unsafe {
        let rect = RECT {
            left: normal.left,
            top: normal.top,
            right: normal.right,
            bottom: normal.bottom,
        };
        let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            Rect {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            }
        } else {
            normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parking_spot_is_outside_every_monitor() {
        let v = virtual_bounds();
        let (x, _) = parking_spot(
            (800, 600),
            Rect {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
        );
        assert!(x > v.right);
    }

    #[test]
    fn a_window_hung_up_minimized_is_marked_as_sitting_still() {
        let parked = hung_up_in_place(Placement {
            flags: 0,
            show_cmd: 1,
            min_x: -1,
            min_y: -1,
            max_x: -2,
            max_y: -2,
            normal: Rect {
                left: 10,
                top: 20,
                right: 810,
                bottom: 620,
            },
        });
        assert!(parked_minimized(&parked));
        // A window pushed off the right edge is a different kind of hanging.
        let moved = Parked {
            parked: Rect {
                left: 2624,
                top: 20,
                right: 3324,
                bottom: 620,
            },
            ..parked
        };
        assert!(!parked_minimized(&moved));
    }

    #[test]
    fn a_minimized_window_comes_back_visible() {
        // A minimized window's record says "minimized". Applying that on the way
        // back left the window minimized, which reads as "the click did nothing".
        assert_eq!(visible_show_cmd(0, SW_SHOWMINIMIZED_CMD), 1);
        // It was maximized before it was minimized: it comes back maximized.
        assert_eq!(
            visible_show_cmd(WPF_RESTORETOMAXIMIZED, SW_SHOWMINIMIZED_CMD),
            SW_SHOWMAXIMIZED_CMD
        );
        // Other show states are left alone.
        assert_eq!(visible_show_cmd(0, 1), 1);
        assert_eq!(
            visible_show_cmd(0, SW_SHOWMAXIMIZED_CMD),
            SW_SHOWMAXIMIZED_CMD
        );
    }

    #[test]
    fn quitting_minimizes_without_losing_the_way_back() {
        let normal = Rect {
            left: 10,
            top: 20,
            right: 810,
            bottom: 620,
        };
        let record = Placement {
            flags: 0,
            show_cmd: 1,
            min_x: -1,
            min_y: -1,
            max_x: -2,
            max_y: -2,
            normal,
        };
        let stowed = minimized_placement(&record);
        assert_eq!(stowed.show_cmd, SW_SHOWMINIMIZED_CMD);
        assert_eq!(stowed.normal, normal);
        // It was maximized before: un-minimizing brings it back maximized.
        let maximized = Placement {
            show_cmd: SW_SHOWMAXIMIZED_CMD,
            ..record
        };
        let stowed = minimized_placement(&maximized);
        assert_eq!(stowed.show_cmd, SW_SHOWMINIMIZED_CMD);
        assert_ne!(stowed.flags & WPF_RESTORETOMAXIMIZED, 0);
        // An ordinary window must not come back maximized.
        assert_eq!(
            minimized_placement(&record).flags & WPF_RESTORETOMAXIMIZED,
            0
        );
    }

    #[test]
    fn a_centred_record_keeps_its_size_and_lands_in_the_middle() {
        let record = Parked {
            placement: Placement {
                flags: 0,
                show_cmd: 1,
                min_x: -1,
                min_y: -1,
                max_x: -2,
                max_y: -2,
                normal: Rect {
                    left: 100,
                    top: 200,
                    right: 900,
                    bottom: 800,
                },
            },
            parked: Rect {
                left: 2624,
                top: 20,
                right: 3424,
                bottom: 620,
            },
            was_maximized: false,
        };
        let work = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let centred = centred_on(&record, work);
        assert_eq!(
            centred.placement.normal,
            Rect {
                left: 560,
                top: 220,
                right: 1360,
                bottom: 820
            }
        );
        // The place it is parked while hidden does not move.
        assert_eq!(centred.parked, record.parked);
        assert_eq!(centred.placement.show_cmd, record.placement.show_cmd);
    }

    #[test]
    fn a_window_bigger_than_the_work_area_is_cut_down_to_fit() {
        let work = Rect {
            left: 100,
            top: 0,
            right: 900,
            bottom: 700,
        };
        let big = Rect {
            left: 0,
            top: 0,
            right: 1600,
            bottom: 1200,
        };
        assert_eq!(centre_in(big, work), work);
    }

    #[test]
    fn a_maximized_record_is_not_centred() {
        // Maximized has no centre: changing the normal rect would make the
        // window come back as a box in the middle of the screen.
        let normal = Rect {
            left: 10,
            top: 20,
            right: 810,
            bottom: 620,
        };
        let record = Parked {
            placement: Placement {
                flags: 0,
                show_cmd: 3,
                min_x: -1,
                min_y: -1,
                max_x: -2,
                max_y: -2,
                normal,
            },
            parked: Rect {
                left: 2624,
                top: 20,
                right: 3424,
                bottom: 620,
            },
            was_maximized: true,
        };
        let work = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(centred_on(&record, work).placement.normal, normal);
    }

    #[test]
    fn a_parked_rect_tolerates_a_rounding_wander() {
        let spot = Rect {
            left: 2624,
            top: 20,
            right: 3324,
            bottom: 620,
        };
        assert!(is_parked_rect(
            Rect {
                left: 2620,
                top: 24,
                right: 3320,
                bottom: 624
            },
            spot
        ));
        assert!(!is_parked_rect(
            Rect {
                left: 100,
                top: 20,
                right: 800,
                bottom: 620
            },
            spot
        ));
    }
}
