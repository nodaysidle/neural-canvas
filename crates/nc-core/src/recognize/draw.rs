//! How a wireframe looks, as marks shared by the canvas overlay and wireframe export, so the two can't
//! disagree: the canvas tessellates marks into capsules, the SVG writer emits them as native elements.

use super::{Recognized, Wireframe};
use crate::document::{Segment, stroke_segments};
use crate::element::{ShapeType, Stroke};
use crate::style::Style;
use crate::theme::hex;
use glam::Vec2;
use kurbo::{Ellipse, PathEl, Rect, RoundedRect, Shape};

/// Label text size, world units.
pub const LABEL_SIZE: f32 = 14.0;

/// Where an element's label goes: the point and whether it is centred on it (else it starts there,
/// vertically centred). `None` without a label.
pub fn label_anchor(s: &Recognized) -> Option<(Vec2, bool)> {
    if s.label.is_empty() {
        return None;
    }
    let r = s.bounds;
    let mid_y = r.center().y;
    Some(match s.shape_type {
        ShapeType::Button
        | ShapeType::Icon
        | ShapeType::Circle
        | ShapeType::Ellipse
        | ShapeType::Image => (v(r.center().x, mid_y), true),
        ShapeType::InputField => (v(r.x0 + r.width() * 0.06, mid_y), false),
        ShapeType::Text | ShapeType::Line | ShapeType::Divider | ShapeType::Arrow => {
            (v(r.x0, mid_y), false)
        }
        // Containers name their content: top-left, inside.
        _ => (v(r.x0 + 12.0, r.y0 + 12.0 + LABEL_SIZE as f64 / 2.0), false),
    })
}

/// Wireframe line width, world units.
pub const LINE_WIDTH: f32 = 2.0;
/// Linear RGB: the wireframe, and elements the quality gate flags, on the dark canvas.
pub const WIRE: [f32; 4] = [0.45, 0.68, 1.0, 1.0];
pub const UNCERTAIN: [f32; 4] = [1.0, 0.6, 0.15, 1.0];
/// The same on the light canvas: the accent blue (#1f5fd1) and a deep amber (#9a5a00). Not the ink
/// inversion: the wireframe must stay distinct from the ink it sits on.
pub const WIRE_LIGHT: [f32; 4] = [0.0137, 0.1145, 0.637, 1.0];
pub const UNCERTAIN_LIGHT: [f32; 4] = [0.323, 0.102, 0.0, 1.0];

/// The colour an element's wireframe is drawn in.
pub fn color(light: bool, uncertain: bool) -> [f32; 4] {
    match (light, uncertain) {
        (false, false) => WIRE,
        (false, true) => UNCERTAIN,
        (true, false) => WIRE_LIGHT,
        (true, true) => UNCERTAIN_LIGHT,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    /// An outline, with rounded corners when `radius > 0`.
    Rect {
        rect: Rect,
        radius: f32,
    },
    Ellipse {
        rect: Rect,
    },
    Line {
        a: Vec2,
        b: Vec2,
    },
    /// A solid placeholder bar (text, a label), as thick as `rect` is tall, round-ended.
    Bar {
        rect: Rect,
    },
}

fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x as f32, y as f32)
}

/// A bar `w` × `h` (fractions of `r`'s size) whose left edge is at fraction `left` of `r`, centred
/// vertically.
fn bar(r: Rect, left: f64, w: f64, h: f64) -> Mark {
    let (cy, bh) = (r.center().y, (r.height() * h).min(10.0));
    let x0 = r.x0 + r.width() * left;
    Mark::Bar {
        rect: Rect::new(x0, cy - bh / 2.0, x0 + r.width() * w, cy + bh / 2.0),
    }
}

