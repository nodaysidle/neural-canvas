//! Deterministic stress scenes for benchmarks and perf tests (docs/PERFORMANCE-BUDGET.md, "S2k").

use crate::{Document, Stroke};
use glam::Vec2;
use kurbo::{CubicBez, ParamCurve, Point};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

pub const S2K_STROKES: usize = 2_000;
pub const S2K_POINTS_PER_STROKE: usize = 100;
/// World area the scene is scattered over, in logical px (a few screens of whiteboard at 100%).
pub const S2K_EXTENT: Vec2 = Vec2::new(6_000.0, 4_000.0);

const INK: [f32; 4] = [0.85, 0.87, 0.93, 1.0];

/// The S2k scene: 2,000 hand-drawn-looking strokes, 200k points. Same seed, same document,
/// on every platform (ChaCha8 is portable and its output is stable across `rand_chacha` versions).
pub fn s2k(seed: u64) -> Document {
    scene(seed, S2K_STROKES, S2K_POINTS_PER_STROKE)
}

/// `strokes` cubic-Bézier strokes of exactly `points_per_stroke` points each.
pub fn scene(seed: u64, strokes: usize, points_per_stroke: usize) -> Document {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut doc = Document::default();
    for _ in 0..strokes {
        let origin = Point::new(
            rng.random_range(0.0..S2K_EXTENT.x as f64),
            rng.random_range(0.0..S2K_EXTENT.y as f64),
        );
        let size = rng.random_range(20.0..600.0);
        let mut ctrl = || {
            origin + kurbo::Vec2::new(rng.random_range(-size..size), rng.random_range(-size..size))
        };
        let curve = CubicBez::new(origin, ctrl(), ctrl(), ctrl());
        let at = |i: usize| {
            let p = curve.eval(i as f64 / (points_per_stroke - 1).max(1) as f64);
            Vec2::new(p.x as f32, p.y as f32)
        };
        let mut stroke = Stroke::new(at(0), rng.random_range(1.5..4.0), INK);
        for i in 1..points_per_stroke {
            stroke.push(at(i));
        }
        doc.strokes.push(stroke);
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s2k_has_the_budgeted_size() {
        let doc = s2k(1);
        assert_eq!(doc.strokes.len(), 2_000);
        let points: usize = doc.strokes.iter().map(|s| s.points.len()).sum();
        assert_eq!(points, 200_000, "no sample may be dropped as a duplicate");
        assert!(
            doc.strokes
                .iter()
                .flat_map(|s| &s.points)
                .all(|p| p.is_finite())
        );
    }

    #[test]
    fn same_seed_same_scene_other_seed_differs() {
        let pts = |seed| -> Vec<Vec2> {
            scene(seed, 50, 20)
                .strokes
                .into_iter()
                .flat_map(|s| s.points)
                .collect()
        };
        assert_eq!(pts(7), pts(7));
        assert_ne!(pts(7), pts(8));
    }
}
