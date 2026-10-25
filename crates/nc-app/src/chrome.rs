//! Canvas chrome drawn as world-space capsules in the renderer's overlay: the selection box and its
//! handles, the marquee and the lasso. Sizes are logical screen px, constant at every zoom.

use glam::Vec2;
use nc_core::select::{HANDLE_RADIUS, Handle};
use nc_core::{Rect, Segment, Viewport};

/// The one accent colour (linear RGB).
pub const ACCENT: [f32; 4] = [0.15, 0.42, 1.0, 1.0];
const HANDLE_FILL: [f32; 4] = [0.92, 0.94, 1.0, 1.0];
/// Quiet chrome: a little brighter than the grid's major lines.
const DIVIDER: [f32; 4] = [0.08, 0.09, 0.12, 1.0];
/// Outline half width, logical px.
const LINE: f32 = 0.6;

fn corners(r: Rect) -> [Vec2; 5] {
    let (a, b) = (
        Vec2::new(r.x0 as f32, r.y0 as f32),
        Vec2::new(r.x1 as f32, r.y1 as f32),
    );
    [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y), a]
}

pub fn polyline(out: &mut Vec<Segment>, points: &[Vec2], closed: bool, view: &Viewport) {
    let half_width = LINE / view.scale;
    let pair = |a: Vec2, b: Vec2| Segment {
        a,
        b,
        half_width,
        color: ACCENT,
    };
    out.extend(points.windows(2).map(|w| pair(w[0], w[1])));
    if closed && let (Some(&first), Some(&last)) = (points.first(), points.last()) {
        out.push(pair(last, first));
    }
}

pub fn rect(out: &mut Vec<Segment>, r: Rect, view: &Viewport) {
    polyline(out, &corners(r), false, view);
}

/// The element "Review uncertain items" is showing: a thicker amber box a little outside it.
pub fn highlight(out: &mut Vec<Segment>, r: Rect, view: &Viewport) {
    let pad = 6.0 / view.scale as f64;
    let half_width = 1.5 / view.scale;
    let c = corners(r.inflate(pad, pad));
    out.extend(c.windows(2).map(|w| Segment {
        a: w[0],
        b: w[1],
        half_width,
        color: nc_core::recognize::draw::UNCERTAIN,
    }));
}

/// Side by side: a hairline down the left pane's right edge (the right pane clips it away).
pub fn pane_divider(out: &mut Vec<Segment>, pane: Vec2, view: &Viewport) {
    let x = view.screen_to_world(Vec2::new(pane.x, 0.0)).x;
    let (top, bottom) = (
        view.screen_to_world(Vec2::ZERO).y,
        view.screen_to_world(pane).y,
    );
    out.push(Segment {
        a: Vec2::new(x, top),
        b: Vec2::new(x, bottom),
        half_width: 1.0 / view.scale,
        color: DIVIDER,
    });
}

/// The selection box with its eight handles (accent ring, light centre).
pub fn selection(out: &mut Vec<Segment>, bounds: Rect, view: &Viewport) {
    rect(out, bounds, view);
    for h in Handle::ALL {
        let at = h.position(bounds);
        for (radius, color) in [(HANDLE_RADIUS, ACCENT), (HANDLE_RADIUS - 1.5, HANDLE_FILL)] {
            out.push(Segment {
                a: at,
                b: at,
                half_width: radius / view.scale,
                color,
            });
        }
    }
}

/// The eraser cursor: a light ring with a dark halo (visible on ink and on the canvas) of `radius`
/// px, as straight RGBA, with its hotspot at the centre.
// shortcut: drawn at scale 1; winit 0.30 gives custom cursors no buffer scale, so it is blurry on HiDPI
// outputs until winit 0.31.
pub fn ring_cursor(radius: f32) -> (Vec<u8>, u16) {
    let size = (radius * 2.0 + 4.0).ceil() as u16;
    let c = size as f32 / 2.0;
    let mut rgba = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size {
        for x in 0..size {
            let d = (Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - Vec2::splat(c)).length();
            let off = (d - radius).abs();
            let ring = (1.5 - off).clamp(0.0, 1.0);
            let halo = (2.5 - off).clamp(0.0, 1.0);
            let value = (255.0 * ring) as u8;
            let alpha = (255.0 * halo.max(ring) * 0.95) as u8;
            rgba.extend([value, value, value, alpha]);
        }
    }
    (rgba, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_stay_8_px_at_any_zoom() {
        let view = Viewport {
            offset: Vec2::ZERO,
            scale: 4.0,
        };
        let mut out = Vec::new();
        selection(&mut out, Rect::new(0.0, 0.0, 10.0, 10.0), &view);
        let outer = out.iter().map(|s| s.half_width).fold(0.0, f32::max);
        assert_eq!(outer * view.scale * 2.0, 8.0);
        assert_eq!(out.len(), 4 + 16);
    }

    #[test]
    fn ring_cursor_is_a_ring() {
        let (rgba, size) = ring_cursor(8.0);
        assert_eq!(size, 20);
        let px = |x: usize, y: usize| &rgba[(y * size as usize + x) * 4..][..4];
        assert_eq!(px(10, 10)[3], 0, "clear centre");
        assert!(px(18, 10)[0] > 200, "light ring at the radius");
        assert_eq!(px(0, 0)[3], 0, "clear corners");
    }
}
