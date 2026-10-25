//! Pure document and viewport logic. No GPU, no windowing: everything here is unit-testable headless.
//!
//! World space is y-down, origin top-left, 1 unit = 1 logical px at 100% zoom (docs/PLAN.md §2).

pub mod scene;

use glam::Vec2;

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 32.0;

/// Maps world space to logical screen space: `screen = world * scale + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub offset: Vec2,
    pub scale: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            scale: 1.0,
        }
    }
}

impl Viewport {
    pub fn world_to_screen(&self, p: Vec2) -> Vec2 {
        p * self.scale + self.offset
    }

    pub fn screen_to_world(&self, s: Vec2) -> Vec2 {
        (s - self.offset) / self.scale
    }

    /// Content follows the hand: dragging right moves content right.
    pub fn pan_by_screen(&mut self, delta: Vec2) {
        self.offset += delta;
    }

    /// Zooms by `factor`, keeping the world point under `anchor` (logical screen px) fixed.
    pub fn zoom_at(&mut self, anchor: Vec2, factor: f32) {
        let world = self.screen_to_world(anchor);
        self.scale = (self.scale * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = anchor - world * self.scale;
    }
}

#[derive(Clone, Debug)]
pub struct Stroke {
    pub points: Vec<Vec2>,
    pub width: f32,
    /// Linear RGBA.
    pub color: [f32; 4],
}

impl Stroke {
    pub fn new(start: Vec2, width: f32, color: [f32; 4]) -> Self {
        Self {
            points: vec![start],
            width,
            color,
        }
    }

    /// Drops near-duplicate samples so segment directions are never NaN.
    pub fn push(&mut self, p: Vec2) -> bool {
        if self
            .points
            .last()
            .is_some_and(|last| last.distance_squared(p) < 1e-8)
        {
            return false;
        }
        self.points.push(p);
        true
    }
}

/// A capsule from `a` to `b`. A single-point stroke yields one zero-length segment (a dot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: Vec2,
    pub b: Vec2,
    pub half_width: f32,
    pub color: [f32; 4],
}

#[derive(Default, Debug)]
pub struct Document {
    pub strokes: Vec<Stroke>,
    /// Bumped on any edit that is not a pure append, so renderers know to rebuild instead of append.
    pub generation: u64,
}

impl Document {
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.strokes.iter().flat_map(|s| {
            let hw = s.width * 0.5;
            let pairs: Box<dyn Iterator<Item = (Vec2, Vec2)>> = if s.points.len() == 1 {
                Box::new(std::iter::once((s.points[0], s.points[0])))
            } else {
                Box::new(s.points.windows(2).map(|w| (w[0], w[1])))
            };
            pairs.map(move |(a, b)| Segment {
                a,
                b,
                half_width: hw,
                color: s.color,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = Viewport {
            offset: Vec2::new(40.0, -12.0),
            scale: 1.5,
        };
        let anchor = Vec2::new(300.0, 200.0);
        let before = v.screen_to_world(anchor);
        v.zoom_at(anchor, 3.7);
        assert!((v.screen_to_world(anchor) - before).length() < 1e-4);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut v = Viewport::default();
        v.zoom_at(Vec2::ZERO, 1e6);
        assert_eq!(v.scale, MAX_ZOOM);
        v.zoom_at(Vec2::ZERO, 1e-9);
        assert_eq!(v.scale, MIN_ZOOM);
    }

    #[test]
    fn pan_right_moves_content_right() {
        let mut v = Viewport::default();
        let p = Vec2::new(10.0, 10.0);
        let before = v.world_to_screen(p);
        v.pan_by_screen(Vec2::new(25.0, 0.0));
        assert!(v.world_to_screen(p).x > before.x);
    }

    #[test]
    fn world_screen_roundtrip() {
        let v = Viewport {
            offset: Vec2::new(-3.0, 7.5),
            scale: 0.37,
        };
        let p = Vec2::new(123.4, -56.7);
        assert!((v.screen_to_world(v.world_to_screen(p)) - p).length() < 1e-3);
    }

    #[test]
    fn duplicate_points_dropped_and_segments_counted() {
        let mut s = Stroke::new(Vec2::ZERO, 2.0, [1.0; 4]);
        assert!(!s.push(Vec2::ZERO));
        assert!(s.push(Vec2::X));
        assert!(s.push(Vec2::ONE));
        let doc = Document {
            strokes: vec![s, Stroke::new(Vec2::Y, 2.0, [1.0; 4])],
            generation: 0,
        };
        let segs: Vec<_> = doc.segments().collect();
        assert_eq!(segs.len(), 3); // 2 segments + 1 dot
        assert!(segs.iter().all(|s| s.a.is_finite() && s.b.is_finite()));
    }
}
