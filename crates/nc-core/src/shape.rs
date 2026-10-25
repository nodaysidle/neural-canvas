//! The rectangle, ellipse and line tools: a drag becomes a [`Shape`], with `Shift` to constrain (square,
//! circle, 45° line) and `Alt` to draw out from the press point as the centre.

use crate::element::{Shape, ShapeKind};
use glam::Vec2;
use std::f32::consts::FRAC_PI_4;

/// The shape for a drag from `start` to `current` (world units).
pub fn from_drag(
    kind: ShapeKind,
    start: Vec2,
    current: Vec2,
    constrain: bool,
    from_centre: bool,
    width: f32,
    color: [f32; 4],
) -> Shape {
    let mut delta = current - start;
    if constrain {
        delta = match kind {
            ShapeKind::Rectangle | ShapeKind::Ellipse => {
                let side = delta.abs().max_element();
                Vec2::new(side.copysign(delta.x), side.copysign(delta.y))
            }
            ShapeKind::Line => {
                let angle = (delta.y.atan2(delta.x) / FRAC_PI_4).round() * FRAC_PI_4;
                Vec2::from_angle(angle) * delta.length()
            }
        };
    }
    let from = if from_centre { start - delta } else { start };
    Shape {
        kind,
        from,
        to: start + delta,
        width,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(kind: ShapeKind, to: Vec2, constrain: bool, centre: bool) -> (Vec2, Vec2) {
        let s = from_drag(
            kind,
            Vec2::new(10.0, 10.0),
            to,
            constrain,
            centre,
            2.0,
            [1.0; 4],
        );
        (s.from, s.to)
    }

    #[test]
    fn plain_drag_spans_press_to_pointer() {
        let to = Vec2::new(40.0, 25.0);
        assert_eq!(
            drag(ShapeKind::Rectangle, to, false, false),
            (Vec2::new(10.0, 10.0), to)
        );
    }

    #[test]
    fn shift_makes_squares_and_circles_in_the_drag_direction() {
        for kind in [ShapeKind::Rectangle, ShapeKind::Ellipse] {
            assert_eq!(
                drag(kind, Vec2::new(40.0, 20.0), true, false).1,
                Vec2::new(40.0, 40.0)
            );
            assert_eq!(
                drag(kind, Vec2::new(-5.0, 15.0), true, false).1,
                Vec2::new(-5.0, 25.0),
                "up-left drags keep their direction"
            );
        }
    }

    #[test]
    fn shift_snaps_lines_to_45_degrees() {
        let (_, to) = drag(ShapeKind::Line, Vec2::new(50.0, 14.0), true, false);
        assert!((to.y - 10.0).abs() < 1e-4, "horizontal: {to}");
        let (_, to) = drag(ShapeKind::Line, Vec2::new(40.0, 37.0), true, false);
        assert!(((to - Vec2::splat(10.0)).angle_to(Vec2::X).abs() - FRAC_PI_4).abs() < 1e-5);
    }

    #[test]
    fn alt_draws_from_the_centre() {
        assert_eq!(
            drag(ShapeKind::Ellipse, Vec2::new(30.0, 20.0), false, true),
            (Vec2::new(-10.0, 0.0), Vec2::new(30.0, 20.0))
        );
        assert_eq!(
            drag(ShapeKind::Rectangle, Vec2::new(30.0, 15.0), true, true),
            (Vec2::new(-10.0, -10.0), Vec2::new(30.0, 30.0)),
            "both modifiers: a square centred on the press"
        );
    }
}