/// The marks one element draws as, in `out`.
pub fn marks(s: &Recognized, out: &mut Vec<Mark>) {
    let r = s.bounds;
    let radius = s.corner_radius.unwrap_or(0.0);
    let ends = s.ends.unwrap_or([v(r.x0, r.y0), v(r.x1, r.y1)]);
    match s.shape_type {
        ShapeType::Rectangle
        | ShapeType::RoundedRectangle
        | ShapeType::Card
        | ShapeType::Container
        | ShapeType::Unknown => out.push(Mark::Rect { rect: r, radius }),
        // A label replaces the placeholder bar.
        ShapeType::Button => {
            out.push(Mark::Rect { rect: r, radius });
            if s.label.is_empty() {
                out.push(bar(r, 0.3, 0.4, 0.25));
            }
        }
        ShapeType::InputField => {
            out.push(Mark::Rect { rect: r, radius });
            if s.label.is_empty() {
                out.push(bar(r, 0.06, 0.3, 0.2));
            }
        }
        ShapeType::Image => {
            out.push(Mark::Rect {
                rect: r,
                radius: 0.0,
            });
            out.push(Mark::Line {
                a: v(r.x0, r.y0),
                b: v(r.x1, r.y1),
            });
            out.push(Mark::Line {
                a: v(r.x1, r.y0),
                b: v(r.x0, r.y1),
            });
        }
        ShapeType::Circle | ShapeType::Ellipse | ShapeType::Icon => {
            out.push(Mark::Ellipse { rect: r })
        }
        ShapeType::Line | ShapeType::Divider => out.push(Mark::Line {
            a: ends[0],
            b: ends[1],
        }),
        ShapeType::Arrow => {
            let [tail, tip] = ends;
            out.push(Mark::Line { a: tail, b: tip });
            let back =
                (tail - tip).normalize_or_zero() * (0.15 * tail.distance(tip)).clamp(8.0, 24.0);
            for turn in [0.5f32, -0.5] {
                out.push(Mark::Line {
                    a: tip,
                    b: tip + Vec2::from_angle(turn).rotate(back),
                });
            }
        }
        ShapeType::Text if s.label.is_empty() => out.push(bar(r, 0.0, 1.0, 0.6)),
        ShapeType::Text => {}
    }
}

/// Flattening tolerance for curves in world units.
const TOLERANCE: f64 = 0.1;

fn outline(path: impl Shape, color: [f32; 4], width: f32, out: &mut Vec<Segment>) {
    let mut points = Vec::new();
    kurbo::flatten(path.path_elements(TOLERANCE), TOLERANCE, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(v(p.x, p.y)),
        PathEl::ClosePath => {
            if let Some(&first) = points.first() {
                points.push(first);
            }
        }
        _ => {}
    });
    if let Some((first, rest)) = points.split_first() {
        let mut stroke = Stroke::new(*first, width, color);
        rest.iter().for_each(|p| {
            stroke.push(*p);
        });
        out.extend(stroke_segments(&stroke));
    }
}

/// Capsules for one mark's outline, `width` world units wide, as the canvas draws it.
pub fn tessellate(mark: &Mark, color: [f32; 4], width: f32, out: &mut Vec<Segment>) {
    let hw = width / 2.0;
    match *mark {
        Mark::Rect { rect, radius } => outline(
            RoundedRect::from_rect(rect, radius as f64),
            color,
            width,
            out,
        ),
        Mark::Ellipse { rect } => outline(Ellipse::from_rect(rect), color, width, out),
        Mark::Line { a, b } => out.push(Segment {
            a,
            b,
            half_width: hw,
            color,
        }),
        Mark::Bar { rect } => {
            let r = rect.height() / 2.0;
            let cy = rect.center().y;
            out.push(Segment {
                a: v(rect.x0 + r, cy),
                b: v((rect.x1 - r).max(rect.x0 + r), cy),
                half_width: r as f32,
                color: [color[0], color[1], color[2], color[3] * 0.55],
            });
        }
    }
}

/// How the wireframe is coloured: in the theme's wireframe colour, or in a style taken from a
/// screenshot, as a filled mockup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    Plain { light: bool },
    Styled(Style),
}

/// One mark and how it is painted. A `Bar` is always solid, in its `fill` colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Painted {
    pub mark: Mark,
    pub fill: Option<[f32; 4]>,
    pub stroke: Option<[f32; 4]>,
    /// Stroke width, world units.
    pub width: f32,
}

/// Outline width of a styled element, world units: a border, not a sketch line.
pub const STYLED_LINE_WIDTH: f32 = 1.0;

fn alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

