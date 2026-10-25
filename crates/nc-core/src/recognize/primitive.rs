//! Per-stroke analysis: what one stroke draws, independent of its neighbours.

use super::geometry::rdp;
use glam::Vec2;
use kurbo::{Line, ParamCurveNearest, Point};

/// A rectangle at any rotation: centre, unit direction of its `w` side, side lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oriented {
    pub centre: Vec2,
    pub axis: Vec2,
    pub w: f32,
    pub h: f32,
}

impl Oriented {
    /// The smallest rectangle around `points` within ±45° of level (strokes are drawn near level, and
    /// ±45° covers every orientation of a rectangle once). Searched in 1° steps, then 0.1°.
    pub fn around(points: &[Vec2]) -> Self {
        let fit = |deg: f32| {
            let axis = Vec2::from_angle(deg.to_radians());
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in points {
                let q = Vec2::new(p.dot(axis), -p.perp_dot(axis));
                lo = lo.min(q);
                hi = hi.max(q);
            }
            let size = hi - lo;
            let mid = (lo + hi) * 0.5;
            let centre = axis * mid.x + axis.perp() * mid.y;
            (
                size.x * size.y,
                Self {
                    centre,
                    axis,
                    w: size.x,
                    h: size.y,
                },
            )
        };
        let best = |range: &mut dyn Iterator<Item = f32>| {
            range
                .map(|d| (d, fit(d)))
                .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
                .expect("non-empty search")
        };
        let (coarse, _) = best(&mut (-45..=45).map(|d| d as f32));
        best(&mut (-10..=10).map(|i| coarse + i as f32 * 0.1)).1.1
    }

    /// Tilt of the `w` side from level, degrees.
    pub fn tilt(&self) -> f32 {
        self.axis.y.atan2(self.axis.x).to_degrees()
    }
}

/// What a single stroke is.
#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    /// A straight open stroke.
    Line { a: Vec2, b: Vec2 },
    /// Handwriting: an elongated, near-level, back-and-forth open stroke.
    Scribble { box_: Oriented },
    /// A shaft with a head at `tip`, in one stroke.
    Arrow { tail: Vec2, tip: Vec2 },
    /// A three-vertex open stroke (an arrowhead drawn on its own).
    Chevron { a: Vec2, apex: Vec2, b: Vec2 },
    /// A closed four-cornered stroke.
    Rect {
        box_: Oriented,
        radius: f32,
        fit: f32,
    },
    /// A closed round stroke.
    Ellipse { box_: Oriented, fit: f32 },
    /// Anything else.
    Other,
}

