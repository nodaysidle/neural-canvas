//! Whole-drawing recognition: joins strokes that draw one thing together (a rectangle in four strokes,
//! an arrow drawn shaft then head, a box with an X), then promotes geometry to UI elements by size and
//! nesting, with the Swift app's size rules applied to rotation-free dimensions.

use super::Recognized;
use super::primitive::{Oriented, Primitive, analyze};
use crate::element::{Element, ElementId, ElementKind, ShapeKind, ShapeType};
use glam::Vec2;
use kurbo::{Line, ParamCurveNearest, Point, Rect};
use std::sync::Arc;

/// One thing on the canvas, made of one or more strokes.
struct Part {
    ids: Vec<ElementId>,
    prim: Primitive,
    bounds: Rect,
    /// Ink width, for straightened bounds.
    width: f32,
}

fn point(v: Vec2) -> Point {
    Point::new(v.x as f64, v.y as f64)
}

fn part(e: &Element) -> Option<Part> {
    let (prim, width) = match &e.kind {
        ElementKind::Stroke(s) => (analyze(&s.points), s.width),
        ElementKind::Shape(s) => {
            // From the traced outline: the box around just the two drag corners has no height.
            let box_ = Oriented::around(&s.points());
            let prim = match s.kind {
                ShapeKind::Rectangle => Primitive::Rect {
                    box_,
                    radius: 0.0,
                    fit: 1.0,
                },
                ShapeKind::Ellipse => Primitive::Ellipse { box_, fit: 1.0 },
                ShapeKind::Line => Primitive::Line { a: s.from, b: s.to },
            };
            (prim, s.width)
        }
        ElementKind::Recognized(_) => return None,
    };
    Some(Part {
        ids: vec![e.id],
        prim,
        bounds: e.bounds()?,
        width,
    })
}