/// The whole wireframe as painted marks, bottom to top. Styled, it sits on a backdrop in the style's
/// background, larger elements first so a card never covers what is inside it; elements the quality
/// gate flags keep their warning outline either way.
pub fn paint(wire: &Wireframe, look: &Look, out: &mut Vec<Painted>) {
    let mut marks = Vec::new();
    let style = match look {
        Look::Plain { light } => {
            for s in &wire.shapes {
                marks.clear();
                self::marks(s, &mut marks);
                let c = color(*light, s.is_uncertain());
                out.extend(marks.iter().map(|&mark| Painted {
                    mark,
                    fill: matches!(mark, Mark::Bar { .. }).then_some(c),
                    stroke: (!matches!(mark, Mark::Bar { .. })).then_some(c),
                    width: LINE_WIDTH,
                }));
            }
            return;
        }
        Look::Styled(style) => style,
    };
    let [bg, surface, accent, text, on_accent] = [
        style.background,
        style.surface,
        style.accent,
        style.text,
        style.on_accent(),
    ]
    .map(hex);
    // The quality gate's amber, in the shade that reads on the style's background.
    let light_background =
        crate::theme::contrast(bg, [0.0, 0.0, 0.0, 1.0]) > crate::theme::contrast(bg, [1.0; 4]);
    let warning = color(light_background, true);
    if let Some(all) = wire
        .shapes
        .iter()
        .map(|s| s.bounds)
        .reduce(|a, b| a.union(b))
    {
        let pad = 2.0 * style.spacing as f64;
        out.push(Painted {
            mark: Mark::Rect {
                rect: all.inflate(pad, pad),
                radius: 0.0,
            },
            fill: Some(bg),
            stroke: None,
            width: 0.0,
        });
    }
    let mut order: Vec<&Recognized> = wire.shapes.iter().collect();
    order.sort_by(|a, b| b.bounds.area().total_cmp(&a.bounds.area()));
    for s in order {
        use ShapeType as T;
        // The element's body, its outline, its placeholder bar, its lines.
        let border = alpha(text, 0.15);
        let (body, outline, bar, line) = match s.shape_type {
            T::Button | T::Icon => (Some(accent), None, on_accent, text),
            T::InputField => (Some(bg), Some(alpha(text, 0.35)), text, text),
            T::Image => (Some(surface), None, text, alpha(text, 0.3)),
            T::Divider => (None, None, text, alpha(text, 0.2)),
            T::Line | T::Arrow | T::Text => (None, None, text, text),
            T::Rectangle
            | T::RoundedRectangle
            | T::Card
            | T::Container
            | T::Circle
            | T::Ellipse
            | T::Unknown => (Some(surface), Some(border), text, text),
        };
        let uncertain = s.is_uncertain();
        let rounded = !matches!(s.shape_type, T::Rectangle);
        marks.clear();
        self::marks(s, &mut marks);
        out.extend(marks.iter().map(|&mark| match mark {
            Mark::Rect { rect, radius } => Painted {
                mark: Mark::Rect {
                    rect,
                    radius: if rounded { style.radius } else { radius },
                },
                fill: body,
                stroke: if uncertain { Some(warning) } else { outline },
                width: STYLED_LINE_WIDTH,
            },
            Mark::Ellipse { .. } => Painted {
                mark,
                fill: body,
                stroke: if uncertain { Some(warning) } else { outline },
                width: STYLED_LINE_WIDTH,
            },
            Mark::Bar { .. } => Painted {
                mark,
                fill: Some(bar),
                stroke: None,
                width: 0.0,
            },
            Mark::Line { .. } => Painted {
                mark,
                fill: None,
                stroke: Some(if uncertain { warning } else { line }),
                width: if uncertain {
                    LINE_WIDTH
                } else {
                    STYLED_LINE_WIDTH.max(1.5)
                },
            },
        }));
    }
}

/// The colour of an element's label.
pub fn label_color(s: &Recognized, look: &Look) -> [f32; 4] {
    match look {
        Look::Plain { light } => color(*light, s.is_uncertain()),
        Look::Styled(style) => match s.shape_type {
            ShapeType::Button | ShapeType::Icon => hex(style.on_accent()),
            _ => hex(style.text),
        },
    }
}

