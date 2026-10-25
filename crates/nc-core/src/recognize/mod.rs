//! Sketch recognition: strokes in, wireframe elements out, with an honest quality report. Pure geometry,
//! deterministic, run on a worker thread by the app (never on the main thread).
//!
//! [`swift`] is a line-for-line port of the Mac app's `SketchRecognitionActor` stroke path, kept as the
//! accuracy baseline. [`recognize`] is the recognizer the app uses.

mod compose;
pub mod draw;
mod geometry;
mod primitive;
pub mod swift;

use crate::element::{Element, ElementId, ElementKind, RecognizedShape, ShapeType, Stroke};
use glam::Vec2;
use kurbo::Rect;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// One recognised wireframe element.
#[derive(Clone, Debug, PartialEq)]
pub struct Recognized {
    pub shape_type: ShapeType,
    /// World units, y-down, including ink width (as the Swift app measured).
    pub bounds: Rect,
    /// 0..=1.
    pub confidence: f32,
    pub corner_radius: Option<f32>,
    /// Lines, dividers and arrows: where they start and end (an arrow's tip is the second point).
    pub ends: Option<[Vec2; 2]>,
    /// The elements it was recognised from.
    pub sources: Vec<ElementId>,
    /// Text the user gave it (a button's caption).
    pub label: String,
    /// Set when the user edited it: the document element that holds the edit.
    pub pinned: Option<ElementId>,
}

/// Corner radius a UI element is drawn with (the Swift app's values).
fn default_radius(t: ShapeType) -> Option<f32> {
    match t {
        ShapeType::Button => Some(8.0),
        ShapeType::InputField | ShapeType::RoundedRectangle => Some(6.0),
        ShapeType::Card | ShapeType::Container => Some(12.0),
        _ => None,
    }
}

impl Recognized {
    fn new(shape_type: ShapeType, bounds: Rect, confidence: f32, sources: Vec<ElementId>) -> Self {
        Self {
            shape_type,
            bounds,
            confidence,
            corner_radius: None,
            ends: None,
            sources,
            label: String::new(),
            pinned: None,
        }
    }

    /// The document element that stores a user's edit of this one (its type, label and bounds).
    pub fn to_pinned(&self) -> RecognizedShape {
        let v = |x: f64, y: f64| Vec2::new(x as f32, y as f32);
        RecognizedShape {
            shape_type: self.shape_type,
            min: v(self.bounds.x0, self.bounds.y0),
            max: v(self.bounds.x1, self.bounds.y1),
            confidence: 1.0,
            sources: self.sources.clone(),
            label: self.label.clone(),
        }
    }

    pub fn is_high_confidence(&self) -> bool {
        self.confidence >= 0.7
    }

    pub fn is_medium_confidence(&self) -> bool {
        (0.4..0.7).contains(&self.confidence)
    }

    /// What "Review uncertain items" walks through: the cases the quality gate flags.
    pub fn is_uncertain(&self) -> bool {
        self.confidence < 0.4 || self.shape_type == ShapeType::Unknown
    }
}

/// Coarse quality of a wireframe (Swift `RecognitionQualityStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityStatus {
    Ready,
    ReviewRecommended,
    LowConfidence,
}

impl QualityStatus {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::ReviewRecommended => "Review recommended",
            Self::LowConfidence => "Low confidence",
        }
    }
}

/// Auditable metrics behind the status (Swift `RecognitionQualityReport`, same thresholds).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QualityReport {
    pub shape_count: usize,
    pub semantic_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub unknown_count: usize,
    pub uncertain_count: usize,
    pub average_confidence: f32,
}

impl QualityReport {
    pub fn of(shapes: &[Recognized]) -> Self {
        let count = |f: &dyn Fn(&Recognized) -> bool| shapes.iter().filter(|s| f(s)).count();
        let high_count = count(&Recognized::is_high_confidence);
        let medium_count = count(&Recognized::is_medium_confidence);
        Self {
            shape_count: shapes.len(),
            semantic_count: count(&|s| s.shape_type.is_semantic()),
            high_count,
            medium_count,
            low_count: shapes.len() - high_count - medium_count,
            unknown_count: count(&|s| s.shape_type == ShapeType::Unknown),
            uncertain_count: count(&Recognized::is_uncertain),
            average_confidence: if shapes.is_empty() {
                0.0
            } else {
                shapes.iter().map(|s| s.confidence).sum::<f32>() / shapes.len() as f32
            },
        }
    }

    pub fn status(&self) -> QualityStatus {
        if self.shape_count == 0 || self.semantic_count == 0 || self.average_confidence < 0.4 {
            QualityStatus::LowConfidence
        } else if self.unknown_count > 0 || self.low_count > 0 || self.average_confidence < 0.65 {
            QualityStatus::ReviewRecommended
        } else {
            QualityStatus::Ready
        }
    }

