//! A plain RGBA image. The platform layer never gives the renderer a raw DIB:
//! everything is normalised here so the rest of the app only sees pixels.

/// Non-premultiplied 8-bit RGBA, top-down rows, tightly packed.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Image {
            width,
            height,
            rgba: vec![0; (width as usize * height as usize * 4).max(4)],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let i = ((y as usize * self.width as usize) + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    /// True when every sampled pixel is the same colour. PrintWindow returns an
    /// all-white or all-black bitmap for windows that do not implement
    /// `WM_PRINT`, and this is how we notice before showing a blank card.
    pub fn looks_blank(&self) -> bool {
        if self.is_empty() {
            return true;
        }
        let first = self.pixel(0, 0);
        let (w, h) = (self.width, self.height);
        let step_x = (w / 24).max(1);
        let step_y = (h / 24).max(1);
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let p = self.pixel(x, y);
                if p[0].abs_diff(first[0]) > 6
                    || p[1].abs_diff(first[1]) > 6
                    || p[2].abs_diff(first[2]) > 6
                {
                    return false;
                }
                x += step_x;
            }
            y += step_y;
        }
        true
    }

    /// True when the middle half of the picture is one colour.
    ///
    /// Chromium and Electron windows can answer `PrintWindow` with their frame
    /// painted and a flat client background where the page should be. The frame
    /// makes the whole picture non-uniform, so [`looks_blank`] alone accepts a
    /// card that is blank exactly where the content belongs. The middle half is
    /// the part the card actually shows; if it is flat, the shot is not a
    /// picture and the caller can try another capture strategy.
    pub fn looks_blank_center(&self) -> bool {
        if self.is_empty() {
            return true;
        }
        let x0 = self.width / 4;
        let y0 = self.height / 4;
        let w = self.width.saturating_sub(x0 * 2).max(1);
        let h = self.height.saturating_sub(y0 * 2).max(1);
        let first = self.pixel(x0, y0);
        let step_x = (w / 24).max(1);
        let step_y = (h / 24).max(1);
        let mut y = y0;
        while y < y0 + h {
            let mut x = x0;
            while x < x0 + w {
                let p = self.pixel(x, y);
                if p[0].abs_diff(first[0]) > 6
                    || p[1].abs_diff(first[1]) > 6
                    || p[2].abs_diff(first[2]) > 6
                {
                    return false;
                }
                x += step_x;
            }
            y += step_y;
        }
        true
    }

    /// Crops the transparent border DWM adds around some windows.
    pub fn trim_uniform_border(&self, tolerance: u8) -> Image {
        if self.is_empty() {
            return self.clone();
        }
        let bg = self.pixel(0, 0);
        let same = |p: [u8; 4]| {
            p[0].abs_diff(bg[0]) <= tolerance
                && p[1].abs_diff(bg[1]) <= tolerance
                && p[2].abs_diff(bg[2]) <= tolerance
        };
        let (w, h) = (self.width, self.height);
        let mut left = 0;
        while left < w && (0..h).all(|y| same(self.pixel(left, y))) {
            left += 1;
        }
        let mut top = 0;
        while top < h && (0..w).all(|x| same(self.pixel(x, top))) {
            top += 1;
        }
        if left >= w || top >= h {
            return self.clone();
        }
        let mut right = w - 1;
        while right > left && (0..h).all(|y| same(self.pixel(right, y))) {
            right -= 1;
        }
        let mut bottom = h - 1;
        while bottom > top && (0..w).all(|x| same(self.pixel(x, bottom))) {
            bottom -= 1;
        }
        self.crop(left, top, right - left + 1, bottom - top + 1)
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Image {
        let w = w.min(self.width.saturating_sub(x)).max(1);
        let h = h.min(self.height.saturating_sub(y)).max(1);
        let mut out = Image::new(w, h);
        for row in 0..h {
            let src = (((y + row) as usize * self.width as usize) + x as usize) * 4;
            let dst = row as usize * w as usize * 4;
            out.rgba[dst..dst + w as usize * 4]
                .copy_from_slice(&self.rgba[src..src + w as usize * 4]);
        }
        out
    }

    /// A high-quality-enough box downscale. Used when the snapshot is larger
    /// than anything the card will ever draw.
    pub fn downscale(&self, max_w: u32, max_h: u32) -> Image {
        if self.is_empty() || (self.width <= max_w && self.height <= max_h) {
            return self.clone();
        }
        let k = (max_w as f32 / self.width as f32).min(max_h as f32 / self.height as f32);
        let nw = ((self.width as f32 * k).round() as u32).max(1);
        let nh = ((self.height as f32 * k).round() as u32).max(1);
        let mut out = Image::new(nw, nh);
        for y in 0..nh {
            let sy0 = (y as f32 / k) as u32;
            let sy1 = (((y + 1) as f32 / k) as u32).clamp(sy0 + 1, self.height);
            for x in 0..nw {
                let sx0 = (x as f32 / k) as u32;
                let sx1 = (((x + 1) as f32 / k) as u32).clamp(sx0 + 1, self.width);
                let mut acc = [0u32; 4];
                let mut n = 0u32;
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let p = self.pixel(sx, sy);
                        acc[0] += p[0] as u32;
                        acc[1] += p[1] as u32;
                        acc[2] += p[2] as u32;
                        acc[3] += p[3] as u32;
                        n += 1;
                    }
                }
                let i = ((y as usize * nw as usize) + x as usize) * 4;
                out.rgba[i] = (acc[0] / n) as u8;
                out.rgba[i + 1] = (acc[1] / n) as u8;
                out.rgba[i + 2] = (acc[2] / n) as u8;
                out.rgba[i + 3] = (acc[3] / n) as u8;
            }
        }
        out
    }

    /// Converts to the premultiplied RGBA tiny-skia expects.
    pub fn to_premultiplied_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for px in self.rgba.chunks_exact(4) {
            let a = px[3] as u32;
            out.push((px[0] as u32 * a / 255) as u8);
            out.push((px[1] as u32 * a / 255) as u8);
            out.push((px[2] as u32 * a / 255) as u8);
            out.push(px[3]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(width: u32, height: u32) -> Image {
        let mut image = Image::new(width, height);
        for px in image.rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 255, 255, 255]);
        }
        image
    }

    fn fill(image: &mut Image, x: u32, y: u32, w: u32, h: u32, rgb: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                let i = ((yy * image.width + xx) * 4) as usize;
                image.rgba[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }

    #[test]
    fn a_blank_picture_is_blank_everywhere() {
        let image = white(40, 30);
        assert!(image.looks_blank());
        assert!(image.looks_blank_center());
    }

    #[test]
    fn a_frame_does_not_hide_a_blank_client() {
        // What a failed Chromium PrintWindow looks like: a painted frame around
        // a flat white page. The whole picture is not uniform, but the part the
        // card shows is.
        let mut image = white(100, 80);
        fill(&mut image, 0, 0, 100, 4, [30, 30, 30]);
        fill(&mut image, 0, 76, 100, 4, [30, 30, 30]);
        fill(&mut image, 0, 0, 4, 80, [30, 30, 30]);
        fill(&mut image, 96, 0, 4, 80, [30, 30, 30]);
        assert!(!image.looks_blank());
        assert!(image.looks_blank_center());
    }

    #[test]
    fn real_content_in_the_centre_is_a_picture() {
        let mut image = white(100, 80);
        fill(&mut image, 0, 0, 100, 4, [30, 30, 30]);
        fill(&mut image, 40, 30, 20, 20, [200, 40, 40]);
        assert!(!image.looks_blank());
        assert!(!image.looks_blank_center());
    }
}
