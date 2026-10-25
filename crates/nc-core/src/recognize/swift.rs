//! The Mac app's stroke recognizer (`SketchRecognitionActor.processStrokes`), ported rule for rule with
//! its thresholds and quirks, as the accuracy baseline. Its Vision (raster) path was dropped with the
//! Mac app; `detectCornerRadius` always returned `nil` there and does here.

use super::Recognized;
use super::geometry::overlap_area;
use crate::element::{ElementId, ShapeType, Stroke};
use glam::Vec2;
use kurbo::Rect;

pub fn recognize(strokes: &[(ElementId, &Stroke)]) -> Vec<Recognized> {
    let mut shapes: Vec<Recognized> = strokes
        .iter()
        .filter_map(|(id, s)| analyze_stroke(*id, s))
        .collect();
    shapes.extend(detect_composite_shapes(strokes));
    let unique = remove_duplicate_shapes(shapes);
    (0..unique.len())
        .map(|i| promote_to_ui_element(i, &unique))
        .collect()
}

fn analyze_stroke(id: ElementId, stroke: &Stroke) -> Option<Recognized> {
    let points = &stroke.points;
    if points.len() < 2 {
        return None;
    }
    let bounds = stroke.bounds()?;
    let (w, h) = (bounds.width(), bounds.height());
    let shape = |t, c| Some(Recognized::new(t, bounds, c, vec![id]));
    if points.len() == 2 {
        let divider = w > h * 3.0 && h <= 6f64.max(stroke.width as f64 * 2.0);
        return shape(
            if divider {
                ShapeType::Divider
            } else {
                ShapeType::Line
            },
            0.8,
        );
    }
    let aspect = w / h.max(1.0);
    let (start, end) = (points[0], points[points.len() - 1]);
    let closed = (start.distance(end) as f64) < w.max(h) * 0.15;
    if !closed && is_approximately_linear(points) {
        return shape(ShapeType::Line, 0.85);
    }
    if closed && is_approximately_circular(points, bounds) {
        let round = aspect > 0.8 && aspect < 1.2;
        return shape(
            if round {
                ShapeType::Circle
            } else {
                ShapeType::Ellipse
            },
            0.75,
        );
    }
    if closed && has_corners(points) {
        return shape(ShapeType::Rectangle, 0.7);
    }
    shape(ShapeType::Unknown, 0.3)
}

/// Every point within 10 % of the chord length of the start–end line.
pub(super) fn is_approximately_linear(points: &[Vec2]) -> bool {
    let (start, end) = (points[0], points[points.len() - 1]);
    let length = start.distance(end);
    if length <= 0.0 {
        return true;
    }
    let d = end - start;
    let max_deviation = points
        .iter()
        .map(|p| (d.y * p.x - d.x * p.y + end.x * start.y - end.y * start.x).abs() / length)
        .fold(0.0, f32::max);
    max_deviation < length * 0.1
}

/// Mean distance from the bounds centre within 20 % of the mean radius.
fn is_approximately_circular(points: &[Vec2], bounds: Rect) -> bool {
    let c = bounds.center();
    let center = Vec2::new(c.x as f32, c.y as f32);
    let radius = ((bounds.width() + bounds.height()) / 4.0) as f32;
    let deviation = points
        .iter()
        .map(|p| (p.distance(center) - radius).abs())
        .sum::<f32>()
        / points.len() as f32;
    deviation < radius * 0.2
}

/// 3 to 6 turns sharper than ~45° between consecutive raw samples.
fn has_corners(points: &[Vec2]) -> bool {
    if points.len() < 4 {
        return false;
    }
    let corners = points
        .windows(3)
        .filter(|w| {
            let (v1, v2) = (w[1] - w[0], w[2] - w[1]);
            let (l1, l2) = (v1.length(), v2.length());
            l1 > 0.0 && l2 > 0.0 && v1.dot(v2) / (l1 * l2) < 0.7
        })
        .count();
    (3..=6).contains(&corners)
}

