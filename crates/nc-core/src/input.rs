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

/// Velocity is measured over this much recent input, seconds.
const PREDICT_WINDOW: f64 = 0.025;
/// No prediction once the pointer has been still this long: the hand stopped.
const PREDICT_STALE: f64 = 0.03;
/// Longest predicted lead, screen px: a mis-prediction never draws a visible spike.
const MAX_LEAD: f32 = 24.0;

/// Predicted ink: extends the live stroke along its current velocity by about one refresh, so what
/// reaches the screen matches where the pen is rather than where it was when the frame was drawn. The
/// prediction is drawn, never stored; the next real sample replaces it.
#[derive(Default)]
pub struct Predictor {
    /// Recent (time, smoothed screen position), oldest first.
    samples: [(f64, Vec2); 8],
    len: usize,
}

impl Predictor {
    pub fn reset(&mut self) {
        self.len = 0;
    }

    pub fn push(&mut self, time: f64, p: Vec2) {
        if self.len == self.samples.len() {
            self.samples.rotate_left(1);
            self.len -= 1;
        }
        self.samples[self.len] = (time, p);
        self.len += 1;
    }

    /// Where the pointer will be `horizon` seconds after its latest sample, given that it is `now`.
    pub fn predict(&self, now: f64, horizon: f64) -> Option<Vec2> {
        let recent = &self.samples[..self.len];
        let &(t1, p1) = recent.last()?;
        if now - t1 > PREDICT_STALE {
            return None;
        }
        let &(t0, p0) = recent.iter().find(|(t, _)| t1 - t <= PREDICT_WINDOW)?;
        if t1 - t0 < 1e-3 {
            return None;
        }
        let lead = (p1 - p0) / (t1 - t0) as f32 * horizon as f32;
        Some(p1 + lead.clamp_length_max(MAX_LEAD))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prediction_extends_steady_motion_and_stops_with_the_hand() {
        let mut p = Predictor::default();
        assert_eq!(p.predict(0.0, 0.008), None);
        // 1,000 px/s to the right, sampled at 1 kHz.
        for i in 0..20 {
            p.push(i as f64 * 1e-3, Vec2::new(i as f32, 50.0));
        }
        let ahead = p.predict(0.019, 0.008).unwrap();
        assert!((ahead - Vec2::new(27.0, 50.0)).length() < 1e-3, "{ahead}");
        assert_eq!(p.predict(0.06, 0.008), None, "still for 41 ms");
        // A flick at 20,000 px/s leads by at most MAX_LEAD.
        p.reset();
        p.push(0.0, Vec2::ZERO);
        p.push(0.005, Vec2::new(100.0, 0.0));
        assert_eq!(p.predict(0.005, 0.008), Some(Vec2::new(124.0, 0.0)));
    }

    #[test]
    fn prediction_error_on_a_curve_is_small() {
        // A 200 px radius circle at 1,500 px/s (a brisk stroke), 1 kHz samples, 8.3 ms horizon.
        let (r, speed, horizon) = (200.0_f64, 1500.0_f64, 1.0 / 120.0);
        let at = |t: f64| {
            let a = t * speed / r;
            Vec2::new((r * a.cos()) as f32, (r * a.sin()) as f32)
        };
        let mut p = Predictor::default();
        let mut worst = 0.0_f32;
        for i in 0..400 {
            let t = i as f64 * 1e-3;
            p.push(t, at(t));
            if i > 30 {
                let err = p.predict(t, horizon).unwrap().distance(at(t + horizon));
                worst = worst.max(err);
            }
        }
        // Lead is 12.5 px; linear extrapolation misses the curve by about a third of a px.
        assert!(worst < 1.0, "worst error {worst} px");
    }

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
