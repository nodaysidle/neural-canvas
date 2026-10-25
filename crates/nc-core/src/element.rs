//! Everything a sketch is made of. Every element has a stable id that survives undo, redo and reload.

use glam::Vec2;
use kurbo::Rect;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type ElementId = Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: ElementId,
    pub kind: ElementKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElementKind {
    Stroke(Stroke),
    Shape(Shape),
    Recognized(RecognizedShape),
}

impl Element {
    pub fn new(kind: ElementKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
        }
    }

    pub fn stroke(stroke: Stroke) -> Self {
        Self::new(ElementKind::Stroke(stroke))
    }

    /// World-space bounds including line width. `None` for an element with no geometry.
    pub fn bounds(&self) -> Option<Rect> {
        match &self.kind {
            ElementKind::Stroke(s) => s.bounds(),
            ElementKind::Shape(s) => {
                Some(rect(s.from, s.to).inflate(s.width as f64 * 0.5, s.width as f64 * 0.5))
            }
            ElementKind::Recognized(r) => Some(rect(r.min, r.max)),
        }
    }
}

fn rect(a: Vec2, b: Vec2) -> Rect {
    Rect::from_points((a.x as f64, a.y as f64), (b.x as f64, b.y as f64))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub points: Vec<Vec2>,
    /// Full width in world units; with pressure, the width at full pressure.
    pub width: f32,
    /// Linear RGBA.
    pub color: [f32; 4],
    /// Stylus pressure per point, 0..=1. Empty for mouse strokes (constant width); otherwise exactly
    /// one value per point.
    pub pressure: Vec<f32>,
}

/// Width multiplier for a pressure: never thinner than 20 %, so a light touch still leaves ink.
pub fn pressure_scale(pressure: f32) -> f32 {
    0.2 + 0.8 * pressure.clamp(0.0, 1.0)
}

impl Stroke {
    /// A constant-width (mouse) stroke.
    pub fn new(start: Vec2, width: f32, color: [f32; 4]) -> Self {
        Self {
            points: vec![start],
            width,
            color,
            pressure: Vec::new(),
        }
    }

    /// A stylus stroke whose width follows `pressure`.
    pub fn with_pressure(start: Vec2, pressure: f32, width: f32, color: [f32; 4]) -> Self {
        Self {
            pressure: vec![pressure],
            ..Self::new(start, width, color)
        }
    }

    /// Drops near-duplicate samples so segment directions are never NaN. On a pressure stroke the
    /// previous pressure carries over.
    pub fn push(&mut self, p: Vec2) -> bool {
        let pressure = self.pressure.last().copied();
        self.push_sample(p, pressure)
    }

    /// Like [`push`](Self::push), with this sample's pressure (ignored on a constant-width stroke).
    pub fn push_sample(&mut self, p: Vec2, pressure: Option<f32>) -> bool {
        if self
            .points
            .last()
            .is_some_and(|last| last.distance_squared(p) < 1e-8)
        {
            return false;
        }
        self.points.push(p);
        if !self.pressure.is_empty() {
            let last = self.pressure[self.pressure.len() - 1];
            self.pressure.push(pressure.unwrap_or(last));
        }
        true
    }

    /// Half width of the capsule from point `i` to point `i + 1` (or the dot at `i`).
    pub fn half_width(&self, i: usize) -> f32 {
        let scale = match self.pressure.as_slice() {
            [] => 1.0,
            p => pressure_scale((p[i] + p[(i + 1).min(p.len() - 1)]) * 0.5),
        };
        self.width * 0.5 * scale
    }

    pub fn bounds(&self) -> Option<Rect> {
        let (first, rest) = self.points.split_first()?;
        let (min, max) = rest
            .iter()
            .fold((*first, *first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let hw = self.width as f64 * 0.5;
        Some(rect(min, max).inflate(hw, hw))
    }
}

/// A drawn primitive from the rect / ellipse / line tools (Phase 3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Drag start and end: opposite corners for rect and ellipse, endpoints for a line.
    pub from: Vec2,
    pub to: Vec2,
    pub width: f32,
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
}

/// A UI element recognised from strokes (Phase 4 recognition writes these).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecognizedShape {
    pub shape_type: ShapeType,
    pub min: Vec2,
    pub max: Vec2,
    /// 0..=1, from the recognition heuristics.
    pub confidence: f32,
    /// The strokes this element was recognised from.
    pub sources: Vec<ElementId>,
}

/// Names kept from the Swift app's `ShapeType`. Only the cases the port plan names are listed; the
/// remaining Swift cases are appended (never inserted: blobs encode the variant index) in Phase 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeType {
    Button,
    InputField,
    Card,
    Container,
    Divider,
    Icon,
    Image,
    Text,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_bounds_include_width() {
        let mut s = Stroke::new(Vec2::new(10.0, 20.0), 4.0, [1.0; 4]);
        s.push(Vec2::new(30.0, 5.0));
        assert_eq!(s.bounds(), Some(Rect::new(8.0, 3.0, 32.0, 22.0)));
    }

    #[test]
    fn pressure_track_stays_parallel_to_points() {
        let mut s = Stroke::with_pressure(Vec2::ZERO, 0.5, 10.0, [1.0; 4]);
        assert!(
            !s.push_sample(Vec2::ZERO, Some(1.0)),
            "duplicate dropped with its pressure"
        );
        assert!(s.push_sample(Vec2::X, Some(1.0)));
        assert!(
            s.push(Vec2::ONE),
            "no pressure given: the last one carries over"
        );
        assert_eq!(s.pressure, [0.5, 1.0, 1.0]);
        assert_eq!(s.half_width(0), 5.0 * pressure_scale(0.75));
        assert_eq!(s.half_width(1), 5.0);

        let mut mouse = Stroke::new(Vec2::ZERO, 10.0, [1.0; 4]);
        mouse.push_sample(Vec2::X, Some(0.1));
        assert!(
            mouse.pressure.is_empty(),
            "mouse strokes stay constant width"
        );
        assert_eq!(mouse.half_width(0), 5.0);
    }

    #[test]
    fn ids_are_unique() {
        let s = Stroke::new(Vec2::ZERO, 1.0, [1.0; 4]);
        assert_ne!(Element::stroke(s.clone()).id, Element::stroke(s).id);
    }
}