/// Capsules for a painted mark's stroke and solid bar; filled boxes and ellipses are the renderer's
/// (it draws them as shapes, not capsules).
pub fn painted_segments(p: &Painted, out: &mut Vec<Segment>) {
    match (p.mark, p.fill, p.stroke) {
        (Mark::Bar { .. }, Some(fill), _) => tessellate(&p.mark, fill, LINE_WIDTH, out),
        (Mark::Bar { .. }, None, _) => {}
        (_, _, Some(stroke)) => tessellate(&p.mark, stroke, p.width, out),
        (_, _, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognize::Recognized;

    fn shape(t: ShapeType, r: Rect) -> Recognized {
        Recognized::new(t, r, 0.9, Vec::new())
    }

    #[test]
    fn ui_elements_draw_their_conventions() {
        let r = Rect::new(0.0, 0.0, 120.0, 40.0);
        let count = |t| {
            let mut out = Vec::new();
            marks(&shape(t, r), &mut out);
            out
        };
        assert_eq!(count(ShapeType::Button).len(), 2, "outline + centred label");
        let Mark::Bar { rect } = count(ShapeType::Button)[1] else {
            panic!()
        };
        assert!((rect.center().x - 60.0).abs() < 1e-6);
        let Mark::Bar { rect } = count(ShapeType::InputField)[1] else {
            panic!()
        };
        assert!(rect.center().x < 40.0, "placeholder at the left");
        assert_eq!(count(ShapeType::Image).len(), 3, "box + X");
        assert_eq!(count(ShapeType::Arrow).len(), 3, "shaft + two barbs");
    }

    #[test]
    fn arrows_point_at_their_tip() {
        let mut s = shape(ShapeType::Arrow, Rect::new(0.0, 0.0, 100.0, 0.0));
        s.ends = Some([Vec2::ZERO, Vec2::new(100.0, 0.0)]);
        let mut out = Vec::new();
        marks(&s, &mut out);
        for m in &out[1..] {
            let Mark::Line { a, b } = m else { panic!() };
            assert_eq!(*a, Vec2::new(100.0, 0.0));
            assert!(b.x < 100.0, "barbs go back along the shaft");
        }
    }

    #[test]
    fn uncertain_elements_are_drawn_in_their_own_colour() {
        let wire = Wireframe {
            shapes: vec![
                shape(ShapeType::Card, Rect::new(0.0, 0.0, 50.0, 50.0)),
                Recognized::new(
                    ShapeType::Unknown,
                    Rect::new(60.0, 0.0, 90.0, 30.0),
                    0.3,
                    vec![],
                ),
            ],
            ..Wireframe::default()
        };
        let mut painted = Vec::new();
        paint(&wire, &Look::Plain { light: false }, &mut painted);
        let mut out = Vec::new();
        painted.iter().for_each(|p| painted_segments(p, &mut out));
        assert!(out.iter().any(|s| s.color == WIRE));
        assert!(out.iter().any(|s| s.color == UNCERTAIN));
        assert!(out.iter().all(|s| s.a.is_finite() && s.b.is_finite()));
    }

    #[test]
    fn styled_wireframe_paints_a_filled_mockup_bottom_to_top() {
        let style = Style {
            background: 0xffffff,
            surface: 0xf4f5f7,
            accent: 0x1f5fd1,
            text: 0x16181e,
            radius: 12.0,
            spacing: 8.0,
            line_height: None,
        };
        let button = shape(ShapeType::Button, Rect::new(20.0, 120.0, 140.0, 160.0));
        let card = shape(ShapeType::Card, Rect::new(0.0, 0.0, 300.0, 200.0));
        let shaky = Recognized::new(
            ShapeType::Unknown,
            Rect::new(160.0, 20.0, 200.0, 60.0),
            0.3,
            vec![],
        );
        // The button comes first in recognition order; the card must still be painted under it.
        let wire = Wireframe {
            shapes: vec![button.clone(), card, shaky],
            ..Wireframe::default()
        };
        let mut out = Vec::new();
        paint(&wire, &Look::Styled(style), &mut out);
        let Mark::Rect { rect, .. } = out[0].mark else {
            panic!()
        };
        assert_eq!(
            rect,
            Rect::new(-16.0, -16.0, 316.0, 216.0),
            "backdrop, 2 × spacing around"
        );
        assert_eq!(out[0].fill, Some(hex(0xffffff)));
        let fills: Vec<_> = out.iter().filter_map(|p| p.fill).collect();
        assert_eq!(fills[1], hex(0xf4f5f7), "the card, under");
        let accent = out
            .iter()
            .position(|p| p.fill == Some(hex(0x1f5fd1)))
            .unwrap();
        let card = out
            .iter()
            .position(|p| p.fill == Some(hex(0xf4f5f7)))
            .unwrap();
        assert!(card < accent, "the button over the card");
        let Mark::Rect { radius, .. } = out[accent].mark else {
            panic!()
        };
        assert_eq!(radius, 12.0, "the style's corners");
        assert!(
            out.iter().any(|p| p.stroke == Some(color(true, true))),
            "uncertain elements keep their warning outline"
        );
        assert_eq!(label_color(&button, &Look::Styled(style)), hex(0xffffff));
    }
}
