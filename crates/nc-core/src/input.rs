//! Pen and mouse sample smoothing with the 1€ filter (Casiez et al. 2012), applied in screen px so the
//! feel does not change with zoom. Fast motion raises the cutoff, so ink keeps up with the hand; slow
//! motion lowers it, so tremor is smoothed out. Every sample still produces exactly one point.

use glam::Vec2;
use one_euro::nalgebra::Vector2;
use one_euro::{OneEuroFilter, OneEuroState};

/// Cutoff at rest, Hz: low enough to calm a hand tremor.
const MIN_CUTOFF: f64 = 1.0;
/// Cutoff rise per px/s of speed. Steady-state lag is bounded by 1 / (2π·BETA) px ≈ 1.6 px.
const BETA: f64 = 0.1;
/// Cutoff of the speed estimate, Hz. High, so the filter opens up within a few ms of a stroke starting.
const D_CUTOFF: f64 = 30.0;
/// Shortest sample interval used for the rate; coalesced events can share a timestamp.
const MIN_DT: f64 = 1e-4;

pub struct Smoother {
    filter: OneEuroFilter<f64>,
    state: Option<(OneEuroState<f64, 2>, f64)>,
}

impl Default for Smoother {
    fn default() -> Self {
        let mut filter = OneEuroFilter::default();
        filter.set_mincutoff(MIN_CUTOFF);
        filter.set_beta(BETA);
        filter.set_dcutoff(D_CUTOFF);
        Self {
            filter,
            state: None,
        }
    }
}

impl Smoother {
    /// Starts a new stroke.
    pub fn reset(&mut self) {
        self.state = None;
    }

    /// Filters one sample (screen px) taken at `time` seconds.
    pub fn smooth(&mut self, p: Vec2, time: f64) -> Vec2 {
        let raw = Vector2::new(p.x as f64, p.y as f64);
        match &mut self.state {
            None => {
                self.state = Some((OneEuroState::new(raw), time));
                p
            }
            Some((state, last)) => {
                let rate = 1.0 / (time - *last).max(MIN_DT);
                *last = time;
                self.filter.filter(state, &raw, rate);
                let out = state.data();
                Vec2::new(out.x as f32, out.y as f32)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Worst lag behind the raw pen over a stroke at constant speed, from `settle` s in.
    fn lag_at(speed: f32, settle: f64) -> f32 {
        let mut s = Smoother::default();
        let mut worst = 0f32;
        for i in 0..400 {
            let t = i as f64 / 1000.0; // 1 kHz mouse
            let raw = Vec2::new(speed * t as f32, 0.0);
            let out = s.smooth(raw, t);
            if t >= settle {
                worst = worst.max(raw.distance(out));
            }
        }
        worst
    }

    #[test]
    fn ink_keeps_up_at_every_speed() {
        for speed in [100.0, 500.0, 2_000.0, 5_000.0] {
            let lag = lag_at(speed, 0.1);
            assert!(lag < 2.0, "{speed} px/s lags {lag:.2} px after settling");
        }
    }

    #[test]
    fn stroke_start_does_not_trail() {
        // From 10 ms into a stroke, the tail is within a few px even at a brisk 2,000 px/s.
        let lag = lag_at(2_000.0, 0.01);
        assert!(lag < 4.0, "start of stroke lags {lag:.2} px");
    }

    #[test]
    fn tremor_is_smoothed() {
        let mut s = Smoother::default();
        let (mut raw_var, mut out_var) = (0f32, 0f32);
        for i in 0..1000 {
            let t = i as f64 / 1000.0;
            // ±1 px alternating jitter around a resting pen.
            let jitter = if i % 2 == 0 { 1.0 } else { -1.0 };
            let raw = Vec2::new(300.0 + jitter, 200.0 - jitter);
            let out = s.smooth(raw, t);
            if i > 100 {
                raw_var += (raw - Vec2::new(300.0, 200.0)).length_squared();
                out_var += (out - Vec2::new(300.0, 200.0)).length_squared();
            }
        }
        assert!(
            out_var < raw_var * 0.25,
            "jitter energy {out_var:.1} vs raw {raw_var:.1}"
        );
    }

    #[test]
    fn first_sample_is_exact_and_duplicate_times_are_finite() {
        let mut s = Smoother::default();
        assert_eq!(s.smooth(Vec2::new(5.0, 6.0), 1.0), Vec2::new(5.0, 6.0));
        assert!(s.smooth(Vec2::new(9.0, 6.0), 1.0).is_finite());
        s.reset();
        assert_eq!(s.smooth(Vec2::new(1.0, 1.0), 2.0), Vec2::new(1.0, 1.0));
    }
}
