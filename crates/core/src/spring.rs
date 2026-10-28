//! Damped springs: the only thing in ShaiWin that moves without a keyframe.

/// A value that chases a target with a spring.
///
/// `k` is stiffness, `c` is damping. Use [`Spring::responsive`] to build one
/// from the two numbers a designer actually thinks in: how long the motion
/// takes and how much it overshoots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
    pub target: f32,
    pub k: f32,
    pub c: f32,
}

/// Fixed integration step. Substepping at 120 Hz keeps stiff springs stable
/// even when a frame takes 60 ms (a busy desktop, a laptop on battery).
const SUBSTEP: f32 = 1.0 / 120.0;

impl Spring {
    pub fn new(x: f32, k: f32, c: f32) -> Self {
        Spring { x, v: 0.0, target: x, k, c }
    }

    /// A spring from a response (seconds, roughly the time to settle) and a
    /// damping ratio (1.0 is critically damped, 0.7 overshoots a little).
    pub fn responsive(x: f32, response: f32, damping: f32) -> Self {
        let response = response.max(0.02);
        let omega = std::f32::consts::TAU / response;
        Spring::new(x, omega * omega, 2.0 * damping.max(0.01) * omega)
    }

    /// A critically damped spring that never overshoots — for things that must
    /// not wobble, such as the line panel sliding in.
    pub fn smooth(x: f32, response: f32) -> Self {
        Spring::responsive(x, response, 1.0)
    }

    pub fn snap(&mut self, x: f32) {
        self.x = x;
        self.v = 0.0;
        self.target = x;
    }

    pub fn retarget(&mut self, target: f32) {
        self.target = target;
    }

    /// Stop wherever the target is, instantly. For the moments where a value
    /// must be exactly right and a slide would read as a glitch — a card that
    /// has just landed on the line, for example.
    pub fn settle(&mut self) {
        self.snap(self.target);
    }

    /// Advances by `dt` seconds, in fixed substeps.
    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        // A tab that was asleep must not blow the spring up.
        let dt = dt.min(0.1);
        let substeps = (dt / SUBSTEP).ceil().max(1.0) as u32;
        let h = dt / substeps as f32;
        for _ in 0..substeps {
            let a = -self.k * (self.x - self.target) - self.c * self.v;
            self.v += a * h;
            self.x += self.v * h;
        }
        if self.x.is_finite() && self.v.is_finite() {
            // Settle exactly once we are close enough; floating point would
            // otherwise keep a redraw loop alive forever. The thresholds are what
            // a pixel (or a degree of tilt) cannot show: 1/500 of a pixel moving
            // at 1/50 of a pixel a second.
            if (self.x - self.target).abs() < 0.002 && self.v.abs() < 0.02 {
                self.x = self.target;
                self.v = 0.0;
            }
        } else {
            self.snap(self.target);
        }
    }

    pub fn at_rest(&self) -> bool {
        self.x == self.target && self.v == 0.0
    }

    /// Progress from a start value to the target, 0..1. Handy for mapping a
    /// spring onto opacity or scale.
    pub fn progress(&self, from: f32) -> f32 {
        let span = self.target - from;
        if span.abs() < f32::EPSILON {
            1.0
        } else {
            ((self.x - from) / span).clamp(0.0, 1.0)
        }
    }
}

/// Exponential approach, framerate independent. Not springy on purpose: hover
/// and press feedback should feel attentive, not bouncy.
pub fn approach(current: f32, target: f32, dt: f32, rate: f32) -> f32 {
    current + (target - current) * (1.0 - (-dt * rate).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges() {
        let mut s = Spring::responsive(0.0, 0.4, 0.8);
        s.retarget(10.0);
        for _ in 0..200 {
            s.step(1.0 / 60.0);
        }
        assert!((s.x - 10.0).abs() < 0.01, "x = {}", s.x);
        assert!(s.at_rest());
    }

    #[test]
    fn frozen_springs_never_reach_a_new_target() {
        // The trap worth remembering: `Spring::new(x, 0.0, 0.0)` has nothing to
        // move it. Retarget one and it stays put forever while `at_rest` says no,
        // which is how the line ended up redrawing at 60Hz for the whole session.
        let mut s = Spring::new(10.0, 0.0, 0.0);
        s.retarget(20.0);
        for _ in 0..600 {
            s.step(1.0 / 60.0);
        }
        assert_eq!(s.x, 10.0);
        assert!(!s.at_rest());
    }

    #[test]
    fn huge_dt_stays_finite() {
        let mut s = Spring::responsive(0.0, 0.3, 0.5);
        s.retarget(100.0);
        s.step(5.0);
        assert!(s.x.is_finite());
        assert!(s.x.abs() < 1000.0);
    }

    #[test]
    fn progress_maps_to_unit() {
        let mut s = Spring::responsive(0.0, 0.2, 1.0);
        s.retarget(1.0);
        s.x = 0.25;
        assert!((s.progress(0.0) - 0.25).abs() < 1e-6);
    }
}
