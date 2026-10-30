//! Taking the still picture that becomes a card.
//!
//! Strategy, in order:
//!
//! 1. `PrintWindow(..., PW_RENDERFULLCONTENT)` — asks the window to paint
//!    itself into our bitmap. Works while the window is occluded, and for many
//!    windows while it is parked off screen.
//! 2. The same call without the flag: a few windows only implement the original
//!    `WM_PRINT` contract.
//! 3. A screen grab of its rectangle, but only while the window is on screen and
//!    nothing is over it. It is what saves Chrome, Electron and games that
//!    ignore `WM_PRINT`.
//!
//! Both `PrintWindow` calls can return an all-white bitmap (some apps do,
//! silently). [`Image::looks_blank_center`] catches a flat client even when a
//! painted frame makes the whole bitmap look varied, and the next strategy gets
//! a chance. A flat shot is not thrown away, though: a genuinely white window
//! has no better picture to offer. The screen is what tells the two apart — see
//! [`matches_screen`] — and a flat shot that disagrees with the screen, or that
//! was taken while something covered the window, is not a picture of it at all.
//! The card then says "no picture" instead of showing a white block.
//!
//! The screen grab must not be taken for a parked window: its rectangle is past
//! the desktop edge, so the grab would be the desktop. It must not be taken for
//! a window that hangs past the edge either, or the card would carry a black
//! band where the screen ends. Our own overlay is not an occluder, though: the
//! rope and the cards lie over the top of a hung window, and a grab that
//! includes them is still a picture of the window. That is why a flat print is
//! compared with the grab only across the middle band, below our rope.
//!
//! A hang starts at the mouse release, which is also when the target's modal
//! move/size loop is unwinding. [`settle`] waits for that loop to name no
//! window, so `PrintWindow` runs against a target that is ready to paint.
//!
//! **DPI.** `GetWindowRect` and the DWM frame are physical pixels because this
//! process is per-monitor aware. A window is not necessarily: a DPI-unaware one
//! on a 150% screen still paints at 96 DPI, so it fills only the top-left 2/3 of
//! a physical-sized bitmap and leaves an opaque black band everywhere else. The
//! bitmap is therefore sized for the target's own scale, and the part it
//! actually painted is measured from a sentinel fill rather than assumed.
//!
//! `PrintWindow` is synchronous: it sends messages to the target window. An
//! unresponsive application blocks the caller for as long as it is hung, which
//! is why the app crate calls this on a worker thread and never on the UI thread.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, GetDC, MonitorFromWindow, ReleaseDC, MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};

use crate::dib::Dib;
use crate::monitor;
use crate::window;
use crate::{Image, Rect};

/// The flag Windows 8.1+ understands: ask DWM-composited windows to paint
/// their full content, not just the client area.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// Longest side kept. A card never draws more than ~400 device pixels, and
/// keeping the whole 4K frame wastes memory for no visible gain.
const MAX_SIDE: u32 = 1280;

/// Largest bitmap [`PrintWindow`] is asked to fill, in either direction. A
/// broken DPI query must not turn into a gigabyte allocation.
const MAX_PRINT_SIDE: u32 = MAX_SIDE * 4;

/// Cleared into the bitmap before `PrintWindow`, in B, G, R, A order. A window
/// does not paint this exact magenta by accident, so whatever is left of it is
/// a part the window did not paint.
const UNPAINTED: [u8; 4] = [0xFF, 0x00, 0xFF, 0xFF];

/// What a snapshot attempt produced, and how it got there.
///
/// The app logs `via` and `settled_ms`. A card that comes out wrong is then a
/// question of which strategy painted it, not a guess.
pub struct Shot {
    pub image: Option<Image>,
    pub via: &'static str,
    pub settled_ms: u64,
}

