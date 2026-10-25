//! Late frame scheduling. Drawing right after the previous frame wastes most of a refresh: the sample is
//! old by the time the compositor latches it. We draw at `vblank + phase` instead, sampling input as late
//! as the compositor still takes the frame, and learn `phase` from presentation feedback: creep later
//! while frames land on time, back off below any phase that missed (docs/reports/phase-2.md).
//!
//! All times are presentation-clock ns (CLOCK_MONOTONIC on Hyprland).

const STEP_NS: u64 = 250_000;
const BACKOFF_NS: u64 = 1_000_000;
/// On-time frames before creeping one step later.
const STREAK: u32 = 60;
/// On-time frames after which a ceiling learnt from a miss is probed again (~30 s at 120 Hz).
const REPROBE: u32 = 3_600;
/// Late by at most this much and we still draw now rather than waiting a refresh.
const SLACK_NS: u64 = 500_000;

#[derive(Clone, Debug, Default)]
pub struct FrameScheduler {
    /// A recent vblank and the refresh period.
    grid: Option<(u64, u64)>,
    phase_ns: u64,
    /// Latest phase known to land on time, learnt from a miss.
    ceiling_ns: Option<u64>,
    /// Fewest refreshes between sample and scanout seen: the on-time pipeline depth.
    depth: Option<u64>,
    streak: u32,
    since_miss: u32,
}

impl FrameScheduler {
    pub fn phase_ns(&self) -> u64 {
        self.phase_ns
    }

    /// When to draw a frame wanted at `now`: `None` to draw now, `Some(t)` to wait until `t`.
    pub fn draw_at(&self, now: u64) -> Option<u64> {
        let (vblank, refresh) = self.grid?;
        // The grid extends both ways: `vblank` may be a scanout still ahead of `now`.
        let current = now - (now as i128 - vblank as i128).rem_euclid(refresh as i128) as u64;
        let target = current + self.phase_ns;
        if now <= target {
            Some(target)
        } else if now <= target + SLACK_NS {
            None
        } else {
            // Too late for this refresh: wait for the next one and sample fresher input then.
            Some(target + refresh)
        }
    }

    /// Feedback for a frame sampled at `sample` and shown at `presented`. `on_schedule` says it was
    /// drawn at the time `draw_at` asked for; only those frames tune the phase.
    pub fn on_presented(&mut self, sample: u64, presented: u64, refresh: u64, on_schedule: bool) {
        if refresh == 0 {
            return;
        }
        self.grid = Some((presented, refresh));
        if !on_schedule || presented < sample {
            return;
        }
        let depth = (presented - sample + self.phase_ns + refresh / 2) / refresh;
        let best = *self.depth.get_or_insert(depth);
        if depth < best {
            self.depth = Some(depth);
        }
        let limit = self.ceiling_ns.unwrap_or(u64::MAX).min(refresh / 2);
        if depth > best {
            // Missed the latch: never go this late again (until a re-probe), and step well back.
            self.ceiling_ns = Some(self.phase_ns.saturating_sub(STEP_NS));
            self.phase_ns = self.phase_ns.saturating_sub(BACKOFF_NS);
            self.streak = 0;
            self.since_miss = 0;
            return;
        }
        self.streak += 1;
        self.since_miss += 1;
        if self.streak >= STREAK && self.phase_ns + STEP_NS <= limit {
            self.phase_ns += STEP_NS;
            self.streak = 0;
        }
        if self.since_miss >= REPROBE
            && let Some(c) = &mut self.ceiling_ns
        {
            *c += STEP_NS;
            self.since_miss = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: u64 = 8_333_333;

    /// A compositor that takes a frame committed by `latch` after a vblank and shows it two refreshes
    /// after that vblank (what Phase 0 measured on Hyprland); later commits slip one refresh.
    fn run(latch: u64, render: u64, frames: usize) -> (FrameScheduler, Vec<bool>) {
        let mut s = FrameScheduler::default();
        let mut now = 1_000_000_000u64;
        let mut misses = Vec::new();
        for _ in 0..frames {
            let wanted = now;
            let (sample, on_schedule) = match s.draw_at(wanted) {
                Some(t) => (t, true),
                None => (wanted, s.grid.is_none()),
            };
            let vblank = sample / R * R;
            let commit = sample + render;
            let hit = commit - vblank <= latch;
            let presented = vblank + if hit { 2 } else { 3 } * R;
            misses.push(!hit);
            s.on_presented(sample, presented, R, on_schedule);
            now = vblank + R; // next frame wanted right at the next vblank, like a frame callback
        }
        (s, misses)
    }

    #[test]
    fn converges_just_inside_the_latch() {
        let latch = 4_000_000;
        let (s, misses) = run(latch, 500_000, 20_000);
        let phase = s.phase_ns();
        assert!(
            phase + 500_000 <= latch && phase + 2_000_000 >= latch,
            "phase {phase} ns for a {latch} ns latch"
        );
        let late = misses[5_000..].iter().filter(|m| **m).count();
        assert!(late <= 5, "{late} misses in 15,000 settled frames");
    }

    #[test]
    fn never_exceeds_half_a_refresh() {
        // A compositor that would take anything: we still stop at R/2.
        let (s, misses) = run(R, 100_000, 10_000);
        assert!(s.phase_ns() <= R / 2);
        assert!(misses.iter().all(|m| !m));
    }

    #[test]
    fn draws_now_without_feedback_and_waits_for_the_target_with_it() {
        let mut s = FrameScheduler::default();
        assert_eq!(s.draw_at(123), None);
        s.phase_ns = 3_000_000;
        s.grid = Some((10 * R, R));
        let v = 12 * R;
        assert_eq!(s.draw_at(v + 1_000_000), Some(v + 3_000_000));
        assert_eq!(s.draw_at(v + 3_200_000), None, "slightly late: draw now");
        assert_eq!(
            s.draw_at(v + 6_000_000),
            Some(v + R + 3_000_000),
            "too late: next refresh"
        );
    }
}
