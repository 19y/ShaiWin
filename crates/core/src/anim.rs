//! The flight path a card takes when it is hung up or taken back down.
//!
//! The window itself never flies: it is parked off screen the instant the
//! snapshot is taken. What the user sees is the snapshot travelling along a
//! lifted arc, growing or shrinking, until the clothespin takes over.
//! The same maths drives the reverse trip when the card is clicked.

/// A one-shot flight along a quadratic Bézier with a lifted control point.
#[derive(Clone, Copy, Debug)]
pub struct ArcFlight {
    pub from: (f32, f32),
    pub to: (f32, f32),
    /// How far above the straight line the arc bulges, in device pixels.
    pub lift: f32,
    /// Seconds.
    pub duration: f32,
    /// Elapsed seconds, 0..duration.
    pub t: f32,
}

impl ArcFlight {
    pub fn new(from: (f32, f32), to: (f32, f32), lift: f32, duration: f32) -> Self {
        ArcFlight { from, to, lift, duration: duration.max(0.05), t: 0.0 }
    }

    /// Advances the flight. Returns `true` while it is still in the air.
    pub fn step(&mut self, dt: f32) -> bool {
        self.t = (self.t + dt.max(0.0)).min(self.duration);
        self.t < self.duration
    }

    pub fn finished(&self) -> bool {
        self.t >= self.duration
    }

    /// Linear progress, 0..1.
    pub fn raw(&self) -> f32 {
        (self.t / self.duration).clamp(0.0, 1.0)
    }

    /// Eased progress: quick off the mark, gentle arrival.
    pub fn eased(&self) -> f32 {
        ease_out_cubic(self.raw())
    }

    /// Current position along the arc, in device pixels.
    pub fn position(&self) -> (f32, f32) {
        let t = self.eased();
        let (x0, y0) = self.from;
        let (x1, y1) = self.to;
        // Control point: halfway, lifted.
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0 - self.lift);
        let u = 1.0 - t;
        (u * u * x0 + 2.0 * u * t * cx + t * t * x1, u * u * y0 + 2.0 * u * t * cy + t * t * y1)
    }

    /// Rotation in degrees along the flight, so the card banks into the line.
    pub fn angle(&self, from: f32, to: f32) -> f32 {
        let t = self.eased();
        from + (to - from) * t
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// A window shrinking into the line should keep its aspect ratio readable;
/// this maps "window rect" to "card rect" while the flight runs.
pub fn lerp_rect(from: (f32, f32, f32, f32), to: (f32, f32, f32, f32), t: f32) -> (f32, f32, f32, f32) {
    let t = ease_in_out_cubic(t);
    (
        from.0 + (to.0 - from.0) * t,
        from.1 + (to.1 - from.1) * t,
        from.2 + (to.2 - from.2) * t,
        from.3 + (to.3 - from.3) * t,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_starts_and_ends_where_asked() {
        let mut f = ArcFlight::new((10.0, 20.0), (300.0, 60.0), 80.0, 0.4);
        assert_eq!(f.position(), (10.0, 20.0));
        while f.step(1.0 / 60.0) {}
        let (x, y) = f.position();
        assert!((x - 300.0).abs() < 0.5 && (y - 60.0).abs() < 0.5, "{x},{y}");
        assert!(f.finished());
    }

    #[test]
    fn flight_is_lifted() {
        let mut f = ArcFlight::new((0.0, 100.0), (200.0, 100.0), 120.0, 0.5);
        f.t = 0.25;
        let (_, y) = f.position();
        assert!(y < 100.0, "should rise above the straight line, got {y}");
    }
}