/// A stroke whose bounds contain every stroke's bounds is a container.
fn detect_composite_shapes(strokes: &[(ElementId, &Stroke)]) -> Vec<Recognized> {
    if strokes.len() < 2 {
        return Vec::new();
    }
    let bounds: Vec<(ElementId, Rect)> = strokes
        .iter()
        .filter_map(|(id, s)| Some((*id, s.bounds()?)))
        .collect();
    let Some(all) = bounds.iter().map(|(_, b)| *b).reduce(|a, b| a.union(b)) else {
        return Vec::new();
    };
    bounds
        .iter()
        .filter(|(_, b)| b.contains_rect(all))
        .map(|(id, b)| Recognized::new(ShapeType::Container, *b, 0.5, vec![*id]))
        .collect()
}

fn remove_duplicate_shapes(shapes: Vec<Recognized>) -> Vec<Recognized> {
    let mut unique: Vec<Recognized> = Vec::new();
    for shape in shapes {
        let duplicate = unique.iter().any(|existing| {
            let ratio = overlap_area(existing.bounds, shape.bounds)
                / existing.bounds.area().min(shape.bounds.area());
            ratio > 0.8 && existing.shape_type == shape.shape_type
        });
        if !duplicate {
            unique.push(shape);
        }
    }
    unique
}

