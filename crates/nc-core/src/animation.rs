//! Animated view changes (Zoom to Fit, Actual Size): ~180 ms ease-out, never blocking input. With reduced
//! motion the view jumps straight to the target (docs/UX-PRINCIPLES.md §3).

use crate::Viewport;
use glam::Vec2;

pub const VIEW_ANIMATION_SECS: f64 = 0.18;

#[derive(Clone, Copy, Debug)]
pub struct ViewAnimation {
    from: Viewport,
    to: Viewport,
    start: f64,
    /// Logical px; the animation moves the world point at the view centre.
    screen: Vec2,
}

impl ViewAnimation {
    /// `None` when there is nothing to animate (reduced motion, or already there): set `to` directly.
    pub fn new(
        from: Viewport,
        to: Viewport,
        start: f64,
        screen: Vec2,
        reduced_motion: bool,
    ) -> Option<Self> {
        (!reduced_motion && from != to).then_some(Self {
            from,
            to,
            start,
            screen,
        })
    }

    /// The view at `now` (seconds), and whether the animation has finished.
    pub fn at(&self, now: f64) -> (Viewport, bool) {
        if now >= self.start + VIEW_ANIMATION_SECS {
            return (self.to, true);
        }
        let t = ((now - self.start) / VIEW_ANIMATION_SECS).max(0.0);
        let e = 1.0 - (1.0 - t).powi(3); // cubic ease-out
        let mid = self.screen * 0.5;
        let (ca, cb) = (self.from.screen_to_world(mid), self.to.screen_to_world(mid));
        // Zoom moves geometrically, so a 0.1× → 32× fit feels as even as 1× → 2×.
        let scale =
            (self.from.scale.ln() + (self.to.scale.ln() - self.from.scale.ln()) * e as f32).exp();
        let center = ca.lerp(cb, e as f32);
        (
            Viewport {
                scale,
                offset: mid - center * scale,
            },
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn views() -> (Viewport, Viewport) {
        let from = Viewport {
            offset: Vec2::new(10.0, 20.0),
            scale: 1.0,
        };
        let to = Viewport {
            offset: Vec2::new(-400.0, 300.0),
            scale: 4.0,
        };
        (from, to)
    }

    #[test]
    fn eases_out_from_start_to_exact_target() {
        let (from, to) = views();
        let screen = Vec2::new(1280.0, 800.0);
        let a = ViewAnimation::new(from, to, 10.0, screen, false).unwrap();
        let (v0, done0) = a.at(10.0);
        assert!(
            !done0 && (v0.offset - from.offset).length() < 1e-3 && (v0.scale - 1.0).abs() < 1e-6
        );
        let (v_end, done) = a.at(10.0 + VIEW_ANIMATION_SECS);
        assert!(done);
        assert_eq!(v_end, to, "lands exactly on the target");
        // Ease-out: more than half of the zoom (in log space) is done at half time.
        let (half, _) = a.at(10.0 + VIEW_ANIMATION_SECS / 2.0);
        assert!(half.scale.ln() > 0.5 * 4f32.ln());
        // Monotonic zoom.
        let scales: Vec<f32> = (0..=18)
            .map(|i| a.at(10.0 + i as f64 * 0.01).0.scale)
            .collect();
        assert!(scales.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn reduced_motion_jumps() {
        let (from, to) = views();
        assert!(ViewAnimation::new(from, to, 0.0, Vec2::splat(500.0), true).is_none());
        assert!(ViewAnimation::new(from, from, 0.0, Vec2::splat(500.0), false).is_none());
    }
}
