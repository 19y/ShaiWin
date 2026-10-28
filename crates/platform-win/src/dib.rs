//! A tiny 32-bit top-down device-independent bitmap, shared by capture and
//! icon extraction. GDI gives us BGRA with an unreliable alpha byte; the
//! conversion to RGBA happens once, here.

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    HBITMAP, HDC, HGDIOBJ, SelectObject,
};

pub struct Dib {
    pub hdc: HDC,
    pub bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub width: u32,
    pub height: u32,
}

impl Dib {
    pub fn new(width: u32, height: u32) -> Option<Dib> {
        if width == 0 || height == 0 {
            return None;
        }
        unsafe {
            let hdc = CreateCompatibleDC(None);
            if hdc.is_invalid() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(hdc);
                return None;
            }
            let old = SelectObject(hdc, bitmap.into());
            Some(Dib { hdc, bitmap, old, bits: bits as *mut u8, width, height })
        }
    }

    /// The pixels as raw BGRA.
    pub fn bgra(&self) -> &[u8] {
        let len = self.width as usize * self.height as usize * 4;
        unsafe { std::slice::from_raw_parts(self.bits, len) }
    }

    /// Fills the whole bitmap with one colour, in B, G, R, A order. Capture
    /// clears to a sentinel before `PrintWindow` so the part the window did not
    /// paint can be told apart from real content.
    pub fn fill(&mut self, b: u8, g: u8, r: u8, a: u8) {
        let len = self.width as usize * self.height as usize * 4;
        let bits = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        for px in bits.chunks_exact_mut(4) {
            px.copy_from_slice(&[b, g, r, a]);
        }
    }

    /// Converts to opaque RGBA. `keep_alpha` is for icons, where the alpha
    /// channel is meaningful; snapshots are always opaque.
    pub fn to_rgba(&self, keep_alpha: bool) -> Vec<u8> {
        let src = self.bgra();
        let mut out = Vec::with_capacity(src.len());
        for px in src.chunks_exact(4) {
            out.push(px[2]);
            out.push(px[1]);
            out.push(px[0]);
            out.push(if keep_alpha { px[3] } else { 255 });
        }
        out
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

#[allow(dead_code)]
fn _assert_hwnd(_: HWND) {}
