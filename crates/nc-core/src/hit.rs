//! Hit tests on ink: picking, marquee and lasso selection, and the eraser. All distances are to the
//! swept path between samples, not to sampled points, so a fast drag cannot skip over ink between two
//! samples (Mac bug #14).

use crate::command::Command;
use crate::document::{Document, element_segments};
use crate::element::{Element, ElementId, ElementKind, Stroke};
use glam::Vec2;
use kurbo::{BezPath, Line, ParamCurveNearest, Point, Rect, Shape as _};
use std::sync::Arc;

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

/// Candidates whose bounds overlap `area`, topmost first.
fn candidates(doc: &Document, area: Rect) -> Vec<(usize, &Arc<Element>)> {
    let mut found: Vec<_> = doc
        .query(area)
        .into_iter()
        .filter_map(|id| doc.index_of(id))
        .map(|i| (i, &doc.elements()[i]))
        .collect();
    found.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
    found
}

/// The topmost element whose ink passes within `tolerance` (world units) of `p`.
pub fn pick(doc: &Document, p: Vec2, tolerance: f32) -> Option<ElementId> {
    let probe = Line::new(point(p), point(p));
    let area = Rect::from_points(probe.p0, probe.p0).inflate(tolerance as f64, tolerance as f64);
    candidates(doc, area).into_iter().find_map(|(_, e)| {
        let mut hit = false;
        element_segments(e, |s| {
            hit |= segment_distance(probe, Line::new(point(s.a), point(s.b)))
                <= (tolerance + s.half_width) as f64;
        });
        hit.then_some(e.id)
    })
}

/// Elements lying entirely inside `area` (marquee selection), in z-order.
pub fn in_rect(doc: &Document, area: Rect) -> Vec<ElementId> {
    let mut found = candidates(doc, area);
    found.retain(|(_, e)| e.bounds().is_some_and(|b| area.contains_rect(b)));
    found.reverse();
    found.into_iter().map(|(_, e)| e.id).collect()
}

/// Elements whose ink lies entirely inside the closed freehand `outline` (lasso selection), in z-order.
pub fn in_lasso(doc: &Document, outline: &[Vec2]) -> Vec<ElementId> {
    let Some((first, rest)) = outline.split_first() else {
        return Vec::new();
    };
    let mut path = BezPath::new();
    path.move_to(point(*first));
    for p in rest {
        path.line_to(point(*p));
    }
    path.close_path();
    let mut found = candidates(doc, path.bounding_box());
    found.retain(|(_, e)| {
        e.ink()
            .is_some_and(|ink| ink.points.iter().all(|p| path.contains(point(*p))))
    });
    found.reverse();
    found.into_iter().map(|(_, e)| e.id).collect()
}

