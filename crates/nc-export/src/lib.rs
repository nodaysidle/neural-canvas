//! Export of sketch elements. SVG is written here and is the single source of truth: PNG is rendered
//! from it (resvg), and PDF will be converted from it (Phase 4). World units map 1:1 to SVG user units,
//! y-down like the canvas, so nothing flips (Mac bug #11).

use nc_core::{Element, ElementKind, Rect, ShapeKind, Stroke};
use resvg::{tiny_skia, usvg};
use std::fmt::Write;
use std::sync::Arc;

/// The canvas works in linear colour; SVG wants sRGB.
fn srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

fn paint(c: [f32; 4]) -> String {
    format!(
        "#{:02x}{:02x}{:02x}\" stroke-opacity=\"{}",
        srgb(c[0]),
        srgb(c[1]),
        srgb(c[2]),
        c[3]
    )
}

fn stroke_svg(out: &mut String, s: &Stroke) {
    let color = paint(s.color);
    if s.pressure.is_empty() {
        let mut d = String::new();
        for (i, p) in s.points.iter().enumerate() {
            let _ = write!(
                d,
                "{}{:.2} {:.2}",
                if i == 0 { "M" } else { " L" },
                p.x,
                p.y
            );
        }
        if s.points.len() == 1 {
            d.push_str(" z");
        }
        let _ = writeln!(
            out,
            r#"<path d="{d}" fill="none" stroke="{color}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
            s.width
        );
    } else {
        // Width varies along the stroke: one round-capped line per segment, as the canvas draws it.
        let _ = writeln!(out, r#"<g stroke="{color}" stroke-linecap="round">"#);
        for (i, w) in s.points.windows(2).enumerate() {
            let _ = writeln!(
                out,
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke-width="{:.2}"/>"#,
                w[0].x,
                w[0].y,
                w[1].x,
                w[1].y,
                2.0 * s.half_width(i)
            );
        }
        out.push_str("</g>\n");
    }
}

/// An SVG document showing `elements` in z-order, cropped to their bounds, optionally on a solid
/// `background`. `None` when nothing has bounds.
pub fn svg(elements: &[Arc<Element>], background: Option<[f32; 4]>) -> Option<String> {
    let b: Rect = elements
        .iter()
        .filter_map(|e| e.bounds())
        .reduce(|a, b| a.union(b))?
        .inflate(1.0, 1.0);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="{x:.2} {y:.2} {w:.2} {h:.2}">
"#,
        x = b.x0,
        y = b.y0,
        w = b.width().ceil(),
        h = b.height().ceil()
    );
    if let Some(bg) = background {
        let fill = paint(bg).replace("stroke-opacity", "fill-opacity");
        let _ = writeln!(
            out,
            r#"<rect x="{:.2}" y="{:.2}" width="100%" height="100%" fill="{fill}"/>"#,
            b.x0, b.y0
        );
    }
    for e in elements {
        match &e.kind {
            ElementKind::Stroke(s) => stroke_svg(&mut out, s),
            ElementKind::Shape(s) => {
                let r = Rect::from_points(
                    (s.from.x as f64, s.from.y as f64),
                    (s.to.x as f64, s.to.y as f64),
                );
                let style = format!(
                    r#"fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round""#,
                    paint(s.color),
                    s.width
                );
                let _ = match s.kind {
                    ShapeKind::Rectangle => writeln!(
                        out,
                        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" {style}/>"#,
                        r.x0,
                        r.y0,
                        r.width(),
                        r.height()
                    ),
                    ShapeKind::Ellipse => writeln!(
                        out,
                        r#"<ellipse cx="{:.2}" cy="{:.2}" rx="{:.2}" ry="{:.2}" {style}/>"#,
                        r.center().x,
                        r.center().y,
                        r.width() / 2.0,
                        r.height() / 2.0
                    ),
                    ShapeKind::Line => writeln!(
                        out,
                        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" {style}/>"#,
                        s.from.x, s.from.y, s.to.x, s.to.y
                    ),
                };
            }
            // shortcut: recognised shapes have no export yet; Phase 4 adds the wireframe export.
            ElementKind::Recognized(_) => {}
        }
    }
    out.push_str("</svg>\n");
    Some(out)
}

#[derive(Debug)]
pub struct ExportError(String);

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExportError {}

/// Renders an SVG to PNG bytes at `scale` device px per unit.
pub fn png(svg: &str, scale: f32) -> Result<Vec<u8>, ExportError> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
        .map_err(|e| ExportError(e.to_string()))?;
    let size = tree
        .size()
        .to_int_size()
        .scale_by(scale)
        .ok_or_else(|| ExportError("image too large".into()))?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| ExportError("image too large".into()))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| ExportError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use nc_core::Shape;

    fn doc() -> Vec<Arc<Element>> {
        let mut s = Stroke::new(Vec2::new(10.0, 10.0), 4.0, [1.0, 0.0, 0.0, 1.0]);
        s.push(Vec2::new(90.0, 10.0));
        let rect = Shape {
            kind: ShapeKind::Rectangle,
            from: Vec2::new(10.0, 30.0),
            to: Vec2::new(90.0, 60.0),
            width: 2.0,
            color: [0.0, 0.0, 1.0, 1.0],
        };
        vec![
            Arc::new(Element::stroke(s)),
            Arc::new(Element::new(ElementKind::Shape(rect))),
        ]
    }

    #[test]
    fn svg_keeps_y_down_and_native_shapes() {
        let svg = svg(&doc(), None).unwrap();
        assert!(
            svg.contains(r#"<path d="M10.00 10.00 L90.00 10.00""#),
            "{svg}"
        );
        assert!(svg.contains(r#"<rect x="10.00" y="30.00" width="80.00" height="30.00""#));
        assert!(svg.contains(r##"stroke="#ff0000""##));
        assert!(svg.contains(r#"viewBox="7.00 7.00 86.00 55.00""#), "{svg}");
        assert!(super::svg(&[], None).is_none());
    }

    /// "Top stays top": the stroke at y = 10 lands in the top rows of the PNG, the rectangle below it.
    #[test]
    fn png_renders_the_svg_the_right_way_up() {
        let bytes = png(&svg(&doc(), None).unwrap(), 2.0).unwrap();
        let pixmap = tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (172, 110));
        let at = |x: u32, y: u32| pixmap.pixel(x, y).unwrap();
        // World (50, 10) → px ((50 - 7) * 2, (10 - 7) * 2).
        let top = at(86, 6);
        assert!(
            top.red() > 200 && top.blue() < 50,
            "red stroke on top: {top:?}"
        );
        let bottom = at(86, 106);
        assert!(
            bottom.blue() > 200 && bottom.red() < 50,
            "blue rect edge below: {bottom:?}"
        );
        assert_eq!(at(86, 40).alpha(), 0, "transparent between");
    }
}
