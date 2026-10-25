//! Pure document and viewport logic. No GPU, no windowing: everything here is unit-testable headless.
//!
//! World space is y-down, origin top-left, 1 unit = 1 logical px at 100% zoom (docs/PLAN.md §2).

pub mod actions;
pub mod animation;
pub mod command;
pub mod document;
pub mod element;
pub mod hit;
pub mod input;
pub mod recognize;
pub mod scene;
pub mod schedule;
pub mod select;
pub mod shape;
pub mod theme;
pub mod tools;

pub use actions::{Action, Key, Shortcut};
pub use animation::ViewAnimation;
pub use command::{Command, CommandError, History, Session};
pub use document::{Document, Segment};
pub use element::{
    Element, ElementId, ElementKind, RecognizedShape, Shape, ShapeKind, ShapeType, Stroke,
};
pub use input::{Predictor, Smoother};
pub use kurbo::{self, Rect};
pub use schedule::FrameScheduler;
pub use tools::{Tool, Tools};

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

    /// Shows all of `bounds` centred in a `screen`-sized view with `padding` logical px on each side
    /// (Mac bug #18: zoom to fit must use the content's real bounds).
    pub fn fit(bounds: Rect, screen: Vec2, padding: f32) -> Self {
        let avail = (screen - Vec2::splat(2.0 * padding)).max(Vec2::ONE);
        let size = Vec2::new(bounds.width() as f32, bounds.height() as f32).max(Vec2::splat(1e-3));
        let scale = (avail / size).min_element().clamp(MIN_ZOOM, MAX_ZOOM);
        let center = bounds.center();
        let center = Vec2::new(center.x as f32, center.y as f32);
        Self {
            scale,
            offset: screen * 0.5 - center * scale,
        }
    }

    /// Zooms by `factor`, keeping the world point under `anchor` (logical screen px) fixed.
    pub fn zoom_at(&mut self, anchor: Vec2, factor: f32) {
        let world = self.screen_to_world(anchor);
        self.scale = (self.scale * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = anchor - world * self.scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

    /// Mac bug #18 guard: fit frames the real content, wherever it is.
    #[test]
    fn fit_uses_content_bounds() {
        let bounds = Rect::new(5_000.0, -3_000.0, 5_400.0, -2_900.0);
        let screen = Vec2::new(1280.0, 800.0);
        let v = Viewport::fit(bounds, screen, 32.0);
        let tl = v.world_to_screen(Vec2::new(5_000.0, -3_000.0));
        let br = v.world_to_screen(Vec2::new(5_400.0, -2_900.0));
        assert!(tl.cmpge(Vec2::splat(31.9)).all() && br.cmple(screen - 31.9).all());
        assert!((tl + br - screen).length() < 1e-2, "content is centred");
        assert!(
            (br.x - tl.x - (1280.0 - 64.0)).abs() < 1e-2,
            "the wider side fills the view"
        );
    }

    #[test]
    fn fit_of_a_dot_is_clamped() {
        let v = Viewport::fit(
            Rect::new(10.0, 10.0, 10.0, 10.0),
            Vec2::new(800.0, 600.0),
            0.0,
        );
        assert_eq!(v.scale, MAX_ZOOM);
        assert!((v.world_to_screen(Vec2::splat(10.0)) - Vec2::new(400.0, 300.0)).length() < 1e-3);
    }

    proptest! {
        #[test]
        fn roundtrip_any_view(
            ox in -1e4f32..1e4, oy in -1e4f32..1e4, scale in MIN_ZOOM..MAX_ZOOM,
            px in -1e4f32..1e4, py in -1e4f32..1e4,
        ) {
            let v = Viewport { offset: Vec2::new(ox, oy), scale };
            let p = Vec2::new(px, py);
            let back = v.screen_to_world(v.world_to_screen(p));
            prop_assert!(back.is_finite());
            prop_assert!((back - p).length() <= 1e-3 * (1.0 + p.length() + Vec2::new(ox, oy).length() / scale));
        }

        #[test]
        fn zoom_holds_any_anchor(
            scale in MIN_ZOOM..MAX_ZOOM, factor in 0.01f32..100.0,
            ax in 0f32..2560.0, ay in 0f32..1440.0,
        ) {
            let mut v = Viewport { offset: Vec2::new(-300.0, 120.0), scale };
            let anchor = Vec2::new(ax, ay);
            let before = v.screen_to_world(anchor);
            v.zoom_at(anchor, factor);
            prop_assert!(v.scale >= MIN_ZOOM && v.scale <= MAX_ZOOM);
            // World point under the anchor stays put, measured in screen px.
            prop_assert!((v.world_to_screen(before) - anchor).length() < 1e-2);
        }
    }
}
