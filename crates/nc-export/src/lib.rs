//! Export of the raw sketch and of its wireframe. SVG is written here and is the single source of
//! truth: PNG and JPEG are rendered from it (resvg), PDF is converted from it (svg2pdf). World units map 1:1 to SVG user units,
//! y-down like the canvas, so nothing flips (Mac bug #11).

use nc_core::recognize::Wireframe;
use nc_core::recognize::draw::{self, Mark};
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

/// Ink as the chosen theme shows it: on a light canvas, lightness inverted (`nc_core::theme`).
fn ink(c: [f32; 4], light: bool) -> [f32; 4] {
    if light {
        nc_core::theme::ink_on_light(c)
    } else {
        c
    }
}

fn stroke_svg(out: &mut String, s: &Stroke, light: bool) {
    let color = paint(ink(s.color, light));
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

/// The `<svg>` opening for content within `bounds`, plus the background.
fn header(bounds: Rect, background: Option<[f32; 4]>) -> String {
    let b = bounds.inflate(1.0, 1.0);
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
    out
}

/// An SVG document of `elements` (raw sketch) in z-order, cropped to their bounds, optionally on a solid
/// `background`. `None` when nothing has bounds.
pub fn svg(elements: &[Arc<Element>], background: Option<[f32; 4]>, light: bool) -> Option<String> {
    let b: Rect = elements
        .iter()
        .filter_map(|e| e.bounds())
        .reduce(|a, b| a.union(b))?;
    let mut out = header(b, background);
    for e in elements {
        match &e.kind {
            ElementKind::Stroke(s) => stroke_svg(&mut out, s, light),
            ElementKind::Shape(s) => {
                let r = Rect::from_points(
                    (s.from.x as f64, s.from.y as f64),
                    (s.to.x as f64, s.to.y as f64),
                );
                let style = format!(
                    r#"fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round""#,
                    paint(ink(s.color, light)),
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

/// The wireframe as SVG, drawn from the same marks as the canvas overlay (uncertain elements in their
/// own colour). `None` when it is empty.
pub fn wireframe_svg(
    wire: &Wireframe,
    background: Option<[f32; 4]>,
    light: bool,
) -> Option<String> {
    let b = wire
        .shapes
        .iter()
        .map(|s| s.bounds)
        .reduce(|a, b| a.union(b))?
        .inflate(draw::LINE_WIDTH as f64, draw::LINE_WIDTH as f64);
    let mut out = header(b, background);
    let mut marks = Vec::new();
    for shape in &wire.shapes {
        marks.clear();
        draw::marks(shape, &mut marks);
        let color = draw::color(light, shape.is_uncertain());
        let stroke = format!(
            r#"fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round""#,
            paint(color),
            draw::LINE_WIDTH
        );
        if let Some((at, centred)) = draw::label_anchor(shape) {
            let _ = writeln!(
                out,
                r#"<text x="{:.2}" y="{:.2}" font-family="Manrope" font-size="{}" fill="{}" text-anchor="{}" dominant-baseline="central">{}</text>"#,
                at.x,
                at.y,
                draw::LABEL_SIZE,
                paint(color).replace("stroke-opacity", "fill-opacity"),
                if centred { "middle" } else { "start" },
                xml_escape(&shape.label)
            );
        }
        for m in &marks {
            let _ = match *m {
                Mark::Rect { rect: r, radius } => writeln!(
                    out,
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{radius:.2}" {stroke}/>"#,
                    r.x0,
                    r.y0,
                    r.width(),
                    r.height()
                ),
                Mark::Ellipse { rect: r } => writeln!(
                    out,
                    r#"<ellipse cx="{:.2}" cy="{:.2}" rx="{:.2}" ry="{:.2}" {stroke}/>"#,
                    r.center().x,
                    r.center().y,
                    r.width() / 2.0,
                    r.height() / 2.0
                ),
                Mark::Line { a, b } => writeln!(
                    out,
                    r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" {stroke}/>"#,
                    a.x, a.y, b.x, b.y
                ),
                // As the canvas draws it: one round-capped line as thick as the bar, at 55 % opacity.
                Mark::Bar { rect: r } => {
                    let (h, cy) = (r.height(), r.center().y);
                    writeln!(
                        out,
                        r#"<line x1="{:.2}" y1="{cy:.2}" x2="{:.2}" y2="{cy:.2}" stroke="{}" stroke-width="{h:.2}" stroke-linecap="round"/>"#,
                        r.x0 + h / 2.0,
                        (r.x1 - h / 2.0).max(r.x0 + h / 2.0),
                        paint([color[0], color[1], color[2], color[3] * 0.55])
                    )
                }
            };
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

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn tree(svg: &str) -> Result<usvg::Tree, ExportError> {
    // Labels render in the bundled Manrope, never a system font: the same output on every machine.
    let mut options = usvg::Options::default();
    let db = options.fontdb_mut();
    db.load_font_data(nc_render::ui::fonts::MANROPE.to_vec());
    db.load_font_data(nc_render::ui::fonts::MANROPE_SEMIBOLD.to_vec());
    usvg::Tree::from_str(svg, &options).map_err(|e| ExportError(e.to_string()))
}

/// Raster exports never exceed this many pixels on their long side: a large sketch at 2× would
/// otherwise need hundreds of MB. They render at the largest scale up to the one asked for that fits.
pub const MAX_RASTER_PX: f32 = 8192.0;

fn raster(svg: &str, scale: f32) -> Result<tiny_skia::Pixmap, ExportError> {
    let tree = tree(svg)?;
    let long = tree.size().width().max(tree.size().height());
    let scale = scale.min(MAX_RASTER_PX / long.max(1.0));
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
    Ok(pixmap)
}

/// Renders an SVG to PNG bytes at `scale` device px per unit.
pub fn png(svg: &str, scale: f32) -> Result<Vec<u8>, ExportError> {
    raster(svg, scale)?
        .encode_png()
        .map_err(|e| ExportError(e.to_string()))
}

/// Renders an SVG to JPEG at `scale`. JPEG has no transparency: give the SVG a background.
pub fn jpeg(svg: &str, scale: f32) -> Result<Vec<u8>, ExportError> {
    let pixmap = raster(svg, scale)?;
    // Opaque pixels once a background is drawn, so premultiplied RGB is plain RGB.
    let rgb: Vec<u8> = pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)
        .encode(
            &rgb,
            pixmap.width(),
            pixmap.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| ExportError(e.to_string()))?;
    Ok(out)
}

/// Converts an SVG to a one-page PDF (svg2pdf), 1 unit = 1 pt.
pub fn pdf(svg: &str) -> Result<Vec<u8>, ExportError> {
    svg2pdf::to_pdf(
        &tree(svg)?,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .map_err(|e| ExportError(e.to_string()))
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
        let svg = svg(&doc(), None, false).unwrap();
        assert!(
            svg.contains(r#"<path d="M10.00 10.00 L90.00 10.00""#),
            "{svg}"
        );
        assert!(svg.contains(r#"<rect x="10.00" y="30.00" width="80.00" height="30.00""#));
        assert!(svg.contains(r##"stroke="#ff0000""##));
        assert!(svg.contains(r#"viewBox="7.00 7.00 86.00 55.00""#), "{svg}");
        assert!(super::svg(&[], None, false).is_none());
    }

    fn wire() -> Wireframe {
        nc_core::recognize::recognize(&doc())
    }

    #[test]
    fn wireframe_svg_uses_native_elements_in_world_units() {
        let svg = wireframe_svg(&wire(), None, false).unwrap();
        assert!(svg.contains("<line "), "{svg}");
        assert!(svg.contains("<rect ") && svg.contains(" rx="), "{svg}");
        assert!(wireframe_svg(&Wireframe::default(), None, false).is_none());
    }

    /// Red (or wireframe blue) ink in the top band, the rectangle's edge in the bottom band, nothing
    /// in between: the output is the right way up. `px` maps world y to image rows.
    fn assert_top_stays_top(
        rgba: impl Fn(u32, u32) -> [u8; 4],
        top: u32,
        mid: u32,
        bottom: u32,
        x: u32,
    ) {
        let lit = |y: u32| (0..3).any(|dy| rgba(x, y + dy).iter().take(3).any(|c| *c > 150));
        assert!(lit(top - 1), "ink at the top (row {top})");
        assert!(!lit(mid - 1), "nothing between (row {mid})");
        assert!(lit(bottom - 1), "ink at the bottom (row {bottom})");
    }

    /// "Top stays top": the stroke at y = 10 lands in the top rows of the PNG, the rectangle below it.
    #[test]
    fn png_renders_the_svg_the_right_way_up() {
        let bytes = png(&svg(&doc(), None, false).unwrap(), 2.0).unwrap();
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

    #[test]
    fn jpeg_is_opaque_and_the_right_way_up() {
        let svg = svg(&doc(), Some([0.0, 0.0, 0.0, 1.0]), false).unwrap();
        let bytes = jpeg(&svg, 2.0).unwrap();
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
            .unwrap()
            .to_rgba8();
        assert_eq!(img.dimensions(), (172, 110));
        // World y 10 → row 6, y 40 (between) → row 66, y 60 → row 106.
        assert_top_stays_top(|x, y| img.get_pixel(x, y).0, 6, 66, 106, 86);
    }

    /// The PDF, rasterised by poppler (`pdftoppm`, as PDF viewers on Linux use), is the right way up.
    /// Skipped without poppler.
    #[test]
    fn pdf_is_the_right_way_up_in_a_real_pdf_renderer() {
        let pdf = pdf(&svg(&doc(), Some([0.0, 0.0, 0.0, 1.0]), false).unwrap()).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        let dir = std::env::temp_dir().join(format!("nc-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.pdf"), &pdf).unwrap();
        // 144 dpi = 2 px per pt, like the PNG test.
        let ran = std::process::Command::new("pdftoppm")
            .args(["-png", "-r", "144", "-singlefile"])
            .arg(dir.join("t.pdf"))
            .arg(dir.join("t"))
            .status();
        let Ok(status) = ran else {
            eprintln!("pdftoppm not installed; skipping the PDF raster check");
            return;
        };
        assert!(status.success());
        let png =
            tiny_skia::Pixmap::decode_png(&std::fs::read(dir.join("t.png")).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((png.width(), png.height()), (172, 110));
        assert_top_stays_top(
            |x, y| {
                let p = png.pixel(x, y).unwrap();
                [p.red(), p.green(), p.blue(), p.alpha()]
            },
            6,
            66,
            106,
            86,
        );
    }

    /// The wireframe export is the right way up too.
    #[test]
    fn wireframe_png_is_the_right_way_up() {
        let w = wire();
        let svg = wireframe_svg(&w, Some([0.0, 0.0, 0.0, 1.0]), false).unwrap();
        let png = tiny_skia::Pixmap::decode_png(&png(&svg, 2.0).unwrap()).unwrap();
        // Bounds grow by the wireframe line width (2) instead of 1: world y → row (y − 6) × 2.
        let y0 = w
            .shapes
            .iter()
            .map(|s| s.bounds.y0)
            .fold(f64::MAX, f64::min)
            - 3.0;
        let row = |y: f64| ((y - y0) * 2.0) as u32;
        assert_top_stays_top(
            |x, y| {
                let p = png.pixel(x, y).unwrap();
                [p.red(), p.green(), p.blue(), p.alpha()]
            },
            row(10.0),
            row(40.0),
            row(60.0),
            png.width() / 2,
        );
    }

    /// Labels are written as text and rendered in the bundled face (no system fonts).
    #[test]
    fn wireframe_labels_are_written_and_rendered() {
        let render = |label: &str| {
            let mut w = wire();
            let boxed = w
                .shapes
                .iter_mut()
                .find(|s| s.bounds.height() > 20.0)
                .unwrap();
            boxed.shape_type = nc_core::ShapeType::Card;
            boxed.label = label.into();
            let svg = wireframe_svg(&w, Some([0.0, 0.0, 0.0, 1.0]), false).unwrap();
            let png = tiny_skia::Pixmap::decode_png(&png(&svg, 2.0).unwrap()).unwrap();
            let lit = png.pixels().iter().filter(|p| p.green() > 100).count();
            (svg, lit)
        };
        let (svg, with) = render("Sign <in>");
        assert!(svg.contains(">Sign &lt;in&gt;</text>"), "{svg}");
        let (_, without) = render("");
        assert!(
            with > without + 50,
            "label glyphs drawn: {with} vs {without} lit px"
        );
        assert!(pdf(&svg).unwrap().starts_with(b"%PDF-"));
    }

    /// A sketch far larger than the cap still exports, at the scale that fits.
    #[test]
    fn huge_sketches_render_within_the_raster_cap() {
        let mut s = Stroke::new(Vec2::new(0.0, 0.0), 4.0, [1.0; 4]);
        s.push(Vec2::new(12_000.0, 3_000.0));
        let svg = svg(
            &[Arc::new(Element::stroke(s))],
            Some([0.0, 0.0, 0.0, 1.0]),
            false,
        )
        .unwrap();
        let png = tiny_skia::Pixmap::decode_png(&png(&svg, 2.0).unwrap()).unwrap();
        assert!(
            png.width() <= MAX_RASTER_PX as u32 && png.width() > 8000,
            "{}",
            png.width()
        );
    }
}
