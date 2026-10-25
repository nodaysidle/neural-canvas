//! Eraser hit testing along the swept path, not at sampled points, so a fast drag cannot skip over a
//! stroke between two samples (Mac bug #14).

use crate::document::{Document, stroke_segments};
use crate::element::{ElementId, ElementKind};
use glam::Vec2;
use kurbo::{Line, ParamCurveNearest, Point, Rect};

fn point(v: Vec2) -> Point {
    Point::new(v.x as f64, v.y as f64)
}

/// Distance between segments `a` and `b`; 0 when they cross.
fn segment_distance(a: Line, b: Line) -> f64 {
    if let Some(x) = a.crossing_point(b)
        && a.nearest(x, 0.0).distance_sq < 1e-12
        && b.nearest(x, 0.0).distance_sq < 1e-12
    {
        return 0.0;
    }
    [
        a.nearest(b.p0, 0.0),
        a.nearest(b.p1, 0.0),
        b.nearest(a.p0, 0.0),
        b.nearest(a.p1, 0.0),
    ]
    .iter()
    .map(|n| n.distance_sq)
    .fold(f64::INFINITY, f64::min)
    .sqrt()
}

// shortcut: linear scan with a bounds pre-check, ~2,000 rect tests per eraser move on S2k; Phase 3 moves
// this onto the rstar index.
/// Appends to `out` every stroke whose ink a disc of `radius` (world units) touches while moving from
/// `from` to `to`.
pub fn strokes_touched(
    doc: &Document,
    from: Vec2,
    to: Vec2,
    radius: f32,
    out: &mut Vec<ElementId>,
) {
    let sweep = Line::new(point(from), point(to));
    let reach = Rect::from_points(sweep.p0, sweep.p1).inflate(radius as f64, radius as f64);
    for element in doc.elements() {
        let ElementKind::Stroke(stroke) = &element.kind else {
            continue;
        };
        if !stroke.bounds().is_some_and(|b| b.overlaps(reach)) {
            continue;
        }
        let touches = stroke_segments(stroke).any(|s| {
            segment_distance(sweep, Line::new(point(s.a), point(s.b)))
                <= (radius + s.half_width) as f64
        });
        if touches {
            out.push(element.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Element, Stroke};
    use std::sync::Arc;

    fn line(a: Vec2, b: Vec2) -> Element {
        let mut s = Stroke::new(a, 2.0, [1.0; 4]);
        s.push(b);
        Element::stroke(s)
    }

    fn touched(doc: &Document, from: Vec2, to: Vec2, radius: f32) -> Vec<ElementId> {
        let mut out = Vec::new();
        strokes_touched(doc, from, to, radius, &mut out);
        out
    }

    /// Mac bug #14 guard: one fast eraser move that jumps clean across a stroke still erases it.
    #[test]
    fn a_fast_drag_cannot_skip_a_stroke() {
        let wall = line(Vec2::new(100.0, 0.0), Vec2::new(100.0, 200.0));
        let id = wall.id;
        let doc = Document::from_elements([Arc::new(wall)]);
        // Both samples are 90 px from the stroke; only the path between them crosses it.
        assert_eq!(
            touched(&doc, Vec2::new(10.0, 100.0), Vec2::new(190.0, 100.0), 4.0),
            [id]
        );
    }

    #[test]
    fn touching_mid_segment_counts_and_near_misses_do_not() {
        let s = line(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        let id = s.id;
        let doc = Document::from_elements([Arc::new(s)]);
        // Ink half width 1 + eraser radius 4: reach is 5 px from the centre line.
        assert_eq!(
            touched(&doc, Vec2::new(100.0, 4.9), Vec2::new(100.0, 4.9), 4.0),
            [id]
        );
        assert!(touched(&doc, Vec2::new(100.0, 5.2), Vec2::new(120.0, 5.2), 4.0).is_empty());
        assert!(touched(&doc, Vec2::new(-20.0, 0.0), Vec2::new(-6.0, 0.0), 4.0).is_empty());
    }

    #[test]
    fn segment_distance_cases() {
        let l = |a: (f64, f64), b: (f64, f64)| Line::new(a, b);
        assert_eq!(
            segment_distance(l((0., 0.), (10., 0.)), l((5., -5.), (5., 5.))),
            0.0
        );
        assert_eq!(
            segment_distance(l((0., 0.), (10., 0.)), l((15., -5.), (15., 5.))),
            5.0
        );
        assert_eq!(
            segment_distance(l((0., 0.), (10., 0.)), l((0., 3.), (10., 3.))),
            3.0
        );
        assert_eq!(
            segment_distance(l((0., 0.), (0., 0.)), l((3., 4.), (3., 4.))),
            5.0
        );
    }
}