    /// Style mirror (Phase 6) refuses to apply onto a low-confidence wireframe.
    pub fn can_apply_imported_style(&self) -> bool {
        self.status() != QualityStatus::LowConfidence
    }
}

/// A recognised sketch: what the overlay draws and what wireframe export writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wireframe {
    pub shapes: Vec<Recognized>,
    pub report: QualityReport,
}

/// A recorded drawing with the element it should be recognised as. Fixtures are stored as JSON under
/// `crates/nc-core/tests/fixtures/`, in world units relative to the drawing's own cell.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub expected: ShapeType,
    pub strokes: Vec<FixtureStroke>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureStroke {
    pub width: f32,
    pub points: Vec<[f32; 2]>,
}

impl Fixture {
    pub fn elements(&self) -> Vec<Arc<Element>> {
        self.strokes
            .iter()
            .map(|s| {
                let mut points = s.points.iter().map(|p| Vec2::from_array(*p));
                let mut stroke = Stroke::new(points.next().unwrap_or_default(), s.width, [1.0; 4]);
                points.for_each(|p| {
                    stroke.push(p);
                });
                Arc::new(Element::stroke(stroke))
            })
            .collect()
    }
}

/// Recognises every stroke and shape in `elements` (in z-order) as one wireframe. Elements the user
/// edited (stored as `Recognized` elements) win: they are used as they are, and the strokes they came
/// from are not recognised again.
pub fn recognize(elements: &[Arc<Element>]) -> Wireframe {
    let pinned: Vec<(ElementId, &RecognizedShape)> = elements
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Recognized(r) => Some((e.id, r)),
            _ => None,
        })
        .collect();
    let covered: HashSet<ElementId> = pinned
        .iter()
        .flat_map(|(_, r)| r.sources.iter().copied())
        .collect();
    let free: Vec<Arc<Element>> = elements
        .iter()
        .filter(|e| !covered.contains(&e.id))
        .cloned()
        .collect();
    let mut shapes = compose::recognize(&free);
    shapes.extend(pinned.into_iter().map(|(id, r)| {
        let bounds = Rect::new(
            r.min.x as f64,
            r.min.y as f64,
            r.max.x as f64,
            r.max.y as f64,
        );
        Recognized {
            corner_radius: default_radius(r.shape_type),
            label: r.label.clone(),
            pinned: Some(id),
            ..Recognized::new(r.shape_type, bounds, r.confidence, r.sources.clone())
        }
    }));
    Wireframe {
        report: QualityReport::of(&shapes),
        shapes,
    }
}

/// The element a drawing is read as: the largest recognised shape (ties: the more confident one), so a
/// card's inner scribbles don't count against it.
pub fn primary(shapes: &[Recognized]) -> Option<&Recognized> {
    shapes.iter().max_by(|a, b| {
        a.bounds
            .area()
            .total_cmp(&b.bounds.area())
            .then(a.confidence.total_cmp(&b.confidence))
    })
}

/// The strokes of `elements` with their ids (the input the Swift recognizer took).
pub fn strokes(elements: &[Arc<Element>]) -> Vec<(ElementId, &Stroke)> {
    elements
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Stroke(s) => Some((e.id, s)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: Vec2, b: Vec2) -> Arc<Element> {
        let mut s = Stroke::new(a, 2.0, [1.0; 4]);
        s.push(b);
        Arc::new(Element::stroke(s))
    }

    /// An edit pins an element: it replaces what recognition made of its strokes, keeps its label,
    /// and is never second-guessed (confidence 1, not uncertain).
    #[test]
    fn pinned_edits_override_recognition() {
        let divider = line(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        let other = line(Vec2::new(0.0, 100.0), Vec2::new(50.0, 160.0));
        let elements = vec![divider.clone(), other];
        let before = recognize(&elements);
        assert_eq!(before.shapes.len(), 2);
        let mut edited = before
            .shapes
            .iter()
            .find(|s| s.sources == [divider.id])
            .unwrap()
            .clone();
        assert_eq!(edited.shape_type, ShapeType::Divider);
        edited.shape_type = ShapeType::Text;
        edited.label = "Section title".into();
        let pin = Arc::new(Element::new(ElementKind::Recognized(edited.to_pinned())));
        let mut with_pin = elements.clone();
        with_pin.push(pin.clone());
        let after = recognize(&with_pin);
        assert_eq!(
            after.shapes.len(),
            2,
            "the divider was not recognised again"
        );
        let shown = after
            .shapes
            .iter()
            .find(|s| s.pinned == Some(pin.id))
            .unwrap();
        assert_eq!(
            (shown.shape_type, shown.label.as_str()),
            (ShapeType::Text, "Section title")
        );
        assert!(!shown.is_uncertain() && shown.confidence == 1.0);
    }
}
