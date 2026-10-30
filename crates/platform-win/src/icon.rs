//! Application icons, pulled out of the executable with the shell's own
//! extractor. The icon is what lets the user recognise "that other Chrome
//! window" before reading a single title.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use crate::Image;

/// Extracts the application's icon and scales it to `size` square.
pub fn for_exe(exe_path: &str, size: u32) -> Option<Image> {
    if exe_path.is_empty() {
        return None;
    }
    let wide: Vec<u16> = std::ffi::OsStr::new(exe_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut large = HICON::default();
        let mut small = HICON::default();
        let count = ExtractIconExW(
            PCWSTR(wide.as_ptr()),
            0,
            Some(&mut large as *mut HICON),
            Some(&mut small as *mut HICON),
            1,
        );
        if count == 0 {
            return None;
        }
        let icon = if size >= 32 || small.is_invalid() {
            large
        } else {
            small
        };
        if icon.is_invalid() {
            return None;
        }
        let image = from_icon(icon);
        let _ = DestroyIcon(large);
        let _ = DestroyIcon(small);
        image.map(|img| img.downscale(size, size))
    }
}

/// Decodes an `HICON` into RGBA. Icons are 32-bit BGRA with a real alpha
/// channel in practice; the 1-bit mask is only consulted when the alpha
/// channel comes back empty (old 16-colour icons).
fn from_icon(icon: HICON) -> Option<Image> {
    unsafe {
        let mut info = ICONINFO::default();
        if GetIconInfo(icon, &mut info).is_err() {
            return None;
        }
        let result = from_icon_bitmaps(&info);
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
        result
    }
}

fn from_icon_bitmaps(info: &ICONINFO) -> Option<Image> {
    if info.hbmColor.is_invalid() {
        return None;
    }
    unsafe {
        let mut bm = BITMAP::default();
        let got = windows::Win32::Graphics::Gdi::GetObjectW(
            info.hbmColor.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        );
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        let mut bi = BITMAPINFO {
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
        let screen = GetDC(None);
        let lines = GetDIBits(
            screen,
            info.hbmColor,
            0,
            h,
            Some(data.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = ReleaseDC(None, screen);
        if lines == 0 {
            return None;
        }
        // BGRA -> RGBA.
        for px in data.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let has_alpha = data.chunks_exact(4).any(|px| px[3] != 0);
        if !has_alpha {
            apply_mask(info, w, h, &mut data);
        }
        Some(Image {
            width: w,
            height: h,
            rgba: data,
        })
    }
}

/// Sets alpha from the icon's AND mask. Only used when the colour bitmap has
/// no alpha channel at all.
fn apply_mask(info: &ICONINFO, w: u32, h: u32, data: &mut [u8]) {
    if info.hbmMask.is_invalid() {
        for px in data.chunks_exact_mut(4) {
            px[3] = 255;
        }
        return;
    }
    unsafe {
        // 1 bit per pixel, rows padded to 32 bits.
        let stride = w.div_ceil(32) * 4;
        let mut mask = vec![0u8; (stride * h) as usize];
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 1,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let screen = GetDC(None);
        let lines = GetDIBits(
            screen,
            info.hbmMask,
            0,
            h,
            Some(mask.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = ReleaseDC(None, screen);
        if lines == 0 {
            for px in data.chunks_exact_mut(4) {
                px[3] = 255;
            }
            return;
        }
        for y in 0..h as usize {
            for x in 0..w as usize {
                let byte = mask[y * stride as usize + x / 8];
                let bit = (byte >> (7 - (x % 8))) & 1;
                let i = (y * w as usize + x) * 4;
                data[i + 3] = if bit == 1 { 0 } else { 255 };
            }
        }
    }
}