/// Where two (infinite) lines cross, if they are far from parallel.
fn corner(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> Option<Vec2> {
    let (da, db) = (a.1 - a.0, b.1 - b.0);
    let cross = da.perp_dot(db);
    if cross.abs() < 0.6 * da.length() * db.length() {
        return None; // closer than ~37° to parallel
    }
    let t = (b.0 - a.0).perp_dot(db) / cross;
    Some(a.0 + da * t)
}

fn as_line(p: &Option<Part>) -> Option<(Vec2, Vec2)> {
    match p {
        Some(Part {
            prim: Primitive::Line { a, b },
            ..
        }) => Some((*a, *b)),
        _ => None,
    }
}

fn merge(parts: &mut [Option<Part>], members: &[usize], prim: Primitive) {
    let mut ids = Vec::new();
    let mut bounds: Option<Rect> = None;
    let mut width = 0.0f32;
    for &i in members {
        let p = parts[i].take().expect("member not yet merged");
        ids.extend(p.ids);
        bounds = Some(bounds.map_or(p.bounds, |b| b.union(p.bounds)));
        width = width.max(p.width);
    }
    parts[members[0]] = Some(Part {
        ids,
        prim,
        bounds: bounds.expect("at least one member"),
        width,
    });
}

/// One end of a line part: (part index, 0 for its start or 1 for its end).
type End = (usize, usize);

/// Four lines whose ends meet at near-right angles: one rectangle.
fn join_rectangles(parts: &mut [Option<Part>]) {
    let lines: Vec<usize> = (0..parts.len())
        .filter(|&i| as_line(&parts[i]).is_some())
        .collect();
    // Corners: (line, end) ↔ (line, end) and where they meet.
    let mut links: Vec<(End, End, Vec2)> = Vec::new();
    for (k, &i) in lines.iter().enumerate() {
        for &j in &lines[k + 1..] {
            let (a, b) = (as_line(&parts[i]).unwrap(), as_line(&parts[j]).unwrap());
            let Some(x) = corner(a, b) else { continue };
            let tol = (0.25 * a.0.distance(a.1).min(b.0.distance(b.1))).max(14.0);
            let end = |l: (Vec2, Vec2)| {
                let e = if l.0.distance(x) < l.1.distance(x) {
                    0
                } else {
                    1
                };
                let at = if e == 0 { l.0 } else { l.1 };
                (at.distance(x) < tol).then_some(e)
            };
            if let (Some(ea), Some(eb)) = (end(a), end(b)) {
                links.push(((i, ea), (j, eb), x));
            }
        }
    }
    let linked = |from: (usize, usize)| {
        links.iter().filter_map(move |&(p, q, x)| {
            if p == from {
                Some((q, x))
            } else if q == from {
                Some((p, x))
            } else {
                None
            }
        })
    };
    let mut used = vec![false; parts.len()];
    for &a in &lines {
        if used[a] {
            continue;
        }
        let found = linked((a, 1)).find_map(|((b, eb), x1)| {
            linked((b, 1 - eb)).find_map(|((c, ec), x2)| {
                if c == a {
                    return None;
                }
                linked((c, 1 - ec)).find_map(|((d, ed), x3)| {
                    if d == a || d == b {
                        return None;
                    }
                    linked((d, 1 - ed))
                        .find(|&(end, _)| end == (a, 0))
                        .map(|(_, x4)| ([a, b, c, d], [x1, x2, x3, x4]))
                })
            })
        });
        if let Some((members, corners)) = found
            && members.iter().all(|&m| !used[m])
        {
            members.iter().for_each(|&m| used[m] = true);
            let prim = Primitive::Rect {
                box_: Oriented::around(&corners),
                radius: 0.0,
                fit: 1.0,
            };
            merge(parts, &members, prim);
        }
    }
}

/// A line with a chevron at one end, pointing back along it: an arrow drawn in two strokes.
fn join_arrows(parts: &mut [Option<Part>]) {
    for i in 0..parts.len() {
        let Some((a, b)) = as_line(&parts[i]) else {
            continue;
        };
        let len = a.distance(b);
        let head = (0..parts.len()).find_map(|j| {
            let Some(Part {
                prim: Primitive::Chevron { a: c0, apex, b: c1 },
                ..
            }) = &parts[j]
            else {
                return None;
            };
            let (tail, tip) = if apex.distance(b) < apex.distance(a) {
                (a, b)
            } else {
                (b, a)
            };
            let back = (tail - tip).normalize_or_zero();
            let reach = (0.2 * len).max(10.0);
            let arm = |c: Vec2| (c - *apex).normalize_or_zero().dot(back) > 0.3;
            let side = |c: Vec2| (c - tail).perp_dot(back).signum();
            (apex.distance(tip) < reach && arm(*c0) && arm(*c1) && side(*c0) != side(*c1))
                .then_some((j, tail, tip))
        });
        if let Some((j, tail, tip)) = head {
            merge(parts, &[i, j], Primitive::Arrow { tail, tip });
        }
    }
}

/// A rectangle with two long crossing lines inside: an image placeholder.
fn join_images(parts: &mut [Option<Part>]) -> Vec<usize> {
    let mut images = Vec::new();
    for r in 0..parts.len() {
        let Some(Part {
            prim: Primitive::Rect { box_, .. },
            bounds,
            ..
        }) = &parts[r]
        else {
            continue;
        };
        let diag = (box_.w * box_.w + box_.h * box_.h).sqrt();
        let inside = bounds.inflate(6.0, 6.0);
        let long_lines: Vec<usize> = (0..parts.len())
            .filter(|&i| {
                as_line(&parts[i]).is_some_and(|(a, b)| {
                    a.distance(b) > 0.6 * diag
                        && inside.contains(point(a))
                        && inside.contains(point(b))
                })
            })
            .collect();
        let cross = long_lines.iter().enumerate().find_map(|(k, &i)| {
            let (a0, a1) = as_line(&parts[i]).unwrap();
            long_lines[k + 1..].iter().find_map(|&j| {
                let (b0, b1) = as_line(&parts[j]).unwrap();
                let la = Line::new(point(a0), point(a1));
                let crosses = la
                    .crossing_point(Line::new(point(b0), point(b1)))
                    .is_some_and(|x| {
                        la.nearest(x, 0.0).distance_sq < 1e-6
                            && Line::new(point(b0), point(b1)).nearest(x, 0.0).distance_sq < 1e-6
                    });
                crosses.then_some(j).map(|j| (i, j))
            })
        });
        if let Some((i, j)) = cross {
            let prim = parts[r].as_ref().expect("checked above").prim.clone();
            merge(parts, &[r, i, j], prim);
            images.push(r);
        }
    }
    images
}

/// A shape plus its rotation-free size, for promotion.
struct Shape {
    rec: Recognized,
    size: Vec2,
    /// Lines only: the angle from level, degrees.
    tilt: Option<f32>,
}

/// The level rectangle a tilted, wobbly box stands for, ink width included: what the wireframe draws.
fn straightened(box_: &Oriented, width: f32) -> Rect {
    let c = box_.centre;
    Rect::from_center_size(
        (c.x as f64, c.y as f64),
        ((box_.w + width) as f64, (box_.h + width) as f64),
    )
}

fn base(part: Part, image: bool) -> Shape {
    let bounds = match &part.prim {
        Primitive::Rect { box_, .. }
        | Primitive::Ellipse { box_, .. }
        | Primitive::Scribble { box_ } => straightened(box_, part.width),
        _ => part.bounds,
    };
    let rec = |t, c| Recognized::new(t, bounds, c, part.ids.clone());
    let bounds_size = Vec2::new(bounds.width() as f32, bounds.height() as f32);
    match part.prim {
        Primitive::Rect { box_, radius, fit } => {
            let mut r = if image {
                rec(ShapeType::Image, 0.88)
            } else {
                rec(ShapeType::Rectangle, 0.6 + 0.3 * fit)
            };
            r.corner_radius = (radius > 3.0).then_some(radius);
            Shape {
                rec: r,
                size: Vec2::new(box_.w, box_.h),
                tilt: None,
            }
        }
        Primitive::Ellipse { box_, fit } => {
            let aspect = box_.w / box_.h.max(1e-3);
            let t = if (0.85..=1.18).contains(&aspect) {
                ShapeType::Circle
            } else {
                ShapeType::Ellipse
            };
            Shape {
                rec: rec(t, 0.6 + 0.3 * fit),
                size: Vec2::new(box_.w, box_.h),
                tilt: None,
            }
        }
        Primitive::Line { a, b } => {
            let d = b - a;
            let tilt = d.y.atan2(d.x).to_degrees();
            let mut r = rec(ShapeType::Line, 0.85);
            r.ends = Some([a, b]);
            Shape {
                rec: r,
                size: Vec2::new(d.length(), 0.0),
                tilt: Some(tilt),
            }
        }
        Primitive::Scribble { box_ } => Shape {
            rec: rec(ShapeType::Text, 0.75),
            size: Vec2::new(box_.w, box_.h),
            tilt: None,
        },
        Primitive::Arrow { tail, tip } => {
            let mut r = rec(ShapeType::Arrow, 0.8);
            r.ends = Some([tail, tip]);
            Shape {
                rec: r,
                size: bounds_size,
                tilt: None,
            }
        }
        Primitive::Chevron { .. } | Primitive::Other => Shape {
            rec: rec(ShapeType::Unknown, 0.3),
            size: bounds_size,
            tilt: None,
        },
    }
}

fn promote(i: usize, all: &[Shape]) -> Recognized {
    let s = &all[i];
    let mut r = s.rec.clone();
    let set = |r: &mut Recognized, t: ShapeType, c: f32, radius: Option<f32>| {
        r.shape_type = t;
        r.confidence = c.min(0.95);
        if radius.is_some() {
            r.corner_radius = radius;
        }
    };
    let (w, h) = (s.size.x, s.size.y);
    match r.shape_type {
        ShapeType::Line => {
            let level = s
                .tilt
                .is_some_and(|t| t.abs() < 4.0 || (180.0 - t.abs()) < 4.0);
            if level && w >= 80.0 {
                set(&mut r, ShapeType::Divider, 0.85, None);
            }
        }
        ShapeType::Circle | ShapeType::Ellipse if w.min(h) <= 64.0 => {
            set(&mut r, ShapeType::Icon, 0.7, None);
        }
        ShapeType::Rectangle => {
            let area = w * h;
            let outer = r.bounds.inflate(4.0, 4.0);
            let inside: Vec<&Shape> = all
                .iter()
                .enumerate()
                .filter(|(j, c)| {
                    *j != i
                        && outer.contains_rect(c.rec.bounds)
                        && c.rec.bounds.area() < r.bounds.area() * 0.65
                })
                .map(|(_, c)| c)
                .collect();
            let children = inside.len();
            let aspect = w / h.max(1.0);
            // How cleanly the box was drawn (its confidence, 0.6..=0.9) scales the rule's confidence.
            let quality = r.confidence / 0.9;
            // Wireframe convention: a button's label is centred, an input's placeholder starts at
            // the left edge.
            let label = match inside.as_slice() {
                [only] if only.rec.shape_type == ShapeType::Text => {
                    let (c, b) = (only.rec.bounds.center().x, r.bounds.center().x);
                    Some(((c - b) / r.bounds.width()) as f32)
                }
                _ => None,
            };
            let field = (24.0..=64.0).contains(&h) && aspect >= 1.4 && w >= 48.0;
            if field && label.is_some_and(|off| off.abs() < 0.12) {
                set(&mut r, ShapeType::Button, 0.85 * quality, Some(8.0));
            } else if field && aspect >= 2.5 && label.is_some_and(|off| off < -0.12) {
                set(&mut r, ShapeType::InputField, 0.85 * quality, Some(6.0));
            } else if area > 260_000.0 {
                set(&mut r, ShapeType::Container, 0.8 * quality, Some(12.0));
            } else if children >= 2 || area > 160_000.0 {
                set(&mut r, ShapeType::Card, 0.75 * quality, Some(12.0));
            } else if aspect >= 3.2 && (24.0..=64.0).contains(&h) && w >= 150.0 {
                set(&mut r, ShapeType::InputField, 0.82 * quality, Some(6.0));
            } else if (1.4..=5.5).contains(&aspect) && (24.0..=64.0).contains(&h) && w >= 48.0 {
                set(&mut r, ShapeType::Button, 0.8 * quality, Some(8.0));
            } else if aspect >= 4.0 && h <= 24.0 {
                set(&mut r, ShapeType::Text, 0.5, None);
            }
        }
        _ => {}
    }
    r
}

pub fn recognize(elements: &[Arc<Element>]) -> Vec<Recognized> {
    let mut parts: Vec<Option<Part>> = elements.iter().map(|e| part(e)).collect();
    join_rectangles(&mut parts);
    join_arrows(&mut parts);
    let images = join_images(&mut parts);
    let shapes: Vec<Shape> = parts
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| Some(base(p?, images.contains(&i))))
        .collect();
    (0..shapes.len()).map(|i| promote(i, &shapes)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::Shape;

    /// Shape-tool elements are exact: no guessing from samples.
    #[test]
    fn shapes_from_the_shape_tools_keep_their_kind_and_size() {
        let shape = |kind, from: Vec2, to: Vec2| {
            Arc::new(Element::new(ElementKind::Shape(Shape {
                kind,
                from,
                to,
                width: 2.0,
                color: [1.0; 4],
            })))
        };
        let out = recognize(&[
            shape(ShapeKind::Rectangle, Vec2::ZERO, Vec2::new(250.0, 150.0)),
            shape(
                ShapeKind::Ellipse,
                Vec2::new(400.0, 0.0),
                Vec2::new(640.0, 120.0),
            ),
            shape(
                ShapeKind::Line,
                Vec2::new(0.0, 300.0),
                Vec2::new(200.0, 300.0),
            ),
        ]);
        let types: Vec<_> = out.iter().map(|r| r.shape_type).collect();
        assert_eq!(
            types,
            [ShapeType::Rectangle, ShapeType::Ellipse, ShapeType::Divider]
        );
        let b = out[0].bounds;
        assert!(
            (b.width() - 252.0).abs() < 0.5 && (b.height() - 152.0).abs() < 0.5,
            "{b:?}"
        );
        assert_eq!(
            out[2].ends,
            Some([Vec2::new(0.0, 300.0), Vec2::new(200.0, 300.0)])
        );
    }
}
