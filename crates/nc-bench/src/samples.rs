//! Cost of one input sample on the hot path: 1€ smoothing, screen → world, append to the live stroke
//! (Mac bug #16 guard, docs/PLAN.md §4: < 1 µs per sample).

use glam::Vec2;
use nc_core::{Smoother, Stroke, Viewport};
use std::hint::black_box;
use std::time::Instant;

pub fn run(samples: usize) {
    let viewport = Viewport {
        offset: Vec2::new(-200.0, 50.0),
        scale: 1.7,
    };
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let mut smoother = Smoother::default();
        let mut stroke = Stroke::with_pressure(Vec2::ZERO, 0.5, 2.5, [1.0; 4]);
        stroke.points.reserve(samples);
        stroke.pressure.reserve(samples);
        let t0 = Instant::now();
        for i in 0..samples {
            let t = i as f64 / 1000.0;
            let raw = Vec2::new(
                (t * 3.0).cos() as f32 * 400.0,
                (t * 2.0).sin() as f32 * 300.0,
            ) + Vec2::splat(i as f32 * 0.01);
            let p = smoother.smooth(black_box(raw), t);
            stroke.push_sample(viewport.screen_to_world(p), Some(0.5));
        }
        black_box(&stroke);
        best = best.min(t0.elapsed().as_secs_f64() * 1e9 / samples as f64);
    }
    println!(
        "input sample (smooth + to world + append with pressure): {best:.0} ns (best of 5 × {samples})"
    );
}
