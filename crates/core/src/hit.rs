//! Plain rectangles. Screen coordinates are device pixels, origin top-left,
//! exactly like Win32 and like a flipped AppKit view.

/// A rectangle; `x`/`y` is the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn from_xyxy(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(&self, by: f32) -> Rect {
        Rect {
            x: self.x + by,
            y: self.y + by,
            w: (self.w - by * 2.0).max(0.0),
            h: (self.h - by * 2.0).max(0.0),
        }
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::from_xyxy(x0, y0, x1, y1)
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.5 || self.h <= 0.5
    }

    /// Shrinks the rectangle to fit inside `bounds`, keeping it whole when it
    /// is small enough. Used to keep a card's ghost on the monitor.
    pub fn clamp_inside(&self, bounds: &Rect) -> Rect {
        let w = self.w.min(bounds.w);
        let h = self.h.min(bounds.h);
        Rect {
            x: self.x.clamp(bounds.x, bounds.right() - w),
            y: self.y.clamp(bounds.y, bounds.bottom() - h),
            w,
            h,
        }
    }
}

/// The first rectangle containing the point, searched back to front so the
/// topmost item wins.
pub fn hit_test<T>(items: &[(T, Rect)], x: f32, y: f32) -> Option<&T> {
    items
        .iter()
        .rev()
        .find(|(_, r)| r.contains(x, y))
        .map(|(t, _)| t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_uses_half_open_edges() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 0.0));
        assert!(r.contains(9.9, 9.9));
        assert!(!r.contains(10.0, 5.0));
    }

    #[test]
    fn hit_test_prefers_last() {
        let items = [
            ("a", Rect::new(0.0, 0.0, 10.0, 10.0)),
            ("b", Rect::new(0.0, 0.0, 10.0, 10.0)),
        ];
        assert_eq!(hit_test(&items, 5.0, 5.0), Some(&"b"));
    }

    #[test]
    fn clamp_keeps_inside() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 50.0);
        let r = Rect::new(90.0, 40.0, 30.0, 30.0).clamp_inside(&bounds);
        assert_eq!(r, Rect::new(70.0, 20.0, 30.0, 30.0));
    }
}
