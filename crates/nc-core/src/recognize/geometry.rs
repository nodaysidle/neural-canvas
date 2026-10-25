//! Geometry shared by the recognizers.

use glam::Vec2;
use kurbo::Rect;

/// Ramer–Douglas–Peucker simplification (the `ramer_douglas_peucker` crate): indices of the points
/// kept, first and last included. A closed path (end back at the start) is split at the point farthest
/// from the start, because RDP's start–end chord is degenerate there.
pub fn rdp(points: &[Vec2], epsilon: f32) -> Vec<usize> {
    let open = |points: &[Vec2]| {
        let pts: Vec<mint::Point2<f64>> = points
            .iter()
            .map(|p| mint::Point2 {
                x: p.x as f64,
                y: p.y as f64,
            })
            .collect();
        ramer_douglas_peucker::rdp(&pts, epsilon as f64)
    };
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    if points.len() < 3 || first.distance(last) > epsilon {
        return open(points);
    }
    let far = (0..points.len())
        .max_by(|&a, &b| {
            points[a]
                .distance_squared(first)
                .total_cmp(&points[b].distance_squared(first))
        })
        .unwrap_or(0);
    let mut kept = open(&points[..=far]);
    kept.extend(open(&points[far..]).into_iter().skip(1).map(|i| i + far));
    kept
}

/// Area of the overlap of two rectangles; 0 when they don't overlap.
pub fn overlap_area(a: Rect, b: Rect) -> f64 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    w * h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_keeps_corners_of_open_and_closed_paths() {
        // A wobbly L: the corner survives, the wobble does not.
        let l: Vec<Vec2> = (0..=10)
            .map(|i| Vec2::new(i as f32 * 10.0, (i % 2) as f32 * 0.5))
            .chain((1..=10).map(|i| Vec2::new(100.0, i as f32 * 10.0)))
            .collect();
        let kept: Vec<Vec2> = rdp(&l, 2.0).into_iter().map(|i| l[i]).collect();
        assert_eq!(kept.len(), 3, "{kept:?}");
        assert_eq!(kept[1].x, 100.0);

        // A closed square drawn from the top-left corner and back: four corners plus the closing point.
        let mut sq = Vec::new();
        for (a, b) in [
            (Vec2::ZERO, Vec2::new(100.0, 0.0)),
            (Vec2::new(100.0, 0.0), Vec2::splat(100.0)),
            (Vec2::splat(100.0), Vec2::new(0.0, 100.0)),
            (Vec2::new(0.0, 100.0), Vec2::ZERO),
        ] {
            sq.extend((0..10).map(|i| a.lerp(b, i as f32 / 10.0)));
        }
        sq.push(Vec2::ZERO);
        assert_eq!(rdp(&sq, 2.0).len(), 5);
    }

    #[test]
    fn overlap_is_zero_when_apart() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(overlap_area(a, Rect::new(5.0, 5.0, 20.0, 20.0)), 25.0);
        assert_eq!(overlap_area(a, Rect::new(20.0, 20.0, 30.0, 30.0)), 0.0);
    }
}