fn promote_to_ui_element(index: usize, all: &[Recognized]) -> Recognized {
    let shape = &all[index];
    let b = shape.bounds;
    let (w, h) = (b.width(), b.height());
    if w <= 0.0 || h <= 0.0 {
        return shape.clone();
    }
    let aspect = w / h.max(1.0);
    let area = w * h;
    let replace = |t, c: f32, radius: Option<f32>| Recognized {
        shape_type: t,
        confidence: c.max(shape.confidence).min(0.95),
        corner_radius: radius.or(shape.corner_radius),
        ..shape.clone()
    };
    match shape.shape_type {
        ShapeType::Line => {
            if w >= 80.0 && h <= 8.0 {
                replace(ShapeType::Divider, 0.78, None)
            } else if aspect >= 4.0 && h <= 20.0 {
                replace(ShapeType::Text, 0.62, None)
            } else {
                shape.clone()
            }
        }
        ShapeType::Circle | ShapeType::Ellipse if w.min(h) <= 64.0 => {
            replace(ShapeType::Icon, 0.58, None)
        }
        ShapeType::Rectangle | ShapeType::RoundedRectangle | ShapeType::Unknown => {
            let contained = all
                .iter()
                .enumerate()
                .filter(|(i, c)| {
                    *i != index && b.contains_rect(c.bounds) && c.bounds.area() < area * 0.65
                })
                .count();
            if contained >= 2 || area > 160_000.0 {
                let t = if area > 260_000.0 {
                    ShapeType::Container
                } else {
                    ShapeType::Card
                };
                replace(t, 0.64, Some(12.0))
            } else if aspect >= 3.2 && (24.0..=64.0).contains(&h) && w >= 150.0 {
                replace(ShapeType::InputField, 0.72, Some(6.0))
            } else if (1.4..=5.5).contains(&aspect) && (24.0..=64.0).contains(&h) && w >= 48.0 {
                replace(ShapeType::Button, 0.7, Some(8.0))
            } else if aspect >= 4.0 && h <= 24.0 {
                replace(ShapeType::Text, 0.62, None)
            } else if (0.75..=1.35).contains(&aspect) && w.min(h) >= 56.0 {
                replace(ShapeType::Image, 0.55, None)
            } else if shape.shape_type == ShapeType::Unknown {
                replace(ShapeType::RoundedRectangle, 0.42, Some(6.0))
            } else {
                shape.clone()
            }
        }
        _ => shape.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[(f32, f32)]) -> Stroke {
        let mut s = Stroke::new(Vec2::new(points[0].0, points[0].1), 2.0, [1.0; 4]);
        for p in &points[1..] {
            s.push(Vec2::new(p.0, p.1));
        }
        s
    }

    fn one(points: &[(f32, f32)]) -> Recognized {
        let s = stroke(points);
        let out = recognize(&[(ElementId::nil(), &s)]);
        assert_eq!(out.len(), 1, "{out:?}");
        out.into_iter().next().unwrap()
    }

    /// A closed rectangle sampled every `step` units along its outline.
    fn rect(x: f32, y: f32, w: f32, h: f32, step: f32) -> Vec<(f32, f32)> {
        let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)];
        let mut out = vec![corners[0]];
        for c in corners.windows(2) {
            let (a, b) = (Vec2::new(c[0].0, c[0].1), Vec2::new(c[1].0, c[1].1));
            let n = (a.distance(b) / step).ceil() as usize;
            out.extend((1..=n).map(|i| {
                let p = a.lerp(b, i as f32 / n as f32);
                (p.x, p.y)
            }));
        }
        out
    }

    #[test]
    fn two_point_strokes_are_lines_or_dividers() {
        assert_eq!(
            one(&[(0.0, 0.0), (200.0, 0.0)]).shape_type,
            ShapeType::Divider
        );
        assert_eq!(one(&[(0.0, 0.0), (50.0, 60.0)]).shape_type, ShapeType::Line);
    }

    #[test]
    fn linear_strokes_promote_by_size() {
        let wobble: Vec<_> = (0..=40)
            .map(|i| (i as f32 * 5.0, (i % 2) as f32 * 3.0))
            .collect();
        let r = one(&wobble);
        assert_eq!((r.shape_type, r.confidence), (ShapeType::Divider, 0.85));
        let diagonal: Vec<_> = (0..=20).map(|i| (i as f32 * 5.0, i as f32 * 4.0)).collect();
        assert_eq!(one(&diagonal).shape_type, ShapeType::Line);
    }

    #[test]
    fn closed_round_strokes_are_circles_or_icons() {
        let circle = |r: f32| -> Vec<(f32, f32)> {
            (0..=64)
                .map(|i| {
                    let a = i as f32 / 64.0 * std::f32::consts::TAU;
                    (r * a.cos(), r * a.sin())
                })
                .collect()
        };
        assert_eq!(one(&circle(100.0)).shape_type, ShapeType::Circle);
        assert_eq!(one(&circle(20.0)).shape_type, ShapeType::Icon);
    }

    /// Swift counted turns between raw samples, so a cleanly sampled rectangle has exactly its 4 corners.
    #[test]
    fn rectangles_promote_by_proportion() {
        let kind = |w, h| one(&rect(0.0, 0.0, w, h, 5.0)).shape_type;
        assert_eq!(kind(120.0, 40.0), ShapeType::Button);
        assert_eq!(kind(300.0, 40.0), ShapeType::InputField);
        assert_eq!(kind(250.0, 150.0), ShapeType::Rectangle);
        // Swift quirk, kept: circularity is tested before corners and its 20 % mean-deviation bound
        // also passes squarish rectangles, so images, cards and containers drawn as one stroke come out
        // as circles. The Phase 4 recognizer fixes this; the baseline keeps it.
        for (w, h) in [(120.0, 120.0), (450.0, 400.0), (600.0, 500.0)] {
            assert_eq!(kind(w, h), ShapeType::Circle, "{w}x{h}");
        }
    }

    #[test]
    fn a_rectangle_around_two_strokes_is_a_card_and_a_container() {
        let outer = stroke(&rect(0.0, 0.0, 300.0, 200.0, 5.0));
        let a = stroke(&[(20.0, 20.0), (120.0, 20.0)]);
        let b = stroke(&[(20.0, 60.0), (200.0, 60.0)]);
        let id = ElementId::nil;
        let out = recognize(&[(id(), &outer), (id(), &a), (id(), &b)]);
        let types: Vec<_> = out.iter().map(|r| r.shape_type).collect();
        assert!(types.contains(&ShapeType::Card), "{types:?}");
        assert!(types.contains(&ShapeType::Container), "{types:?}");
    }
}