/// The part of segment `a`→`b` (as parameters `t0..=t1` along it) within `reach` of `sweep`, if any.
/// Distance to a segment is convex along a line, so the part is one interval: find the closest point,
/// then bisect out to both edges.
fn cut(a: Vec2, b: Vec2, sweep: Line, reach: f64) -> Option<(f64, f64)> {
    let (a, b) = (point(a), point(b));
    let dist = |t: f64| sweep.nearest(a.lerp(b, t), 0.0).distance_sq.sqrt();
    if segment_distance(Line::new(a, b), sweep) > reach {
        return None;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if dist(m1) <= dist(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    let closest = (lo + hi) * 0.5;
    let edge = |mut inside: f64, mut outside: f64| {
        if dist(outside) <= reach {
            return outside;
        }
        for _ in 0..40 {
            let mid = (inside + outside) * 0.5;
            if dist(mid) <= reach {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        inside
    };
    Some((edge(closest, 0.0), edge(closest, 1.0)))
}

/// What is left of `s` after a disc of `radius` sweeps along `sweep`: `None` if untouched, otherwise
/// the surviving pieces (empty when nothing is left). Ink ends exactly at the eraser's edge.
fn split(s: &Stroke, sweep: Line, radius: f32) -> Option<Vec<Stroke>> {
    let sample = |i: usize| (s.points[i], s.pressure.get(i).copied());
    let lerp = |i: usize, t: f64| {
        let t = t as f32;
        let p = s.points[i].lerp(s.points[i + 1], t);
        let pr = s.pressure.get(i).map(|a| a + (s.pressure[i + 1] - a) * t);
        (p, pr)
    };
    if s.points.len() == 1 {
        let reach = (radius + s.half_width(0)) as f64;
        return (segment_distance(Line::new(point(s.points[0]), point(s.points[0])), sweep)
            <= reach)
            .then(Vec::new);
    }
    let mut pieces = Vec::new();
    let mut piece: Option<Stroke> = None;
    let add = |piece: &mut Option<Stroke>, (p, pr): (Vec2, Option<f32>)| match piece {
        Some(st) => {
            st.push_sample(p, pr);
        }
        None => {
            *piece = Some(match pr {
                Some(pr) => Stroke::with_pressure(p, pr, s.width, s.color),
                None => Stroke::new(p, s.width, s.color),
            })
        }
    };
    let mut touched = false;
    for i in 0..s.points.len() - 1 {
        let reach = (radius + s.half_width(i)) as f64;
        match cut(s.points[i], s.points[i + 1], sweep, reach) {
            None => {
                if piece.is_none() {
                    add(&mut piece, sample(i));
                }
                add(&mut piece, sample(i + 1));
            }
            Some((t0, t1)) => {
                touched = true;
                if t0 > 0.0 {
                    if piece.is_none() {
                        add(&mut piece, sample(i));
                    }
                    add(&mut piece, lerp(i, t0));
                }
                pieces.extend(piece.take());
                if t1 < 1.0 {
                    add(&mut piece, lerp(i, t1));
                    add(&mut piece, sample(i + 1));
                }
            }
        }
    }
    pieces.extend(piece);
    pieces.retain(|p| p.points.len() >= 2);
    touched.then_some(pieces)
}

/// The edit a disc of `radius` (world units) makes moving from `from` to `to`: each touched stroke or
/// shape is cut where the disc passed, keeping the rest. The first surviving piece keeps the element's
/// id and z-position; further pieces go right above it. `None` when nothing is touched.
pub fn erase(doc: &Document, from: Vec2, to: Vec2, radius: f32) -> Option<Command> {
    let sweep = Line::new(point(from), point(to));
    let area = Rect::from_points(sweep.p0, sweep.p1).inflate(radius as f64, radius as f64);
    let mut commands = Vec::new();
    // Topmost first, so inserting pieces above one element never shifts one still to be edited.
    for (index, element) in candidates(doc, area) {
        let Some(ink) = element.ink() else { continue };
        let Some(pieces) = split(&ink, sweep, radius) else {
            continue;
        };
        let mut pieces = pieces.into_iter();
        match pieces.next() {
            None => commands.push(Command::Remove { id: element.id }),
            Some(first) => {
                commands.push(Command::Replace {
                    element: Arc::new(Element {
                        id: element.id,
                        kind: ElementKind::Stroke(first),
                    }),
                });
                for (k, rest) in pieces.enumerate() {
                    commands.push(Command::Insert {
                        index: index + 1 + k,
                        element: Arc::new(Element::stroke(rest)),
                    });
                }
            }
        }
    }
    (!commands.is_empty()).then_some(Command::Batch(commands))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{Shape, ShapeKind};

    fn line(a: Vec2, b: Vec2) -> Element {
        let mut s = Stroke::new(a, 2.0, [1.0; 4]);
        s.push(b);
        Element::stroke(s)
    }

    fn strokes(doc: &Document) -> Vec<Vec<Vec2>> {
        doc.elements()
            .iter()
            .map(|e| e.ink().unwrap().points.clone())
            .collect()
    }

    fn assert_close(got: &[Vec2], want: &[Vec2]) {
        assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
        for (g, w) in got.iter().zip(want) {
            assert!(g.distance(*w) < 1e-3, "{got:?} vs {want:?}");
        }
    }

    fn erased(doc: &mut Document, from: Vec2, to: Vec2, radius: f32) -> bool {
        match erase(doc, from, to, radius) {
            Some(c) => {
                c.apply(doc).unwrap();
                true
            }
            None => false,
        }
    }

    /// Mac bug #14 guard: one fast eraser move that jumps clean across a stroke still cuts it.
    #[test]
    fn a_fast_drag_cannot_skip_a_stroke() {
        let mut doc = Document::from_elements([Arc::new(line(
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 200.0),
        ))]);
        // Both samples are 90 px from the stroke; only the path between them crosses it.
        assert!(erased(
            &mut doc,
            Vec2::new(10.0, 100.0),
            Vec2::new(190.0, 100.0),
            4.0
        ));
        // Reach is radius 4 + half width 1 = 5 either side of y = 100.
        let pieces = strokes(&doc);
        assert_eq!(pieces.len(), 2);
        assert_close(&pieces[0], &[Vec2::new(100.0, 0.0), Vec2::new(100.0, 95.0)]);
        assert_close(
            &pieces[1],
            &[Vec2::new(100.0, 105.0), Vec2::new(100.0, 200.0)],
        );
    }

    #[test]
    fn cuts_split_keep_id_order_and_undo_exactly() {
        let below = line(Vec2::new(-50.0, 50.0), Vec2::new(50.0, 50.0));
        let target = line(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        let above = line(Vec2::new(-50.0, -50.0), Vec2::new(50.0, -50.0));
        let id = target.id;
        let mut doc = Document::from_elements([below, target, above].map(Arc::new));
        let original = doc.elements().to_vec();
        let undo = erase(&doc, Vec2::new(100.0, -3.0), Vec2::new(100.0, 3.0), 4.0)
            .unwrap()
            .apply(&mut doc)
            .unwrap();
        let ids: Vec<_> = doc.elements().iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[1], id, "first piece keeps the id and z-position");
        assert_eq!((ids[0], ids[3]), (original[0].id, original[2].id));
        assert_close(
            &strokes(&doc)[2],
            &[Vec2::new(105.0, 0.0), Vec2::new(200.0, 0.0)],
        );
        undo.apply(&mut doc).unwrap();
        assert_eq!(doc.elements(), original.as_slice());
    }

    #[test]
    fn near_misses_do_not_cut_and_full_cover_removes() {
        let mut doc =
            Document::from_elements([Arc::new(line(Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0)))]);
        // Ink half width 1 + eraser radius 4: reach is 5 px from the centre line.
        assert!(!erased(
            &mut doc,
            Vec2::new(10.0, 5.2),
            Vec2::new(30.0, 5.2),
            4.0
        ));
        assert!(!erased(
            &mut doc,
            Vec2::new(-20.0, 0.0),
            Vec2::new(-6.0, 0.0),
            4.0
        ));
        assert!(erased(
            &mut doc,
            Vec2::new(-10.0, 0.0),
            Vec2::new(30.0, 0.0),
            4.0
        ));
        assert!(doc.elements().is_empty());
    }

    #[test]
    fn pressure_is_interpolated_at_the_cut() {
        let mut s = Stroke::with_pressure(Vec2::ZERO, 0.0, 2.0, [1.0; 4]);
        s.push_sample(Vec2::new(100.0, 0.0), Some(1.0));
        let mut doc = Document::from_elements([Arc::new(Element::stroke(s))]);
        // Radius 4 + local half width at t = 0.5 (pressure 0.5 → scale 0.6 → 0.6).
        assert!(erased(
            &mut doc,
            Vec2::new(50.0, -1.0),
            Vec2::new(50.0, 1.0),
            4.0
        ));
        let ElementKind::Stroke(left) = &doc.elements()[0].kind else {
            panic!()
        };
        assert_eq!(left.pressure.len(), left.points.len());
        let end = left.points[1].x;
        assert!((45.0..46.0).contains(&end), "cut at {end}");
        assert!((left.pressure[1] - end / 100.0).abs() < 1e-5);
    }

    /// A rectangle is ink like any other: touching the middle of any edge cuts a gap in that edge only.
    #[test]
    fn a_rectangle_erases_from_any_edge() {
        let rect = Element::new(ElementKind::Shape(Shape {
            kind: ShapeKind::Rectangle,
            from: Vec2::new(0.0, 0.0),
            to: Vec2::new(100.0, 60.0),
            width: 2.0,
            color: [1.0; 4],
        }));
        let mids = [
            Vec2::new(50.0, 0.0),
            Vec2::new(100.0, 30.0),
            Vec2::new(50.0, 60.0),
            Vec2::new(0.0, 30.0),
        ];
        for mid in mids {
            let mut doc = Document::from_elements([Arc::new(rect.clone())]);
            assert!(erased(&mut doc, mid, mid, 4.0), "edge at {mid}");
            let ink: Vec<Vec2> = strokes(&doc).concat();
            assert!(
                ink.iter().all(|p| p.distance(mid) >= 4.99),
                "gap around {mid}"
            );
            let total: f32 = strokes(&doc)
                .iter()
                .map(|p| p.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>())
                .sum();
            assert!(
                (total - 310.0).abs() < 0.01,
                "only 10 px cut from 320, got {total}"
            );
            assert!(
                doc.elements()
                    .iter()
                    .all(|e| matches!(e.kind, ElementKind::Stroke(_)))
            );
        }
    }

    #[test]
    fn pick_takes_the_topmost_ink_not_the_box() {
        let low = line(Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
        let high = line(Vec2::new(0.0, 100.0), Vec2::new(100.0, 0.0));
        let (low_id, high_id) = (low.id, high.id);
        let doc = Document::from_elements([low, high].map(Arc::new));
        assert_eq!(pick(&doc, Vec2::new(50.0, 50.0), 2.0), Some(high_id));
        assert_eq!(pick(&doc, Vec2::new(20.0, 22.0), 2.0), Some(low_id));
        assert_eq!(
            pick(&doc, Vec2::new(50.0, 10.0), 2.0),
            None,
            "inside both boxes, on no ink"
        );
    }

    #[test]
    fn marquee_and_lasso_take_only_enclosed_elements() {
        let a = line(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
        let b = line(Vec2::new(30.0, 10.0), Vec2::new(90.0, 10.0));
        let (ida, idb) = (a.id, b.id);
        let doc = Document::from_elements([a, b].map(Arc::new));
        assert_eq!(in_rect(&doc, Rect::new(0.0, 0.0, 50.0, 50.0)), [ida]);
        assert_eq!(in_rect(&doc, Rect::new(0.0, 0.0, 100.0, 50.0)), [ida, idb]);
        // A triangle around `a` that also crosses `b`.
        let lasso = [
            Vec2::new(0.0, 0.0),
            Vec2::new(60.0, 0.0),
            Vec2::new(0.0, 40.0),
        ];
        assert_eq!(in_lasso(&doc, &lasso), [ida]);
        assert!(in_lasso(&doc, &[]).is_empty());
    }
}