pub fn snapshot(hwnd: isize) -> Shot {
    let h = window::hwnd_of(hwnd);
    unsafe {
        if !IsWindow(Some(h)).as_bool() || IsIconic(h).as_bool() {
            return Shot {
                image: None,
                via: "gone",
                settled_ms: 0,
            };
        }
    }
    let settled_ms = settle(h);
    let Some(full) = window::rect_of(h) else {
        return Shot {
            image: None,
            via: "no rect",
            settled_ms,
        };
    };
    let visible = window::visible_rect(h).unwrap_or(full);
    let (fw, fh) = (full.width().max(1), full.height().max(1));

    // See the module note: a DPI-unaware window paints into a smaller bitmap
    // than its physical rectangle. Ask for the size it will actually fill.
    let scale = print_scale(h);
    let pw = ((fw as f32 * scale).round() as u32).clamp(1, MAX_PRINT_SIDE);
    let ph = ((fh as f32 * scale).round() as u32).clamp(1, MAX_PRINT_SIDE);

    // A frame around a flat client is what a failed Chromium PrintWindow looks
    // like; only the middle matters for the card, so test that part. Keep the
    // first flat shot for now: a genuinely white window (a blank form, a white
    // page) has no better picture to offer, and showing it is honest. Whether
    // it is really the window is decided below, against the screen.
    let mut chosen: Option<(Image, &'static str)> = None;
    let mut flat: Option<Image> = None;
    for (via, flags) in [
        ("print:full", PW_RENDERFULLCONTENT),
        ("print:legacy", PRINT_WINDOW_FLAGS(0)),
    ] {
        if let Some(image) =
            print_window(h, pw, ph, flags).and_then(|printed| crop_visible(&printed, full, visible))
        {
            if !image.looks_blank_center() {
                chosen = Some((image, via));
                break;
            }
            if flat.is_none() {
                flat = Some(image);
            }
        }
    }

    let had_flat = flat.is_some();
    if chosen.is_none() {
        let on_screen = window::on_screen(h);
        // The screen can testify about the window only while it is visible in
        // full: off screen it is the desktop, covered it is whatever covers it.
        let clear = on_screen && unoccluded(h, visible);
        let grab_rect = if clear {
            clip(visible, monitor::virtual_bounds())
        } else {
            None
        };
        // A grab of part of the window would carry the desktop along the edge
        // that hangs off the screen, so only a whole one is used.
        let grab = grab_rect.filter(|r| *r == visible).and_then(screen_grab);

        // Off screen, a flat print is still the window's own painting; on
        // screen, only an unoccluded one is trustworthy.
        chosen = choose_flat(flat, grab, !on_screen || clear);
    }

    match chosen {
        Some((image, via)) => Shot {
            image: Some(image.downscale(MAX_SIDE, MAX_SIDE)),
            via,
            settled_ms,
        },
        // A discarded flat print is worth naming: it is the case that used to
        // turn into a white card. No print at all is just no picture.
        None => Shot {
            image: None,
            via: if had_flat { "print:blank" } else { "none" },
            settled_ms,
        },
    }
}

/// Picks between a flat print and the screen, once both are in hand.
///
/// A flat print is the only picture of a genuinely white window, and the screen
/// is the only picture of a window that ignores `WM_PRINT`. The screen decides:
/// a print that agrees with it is the window (keep it, so the thumbnail does not
/// show our own rope); one that does not is the silent all-white bitmap that
/// Chromium and Electron hand out while they hang, and the screen wins.
///
/// `trusted` says the print was taken while nothing covered the window, or that
/// the window is off screen. An untrusted flat print is not a picture of
/// anything the user can see, so it is dropped rather than shown as a white card.
fn choose_flat(
    flat: Option<Image>,
    grab: Option<Image>,
    trusted: bool,
) -> Option<(Image, &'static str)> {
    match (flat, grab) {
        (Some(print), Some(screen)) if matches_screen(&print, &screen) => {
            Some((print, "print:flat"))
        }
        (_, Some(screen)) => Some((screen, "screen")),
        (Some(print), None) if trusted => Some((print, "print:flat")),
        _ => None,
    }
}

/// How far a print pixel may be from the screen pixel and still be the same
/// face: compositing and scaling shift flat colours by a few levels, a page
/// behind a white print does not.
const SAME_COLOUR: u8 = 24;

/// Whether a flat print and a screen grab of the same window agree.
///
/// Both cover the window's visible rectangle, but not necessarily at the same
/// size: a DPI-unaware window prints smaller than the screen. Three points
/// across the middle band are compared — the top of a hung window is under our
/// own rope, and the middle is what the card shows. A flat print that matches
/// the screen is the window's real, uniform face; one that does not is the
/// silent all-white bitmap Chromium and Electron hand out while they hang.
fn matches_screen(print: &Image, screen: &Image) -> bool {
    if print.is_empty() || screen.is_empty() {
        return false;
    }
    let at = |image: &Image, fx: u32| image.pixel(image.width * fx / 100, image.height / 2);
    [30, 50, 70].into_iter().all(|fx| {
        let (a, b) = (at(print, fx), at(screen, fx));
        a[0].abs_diff(b[0]) <= SAME_COLOUR
            && a[1].abs_diff(b[1]) <= SAME_COLOUR
            && a[2].abs_diff(b[2]) <= SAME_COLOUR
    })
}

/// The part of `rect` that lies inside `bounds`, or `None` when they do not
/// overlap enough to be a picture.
fn clip(rect: Rect, bounds: Rect) -> Option<Rect> {
    let clipped = Rect {
        left: rect.left.max(bounds.left),
        top: rect.top.max(bounds.top),
        right: rect.right.min(bounds.right),
        bottom: rect.bottom.min(bounds.bottom),
    };
    if clipped.width() < 4 || clipped.height() < 4 {
        None
    } else {
        Some(clipped)
    }
}

/// Whether the target is the top-level window at a grid of points inside its
/// visible rectangle. A screen grab is only allowed then: if something covers
/// the window, the grab would show the covering window instead.
///
/// Our own overlay is not an occluder. The rope and the cards are topmost over
/// the top of a hung window, so counting them would refuse every grab for the
/// short windows where the grid lands in the band.
fn unoccluded(hwnd: HWND, rect: Rect) -> bool {
    let (w, h) = (rect.width(), rect.height());
    if w < 4 || h < 4 {
        return false;
    }
    let me = std::process::id();
    for fy in [1, 2, 3] {
        for fx in [1, 2, 3] {
            let x = rect.left + w * fx / 4;
            let y = rect.top + h * fy / 4;
            match window::root_at(x, y) {
                Some(top) if top == hwnd => {}
                Some(top) if window::pid_of(top) == me => {}
                _ => return false,
            }
        }
    }
    true
}

/// Waits for a title-bar drag's modal move/size loop to finish.
///
/// The app notices the mouse release and asks for a snapshot immediately, but
/// the target's own loop can still be alive for a few milliseconds. While it is,
/// Chromium and Electron windows answer `PrintWindow` with a painted frame and a
/// flat client, which is the all-white card this prevents. The loop is owned by
/// the target's UI thread and named by `GetGUIThreadInfo`.
fn settle(hwnd: HWND) -> u64 {
    let thread = window::thread_of(hwnd);
    if thread == 0 {
        return 0;
    }
    let start = Instant::now();
    while window::moving_on(thread).is_some() {
        if start.elapsed() >= Duration::from_millis(500) {
            return start.elapsed().as_millis() as u64;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // The loop has exited; give the target one frame to paint its final state.
    if start.elapsed() >= Duration::from_millis(1) {
        std::thread::sleep(Duration::from_millis(8));
    }
    start.elapsed().as_millis() as u64
}

/// How much of its physical rectangle a window fills when asked to print.
///
/// `GetDpiForWindow` answers 96 for a DPI-unaware window and the system DPI for
/// a system-aware one, while the monitor is at its real DPI. The ratio is the
/// scale the window's own coordinates use; per-monitor-aware windows answer the
/// monitor DPI and get 1.0, as before.
fn print_scale(hwnd: HWND) -> f32 {
    unsafe {
        let target = GetDpiForWindow(hwnd);
        if target == 0 {
            return 1.0;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return 1.0;
        }
        let mut mx = 0u32;
        let mut my = 0u32;
        let physical = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut mx, &mut my) {
            Ok(()) if mx > 0 => mx,
            _ => return 1.0,
        };
        let scale = target as f32 / physical as f32;
        // A wildly different answer is a broken query, not a window at 25%:
        // print at the physical size instead of risking a useless thumbnail.
        if (0.25..=4.0).contains(&scale) {
            scale
        } else {
            1.0
        }
    }
}

/// Crops a `PrintWindow` bitmap to the part of the window the user sees.
///
/// The bitmap may be smaller than the physical window rectangle (see
/// [`print_scale`]), and some windows paint only part of what they are given.
/// [`painted_bounds`] finds the part that holds content; the visible rectangle
/// is mapped onto that part, so the card matches what is on screen at any DPI.
fn crop_visible(printed: &Image, full: Rect, visible: Rect) -> Option<Image> {
    let (bx, by, bw, bh) = painted_bounds(printed)?;
    let kx = bw as f32 / full.width().max(1) as f32;
    let ky = bh as f32 / full.height().max(1) as f32;
    if !(0.1..=4.0).contains(&kx) || !(0.1..=4.0).contains(&ky) {
        return None;
    }
    let map_x = |x: i32| bx as f32 + (x - full.left) as f32 * kx;
    let map_y = |y: i32| by as f32 + (y - full.top) as f32 * ky;
    let x0 = map_x(visible.left).round().clamp(0.0, printed.width as f32) as u32;
    let y0 = map_y(visible.top).round().clamp(0.0, printed.height as f32) as u32;
    let x1 = map_x(visible.right)
        .round()
        .clamp(0.0, printed.width as f32) as u32;
    let y1 = map_y(visible.bottom)
        .round()
        .clamp(0.0, printed.height as f32) as u32;
    let cw = x1.saturating_sub(x0).min(printed.width.saturating_sub(x0));
    let ch = y1.saturating_sub(y0).min(printed.height.saturating_sub(y0));
    if cw < 16 || ch < 16 {
        return None;
    }
    Some(printed.crop(x0, y0, cw, ch))
}

/// The bounding box of everything that is not the sentinel colour, or `None`
/// when `PrintWindow` left the bitmap untouched.
fn painted_bounds(image: &Image) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 {
        return None;
    }
    let mut min_x = w;
    let mut max_x = 0u32;
    let mut min_y = h;
    let mut max_y = 0u32;
    for y in 0..h {
        let start = y as usize * w as usize * 4;
        let row = &image.rgba[start..start + w as usize * 4];
        let mut left = None;
        let mut right = 0u32;
        for (x, px) in row.chunks_exact(4).enumerate() {
            // `to_rgba` puts the sentinel out as R=255, G=0, B=255.
            if px[0] != UNPAINTED[2] || px[1] != UNPAINTED[1] || px[2] != UNPAINTED[0] {
                if left.is_none() {
                    left = Some(x as u32);
                }
                right = x as u32;
            }
        }
        if let Some(left) = left {
            min_x = min_x.min(left);
            max_x = max_x.max(right);
            min_y = min_y.min(y);
            max_y = y;
        }
    }
    if min_y >= h {
        return None;
    }
    Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

fn print_window(hwnd: HWND, width: u32, height: u32, flags: PRINT_WINDOW_FLAGS) -> Option<Image> {
    let mut dib = Dib::new(width, height)?;
    dib.fill(UNPAINTED[0], UNPAINTED[1], UNPAINTED[2], UNPAINTED[3]);
    let ok = unsafe { PrintWindow(hwnd, dib.hdc, flags) };
    if !ok.as_bool() {
        return None;
    }
    Some(Image {
        width,
        height,
        rgba: dib.to_rgba(false),
    })
}

/// A straight copy of the screen. Used when the window does not implement
/// `WM_PRINT`; it must be on screen at the time of the call.
///
/// The copy goes into the bitmap's own DC. [`Dib::new`] has already selected the
/// bitmap there; selecting it into a second memory DC is what used to leave the
/// copy in a stock 1×1 bitmap, which is why this strategy always returned a
/// blank picture.
pub fn screen_grab(rect: Rect) -> Option<Image> {
    let (w, h) = (rect.width().max(1), rect.height().max(1));
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let Some(dib) = Dib::new(w as u32, h as u32) else {
            let _ = ReleaseDC(None, screen);
            return None;
        };
        let copied = BitBlt(
            dib.hdc,
            0,
            0,
            w,
            h,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY,
        )
        .is_ok();
        let _ = ReleaseDC(None, screen);
        if !copied {
            return None;
        }
        Some(Image {
            width: w as u32,
            height: h as u32,
            rgba: dib.to_rgba(false),
        })
    }
}

/// A rectangle in screen coordinates, for the flight animation: where the real
/// window sits right now.
pub fn rect_now(hwnd: isize) -> Option<Rect> {
    window::rect_of(window::hwnd_of(hwnd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn image(width: u32, height: u32, sentinel: bool) -> Image {
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        if sentinel {
            for px in rgba.chunks_exact_mut(4) {
                px.copy_from_slice(&[255, 0, 255, 255]);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    fn painted(width: u32, height: u32, rgb: [u8; 3]) -> Image {
        let mut image = image(width, height, false);
        for px in image.rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        image
    }

    #[test]
    fn an_untouched_bitmap_has_no_picture() {
        assert_eq!(painted_bounds(&image(40, 30, true)), None);
        assert!(
            crop_visible(&image(40, 30, true), rect(0, 0, 40, 30), rect(0, 0, 40, 30)).is_none()
        );
    }

    #[test]
    fn a_dpi_scaled_print_is_cropped_in_the_targets_own_space() {
        // A DPI-unaware window on a 150% screen: 690x480 physical, but it
        // paints only 460x320. The visible frame is inset by the invisible
        // resize border, which has to be scaled with it.
        let full = rect(0, 0, 690, 480);
        let visible = rect(7, 0, 683, 473);
        let printed = image(460, 320, false);
        let cropped = crop_visible(&printed, full, visible).expect("a full bitmap is a picture");
        assert_eq!((cropped.width, cropped.height), (450, 315));
    }

    #[test]
    fn only_the_painted_part_is_used() {
        // Some windows paint a smaller rectangle than they are given. The
        // sentinel border says where the content stops; the crop follows it.
        let full = rect(0, 0, 100, 100);
        let visible = rect(5, 5, 95, 95);
        let mut printed = image(100, 100, true);
        // 80x60 of content in the top-left corner.
        for y in 0..60 {
            for x in 0..80 {
                let i = ((y * 100 + x) * 4) as usize;
                printed.rgba[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);
            }
        }
        let bounds = painted_bounds(&printed).unwrap();
        assert_eq!(bounds, (0, 0, 80, 60));
        let cropped = crop_visible(&printed, full, visible).unwrap();
        assert_eq!((cropped.width, cropped.height), (72, 54));
    }

    #[test]
    fn a_grab_is_clipped_to_the_desktop() {
        let desktop = rect(0, 0, 2560, 1440);
        // A maximized window sticks out past the screen edges by the resize
        // border; the grab must not carry the desktop that is not there.
        assert_eq!(
            clip(rect(-8, -8, 2568, 1408), desktop),
            Some(rect(0, 0, 2560, 1408))
        );
        // A window fully on the desktop is its own clip, so the caller can tell
        // that the grab covers the whole visible rectangle.
        assert_eq!(
            clip(rect(100, 100, 300, 200), desktop),
            Some(rect(100, 100, 300, 200))
        );
        // Past the edge there is nothing worth grabbing.
        assert_eq!(clip(rect(3000, 100, 3200, 200), desktop), None);
    }

    #[test]
    fn a_flat_print_is_the_window_when_the_screen_agrees() {
        // The print is smaller than the screen (a DPI-unaware window); the
        // comparison is by relative position, so the sizes do not have to match.
        let print = painted(200, 100, [255, 255, 255]);
        let mut screen = painted(400, 200, [252, 252, 252]);
        assert!(matches_screen(&print, &screen));
        // The real page behind the flat print: content where the print is white.
        for y in 80..120 {
            for x in 160..240 {
                let i = ((y * 400 + x) * 4) as usize;
                screen.rgba[i..i + 4].copy_from_slice(&[20, 40, 160, 255]);
            }
        }
        assert!(!matches_screen(&print, &screen));
    }

    #[test]
    fn a_flat_print_loses_to_a_screen_that_shows_content() {
        // The white-card bug: PrintWindow hands back a flat client while the
        // screen shows the page the user is looking at.
        let print = painted(200, 100, [255, 255, 255]);
        let mut screen = painted(200, 100, [255, 255, 255]);
        for y in 40..60 {
            for x in 40..160 {
                let i = ((y * 200 + x) * 4) as usize;
                screen.rgba[i..i + 4].copy_from_slice(&[30, 80, 200, 255]);
            }
        }
        let (image, via) =
            choose_flat(Some(print), Some(screen), true).expect("the screen is a picture");
        assert_eq!(via, "screen");
        assert_eq!(image.pixel(100, 50), [30, 80, 200, 255]);
    }

    #[test]
    fn a_flat_print_that_matches_the_screen_is_kept() {
        // A genuinely white window: the print is the better picture, because the
        // grab may include our own rope over the top of the window.
        let print = painted(200, 100, [250, 250, 250]);
        let screen = painted(400, 200, [255, 255, 255]);
        let (image, via) =
            choose_flat(Some(print), Some(screen), true).expect("a white window has a picture");
        assert_eq!(via, "print:flat");
        assert_eq!((image.width, image.height), (200, 100));
    }

    #[test]
    fn a_flat_print_survives_without_a_screen_only_when_it_is_trustworthy() {
        let print = painted(200, 100, [255, 255, 255]);
        // Off screen or unoccluded: the print is the window's own painting.
        assert_eq!(
            choose_flat(Some(print.clone()), None, true).map(|(_, via)| via),
            Some("print:flat")
        );
        // Covered: it is a print of something the user cannot see. Better no
        // picture than a white block.
        assert!(choose_flat(Some(print), None, false).is_none());
    }

    #[test]
    fn a_window_that_prints_nothing_falls_back_to_the_screen() {
        let screen = painted(200, 100, [10, 20, 30]);
        assert_eq!(
            choose_flat(None, Some(screen), true).map(|(_, via)| via),
            Some("screen")
        );
        assert!(choose_flat(None, None, true).is_none());
    }
}
