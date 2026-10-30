//! Text for the labels, drawn through GDI.
//!
//! The labels are the only text in the app. Rasterising them used to mean
//! parsing Microsoft YaHei in-process, which handed the rasteriser an outline
//! for every glyph in the collection: about 155 MB of private memory held for
//! a handful of card captions. GDI uses the font data the system already has,
//! and gives back the same coverage bitmap the labels need.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush,
    GetTextExtentPoint32W, GetTextMetricsW, SelectObject, SetBkMode, SetTextColor, TextOutW,
    ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, HDC,
    HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, TEXTMETRICW, TRANSPARENT,
};

const FACE: windows::core::PCWSTR = w!("Microsoft YaHei UI");
const FALLBACK_FACE: windows::core::PCWSTR = w!("Segoe UI");
const PAD: i32 = 2;

/// One memory DC and one font per size, kept for the life of the app. Creating
/// a font is cheap but not free, and a card asks for the same two sizes on
/// every rebuilt sprite.
pub struct Text {
    dc: HDC,
    /// Keyed by device size and weight: a tag label is semibold while every
    /// other caption is regular, and they must not evict each other.
    fonts: HashMap<(i32, i32), (HFONT, i32)>,
}

impl Text {
    pub fn new() -> Option<Text> {
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return None;
        }
        Some(Text {
            dc,
            fonts: HashMap::new(),
        })
    }

    /// The font for `px` and the height of its cell. Selecting it into the DC
    /// is part of getting it, because both measuring and drawing read the
    /// selected font.
    fn font(&mut self, px: f32, weight: i32) -> Option<i32> {
        let key = (px.round().max(1.0) as i32, weight);
        if let Some((font, cell)) = self.fonts.get(&key) {
            unsafe {
                SelectObject(self.dc, HGDIOBJ(font.0));
            }
            return Some(*cell);
        }
        let mut font = unsafe { make_font(key.0, weight, FACE) };
        if font.is_invalid() {
            font = unsafe { make_font(key.0, weight, FALLBACK_FACE) };
        }
        if font.is_invalid() {
            return None;
        }
        unsafe {
            SelectObject(self.dc, HGDIOBJ(font.0));
        }
        let mut metrics = TEXTMETRICW::default();
        if !unsafe { GetTextMetricsW(self.dc, &mut metrics) }.as_bool() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
            return None;
        }
        self.fonts.insert(key, (font, metrics.tmHeight));
        Some(metrics.tmHeight)
    }

    pub fn width(&mut self, text: &str, px: f32) -> Option<f32> {
        self.width_weight(text, px, FW_NORMAL.0 as i32)
    }

    pub fn width_bold(&mut self, text: &str, px: f32) -> Option<f32> {
        self.width_weight(text, px, FW_SEMIBOLD.0 as i32)
    }

    fn width_weight(&mut self, text: &str, px: f32, weight: i32) -> Option<f32> {
        self.font(px, weight)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        unsafe {
            let _ = GetTextExtentPoint32W(self.dc, &wide, &mut size);
        }
        Some(size.cx as f32)
    }

    /// `text` as a premultiplied RGBA image: white glyphs on transparent,
    /// tinted to `rgb`.
    pub fn render(
        &mut self,
        text: &str,
        px: f32,
        rgb: (u8, u8, u8),
    ) -> Option<(Vec<u8>, u32, u32)> {
        self.render_weight(text, px, rgb, FW_NORMAL.0 as i32)
    }

    pub fn render_bold(
        &mut self,
        text: &str,
        px: f32,
        rgb: (u8, u8, u8),
    ) -> Option<(Vec<u8>, u32, u32)> {
        self.render_weight(text, px, rgb, FW_SEMIBOLD.0 as i32)
    }

    fn render_weight(
        &mut self,
        text: &str,
        px: f32,
        rgb: (u8, u8, u8),
        weight: i32,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let cell = self.font(px, weight)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        unsafe {
            let _ = GetTextExtentPoint32W(self.dc, &wide, &mut size);
        }
        let width = size.cx.max(1) as u32 + PAD as u32 * 2;
        let height = cell.max(1) as u32 + PAD as u32 * 2;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Top-down: row zero is the first scanline, the same order a
                // Pixmap uses.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = ptr::null_mut();
        let dib =
            unsafe { CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .ok()?;
        if dib.is_invalid() || bits.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(dib.0));
            }
            return None;
        }
        let old = unsafe { SelectObject(self.dc, HGDIOBJ(dib.0)) };
        unsafe {
            ptr::write_bytes(bits as *mut u8, 0, (width * height) as usize * 4);
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(0x00FF_FFFF));
            let _ = TextOutW(self.dc, PAD, PAD, &wide);
            let _ = GdiFlush();
        }

        // White on black: the red channel is the coverage of the glyph, which
        // is all premultiplied text needs.
        let mut out = vec![0u8; (width * height) as usize * 4];
        unsafe {
            let src = bits as *const u8;
            for i in 0..(width * height) as usize {
                let coverage = *src.add(i * 4 + 2);
                if coverage == 0 {
                    continue;
                }
                let f = coverage as f32 / 255.0;
                out[i * 4] = (rgb.0 as f32 * f).round() as u8;
                out[i * 4 + 1] = (rgb.1 as f32 * f).round() as u8;
                out[i * 4 + 2] = (rgb.2 as f32 * f).round() as u8;
                out[i * 4 + 3] = coverage;
            }
            SelectObject(self.dc, old);
            let _ = DeleteObject(HGDIOBJ(dib.0));
        }
        Some((out, width, height))
    }
}

unsafe fn make_font(px: i32, weight: i32, face: windows::core::PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            -px,
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
            ANTIALIASED_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            face,
        )
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        unsafe {
            for (font, _) in self.fonts.values() {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
            let _ = DeleteDC(self.dc);
        }
    }
}