fn path_length(points: &[Vec2]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// |Shoelace area| of the closed polygon through `points`.
fn polygon_area(points: &[Vec2]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        * 0.5
}

/// Mean |r − 1| with points normalised to the unit circle of `box_` (0 for a perfect ellipse).
fn ellipse_error(points: &[Vec2], box_: &Oriented) -> f32 {
    let (rx, ry) = (box_.w / 2.0, box_.h / 2.0);
    if rx < 1e-3 || ry < 1e-3 {
        return f32::MAX;
    }
    let sum: f32 = points
        .iter()
        .map(|p| {
            let d = *p - box_.centre;
            let u = Vec2::new(d.dot(box_.axis) / rx, d.dot(box_.axis.perp()) / ry);
            (u.length() - 1.0).abs()
        })
        .sum();
    sum / points.len() as f32
}

pub fn analyze(points: &[Vec2]) -> Primitive {
    if points.len() < 2 {
        return Primitive::Other;
    }
    let (start, end) = (points[0], points[points.len() - 1]);
    let length = path_length(points);
    let chord = start.distance(end);
    let box_ = Oriented::around(points);
    let size = box_.w.max(box_.h);
    if size < 1.0 {
        return Primitive::Other;
    }
    // Back near the start after going round: a thin rectangle's outline is just over twice its length.
    let closed = chord < (0.2 * size).max(12.0) && length > 2.1 * size;
    if closed {
        return closed_shape(points, box_);
    }
    // A hand-drawn straight line is within 2 % of its chord; handwriting goes up and down.
    if length <= 1.02 * chord {
        return Primitive::Line { a: start, b: end };
    }
    // Before the scribble test: a level arrow is long and thin too, but its bends crowd one end.
    let eps = (0.04 * size).max(2.0);
    let verts: Vec<Vec2> = rdp(points, eps).into_iter().map(|i| points[i]).collect();
    if let Some(arrow) = arrow(&verts) {
        return arrow;
    }
    // The box's `w` side is within ±45° of level, so a level scribble has it long and barely tilted.
    if box_.tilt().abs() < 15.0 && box_.w >= 3.0 * box_.h && (2.5..=30.0).contains(&box_.h) {
        return Primitive::Scribble { box_ };
    }
    if let Some(chevron) = chevron(points) {
        return chevron;
    }
    if verts.len() == 2 {
        return Primitive::Line { a: start, b: end };
    }
    Primitive::Other
}

fn closed_shape(points: &[Vec2], box_: Oriented) -> Primitive {
    let ell = ellipse_error(points, &box_);
    // Close to an ellipse wins first: a coarsely sampled small circle is a polygon that fills its box
    // like a rounded square, but its radius barely varies (rectangles are ≥ 0.1 off).
    if ell < 0.06 {
        return Primitive::Ellipse {
            box_,
            fit: (1.0 - ell / 0.09).clamp(0.0, 1.0),
        };
    }
    let fill = polygon_area(points) / (box_.w * box_.h).max(1e-3);
    // A rectangle fills its box (1.0), an ellipse π/4 ≈ 0.785 of it; hand wobble costs a few %.
    if fill > 0.82 {
        // Rounded corners take (4 − π) r² from the box: solve for r.
        let missing = ((1.0 - fill) * box_.w * box_.h).max(0.0);
        let radius = (missing / (4.0 - std::f32::consts::PI)).sqrt();
        return Primitive::Rect {
            box_,
            radius: radius.min(box_.w.min(box_.h) / 2.0),
            fit: ((fill - 0.82) / 0.14).clamp(0.0, 1.0),
        };
    }
    if ell < 0.09 {
        return Primitive::Ellipse {
            box_,
            fit: (1.0 - ell / 0.09).clamp(0.0, 1.0),
        };
    }
    Primitive::Other
}

/// An open stroke bent once, sharply: the point farthest from the start–end chord is well off it,
/// and both arms are straight (an arrowhead drawn on its own).
fn chevron(points: &[Vec2]) -> Option<Primitive> {
    let (a, b) = (points[0], points[points.len() - 1]);
    let chord = Line::new(pt(a), pt(b));
    let (k, off) = points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, chord.nearest(pt(*p), 0.0).distance_sq.sqrt() as f32))
        .max_by(|x, y| x.1.total_cmp(&y.1))?;
    let apex = points[k];
    if k == 0 || k == points.len() - 1 || off < 0.3 * a.distance(b).max(1.0) {
        return None;
    }
    let straight = |arm: &[Vec2]| {
        let seg = Line::new(pt(arm[0]), pt(arm[arm.len() - 1]));
        let len = arm[0].distance(arm[arm.len() - 1]);
        arm.iter()
            .all(|p| (seg.nearest(pt(*p), 0.0).distance_sq.sqrt() as f32) < 0.2 * len + 1.5)
    };
    (straight(&points[..=k]) && straight(&points[k..])).then_some(Primitive::Chevron { a, apex, b })
}

fn pt(v: Vec2) -> Point {
    Point::new(v.x as f64, v.y as f64)
}

/// A single-stroke arrow: one long shaft, then a short head (two barbs, one each side) at its tip.
fn arrow(verts: &[Vec2]) -> Option<Primitive> {
    if verts.len() < 4 {
        return None;
    }
    let shaft = (0..verts.len() - 1)
        .max_by(|&a, &b| {
            verts[a]
                .distance(verts[a + 1])
                .total_cmp(&verts[b].distance(verts[b + 1]))
        })
        .expect("at least one segment");
    let (s0, s1) = (verts[shaft], verts[shaft + 1]);
    let len = s0.distance(s1);
    let rest: Vec<Vec2> = verts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != shaft && *i != shaft + 1)
        .map(|(_, v)| *v)
        .collect();
    // The head is at whichever shaft end the other vertices crowd around.
    let (tail, tip) = if rest.iter().all(|v| v.distance(s1) < 0.45 * len) {
        (s0, s1)
    } else if rest.iter().all(|v| v.distance(s0) < 0.45 * len) {
        (s1, s0)
    } else {
        return None;
    };
    let dir = (tip - tail) / len;
    let barbs: Vec<Vec2> = rest
        .into_iter()
        .filter(|v| v.distance(tip) > 0.08 * len && (*v - tip).dot(dir) < 0.0)
        .collect();
    let side = |v: &Vec2| (*v - tail).perp_dot(dir).signum();
    let both_sides = barbs.iter().any(|v| side(v) > 0.0) && barbs.iter().any(|v| side(v) < 0.0);
    both_sides.then_some(Primitive::Arrow { tail, tip })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(corners: &[Vec2], step: f32) -> Vec<Vec2> {
        let mut out = vec![corners[0]];
        for w in corners.windows(2) {
            let n = (w[0].distance(w[1]) / step).ceil() as usize;
            out.extend((1..=n).map(|i| w[0].lerp(w[1], i as f32 / n as f32)));
        }
        out
    }

    fn rect(w: f32, h: f32, tilt_deg: f32) -> Vec<Vec2> {
        let r = Vec2::from_angle(tilt_deg.to_radians());
        let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h), (0.0, 0.0)]
            .map(|(x, y)| r.rotate(Vec2::new(x, y)));
        sample(&c, 4.0)
    }

    #[test]
    fn oriented_box_undoes_tilt() {
        let b = Oriented::around(&rect(300.0, 40.0, 3.0));
        assert!(
            (b.w - 300.0).abs() < 1.0 && (b.h - 40.0).abs() < 1.0,
            "{b:?}"
        );
        assert!((b.tilt() - 3.0).abs() < 0.2);
    }

    #[test]
    fn closed_strokes_are_rects_or_ellipses_at_any_tilt() {
        for tilt in [-3.0, 0.0, 4.0] {
            assert!(matches!(
                analyze(&rect(120.0, 120.0, tilt)),
                Primitive::Rect { .. }
            ));
            assert!(matches!(
                analyze(&rect(600.0, 480.0, tilt)),
                Primitive::Rect { .. }
            ));
        }
        let ellipse: Vec<Vec2> = (0..=80)
            .map(|i| {
                let a = i as f32 / 80.0 * std::f32::consts::TAU;
                Vec2::new(120.0 * a.cos(), 60.0 * a.sin())
            })
            .collect();
        assert!(matches!(analyze(&ellipse), Primitive::Ellipse { .. }));
    }

    #[test]
    fn open_strokes_are_lines_scribbles_arrows_and_chevrons() {
        let line = sample(&[Vec2::ZERO, Vec2::new(200.0, 30.0)], 4.0);
        assert!(matches!(analyze(&line), Primitive::Line { .. }));
        let scribble: Vec<Vec2> = (0..60)
            .map(|i| Vec2::new(i as f32 * 3.0, if i % 4 < 2 { 0.0 } else { 10.0 }))
            .collect();
        assert!(matches!(analyze(&scribble), Primitive::Scribble { .. }));
        let tip = Vec2::new(200.0, 0.0);
        let arrow = sample(
            &[
                Vec2::ZERO,
                tip,
                Vec2::new(180.0, -12.0),
                tip,
                Vec2::new(180.0, 12.0),
            ],
            3.0,
        );
        assert_eq!(
            analyze(&arrow),
            Primitive::Arrow {
                tail: Vec2::ZERO,
                tip
            }
        );
        let chevron = sample(
            &[Vec2::new(-20.0, -12.0), Vec2::ZERO, Vec2::new(-20.0, 12.0)],
            3.0,
        );
        assert!(matches!(analyze(&chevron), Primitive::Chevron { .. }));
    }
}
