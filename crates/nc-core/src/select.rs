//! The select tool: click, marquee and lasso selection, moving and resizing through handles, and the
//! edits that act on a selection (delete, duplicate, copy, paste).

use crate::command::Command;
use crate::document::Document;
use crate::element::{Element, ElementId};
use crate::hit::{in_lasso, in_rect, pick};
use glam::Vec2;
use kurbo::{Affine, Point, Rect};
use std::sync::Arc;
use uuid::Uuid;

/// Handle radius in logical screen px: handles are 8 px across at every zoom.
pub const HANDLE_RADIUS: f32 = 4.0;

/// MIME type of the internal clipboard format.
pub const CLIPBOARD_MIME: &str = "application/x-neuralcanvas";
const CLIPBOARD_MAGIC: &[u8] = b"NCv1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::NorthWest,
        Self::North,
        Self::NorthEast,
        Self::East,
        Self::SouthEast,
        Self::South,
        Self::SouthWest,
        Self::West,
    ];

    /// Which edges this handle moves, as (x, y): -1 the min edge, 1 the max edge, 0 neither.
    fn sides(self) -> (i8, i8) {
        match self {
            Self::NorthWest => (-1, -1),
            Self::North => (0, -1),
            Self::NorthEast => (1, -1),
            Self::East => (1, 0),
            Self::SouthEast => (1, 1),
            Self::South => (0, 1),
            Self::SouthWest => (-1, 1),
            Self::West => (-1, 0),
        }
    }

    pub fn position(self, b: Rect) -> Vec2 {
        let (sx, sy) = self.sides();
        let pick = |s, lo: f64, hi: f64| match s {
            -1 => lo,
            1 => hi,
            _ => (lo + hi) * 0.5,
        };
        Vec2::new(pick(sx, b.x0, b.x1) as f32, pick(sy, b.y0, b.y1) as f32)
    }
}

/// The handle of `bounds` (screen space) under `p`, if any. Corners win over edges where they overlap.
pub fn handle_at(bounds: Rect, p: Vec2) -> Option<Handle> {
    let reach = HANDLE_RADIUS + 2.0;
    Handle::ALL
        .into_iter()
        .filter(|h| h.position(bounds).distance(p) <= reach)
        .min_by_key(|h| {
            let (sx, sy) = h.sides();
            (sx == 0 || sy == 0) as u8
        })
}

/// `bounds` with the handle's edges moved by `delta`. Dragging past the opposite edge stops at 1 unit
/// rather than flipping the selection.
pub fn resized(bounds: Rect, handle: Handle, delta: Vec2) -> Rect {
    let (sx, sy) = handle.sides();
    let (dx, dy) = (delta.x as f64, delta.y as f64);
    let mut r = bounds;
    match sx {
        -1 => r.x0 = (r.x0 + dx).min(r.x1 - 1.0),
        1 => r.x1 = (r.x1 + dx).max(r.x0 + 1.0),
        _ => {}
    }
    match sy {
        -1 => r.y0 = (r.y0 + dy).min(r.y1 - 1.0),
        1 => r.y1 = (r.y1 + dy).max(r.y0 + 1.0),
        _ => {}
    }
    r
}

/// The map taking rectangle `from` onto `to`.
pub fn fit_affine(from: Rect, to: Rect) -> Affine {
    let scale = |a: f64, b: f64| if a > 1e-9 { b / a } else { 1.0 };
    Affine::translate((to.x0, to.y0))
        * Affine::scale_non_uniform(
            scale(from.width(), to.width()),
            scale(from.height(), to.height()),
        )
        * Affine::translate((-from.x0, -from.y0))
}

/// Union of the selected elements' bounds.
pub fn bounds(doc: &Document, ids: &[ElementId]) -> Option<Rect> {
    ids.iter()
        .filter_map(|id| doc.get(*id)?.bounds())
        .reduce(|a, b| a.union(b))
}

fn snapshot(doc: &Document, ids: &[ElementId]) -> Vec<Arc<Element>> {
    ids.iter().filter_map(|id| doc.get(*id).cloned()).collect()
}

/// Replaces each element with its original mapped through `t`. Taking the originals every time (rather
/// than the current state) keeps a long drag free of rounding drift.
pub fn transformed(originals: &[Arc<Element>], t: Affine) -> Command {
    Command::Batch(
        originals
            .iter()
            .map(|e| Command::Replace {
                element: Arc::new(e.transformed(t)),
            })
            .collect(),
    )
}

fn translate(delta: Vec2) -> Affine {
    Affine::translate((delta.x as f64, delta.y as f64))
}

pub fn translated(doc: &Document, ids: &[ElementId], delta: Vec2) -> Command {
    transformed(&snapshot(doc, ids), translate(delta))
}

pub fn delete(ids: &[ElementId]) -> Command {
    Command::Batch(ids.iter().map(|&id| Command::Remove { id }).collect())
}

/// Copies of `elements` with fresh ids, moved by `delta`, appended on top in their current order.
fn appended(elements: impl IntoIterator<Item = Element>, delta: Vec2) -> (Command, Vec<ElementId>) {
    let t = translate(delta);
    let copies: Vec<Element> = elements
        .into_iter()
        .map(|e| Element {
            id: Uuid::new_v4(),
            ..e.transformed(t)
        })
        .collect();
    let ids = copies.iter().map(|e| e.id).collect();
    let command = Command::Batch(copies.into_iter().map(Command::append).collect());
    (command, ids)
}

/// Copies of the selection (in z-order) offset by `offset`; returns the copies' ids to select.
pub fn duplicate(doc: &Document, ids: &[ElementId], offset: Vec2) -> (Command, Vec<ElementId>) {
    appended(selected(doc, ids).iter().map(|e| (**e).clone()), offset)
}

/// Pastes `elements` centred on `centre` with fresh ids; returns the new ids to select.
pub fn paste(elements: Vec<Element>, centre: Vec2) -> (Command, Vec<ElementId>) {
    let delta = elements
        .iter()
        .filter_map(Element::bounds)
        .reduce(|a, b| a.union(b))
        .map_or(Vec2::ZERO, |b| {
            let c = b.center();
            centre - Vec2::new(c.x as f32, c.y as f32)
        });
    appended(elements, delta)
}

/// The selected elements in z-order (what copy and duplicate take).
pub fn selected(doc: &Document, ids: &[ElementId]) -> Vec<Arc<Element>> {
    let mut elements = snapshot(doc, ids);
    elements.sort_by_key(|e| doc.index_of(e.id));
    elements
}

/// The internal clipboard format: a magic tag and the elements.
pub fn encode(elements: &[Arc<Element>]) -> Vec<u8> {
    let mut out = CLIPBOARD_MAGIC.to_vec();
    out.extend(postcard::to_stdvec(elements).expect("elements always serialize"));
    out
}

/// `None` when the bytes are not our format (or from an incompatible version).
pub fn decode(bytes: &[u8]) -> Option<Vec<Element>> {
    postcard::from_bytes(bytes.strip_prefix(CLIPBOARD_MAGIC)?).ok()
}

/// A select-tool drag in progress. Positions are world units.
#[derive(Clone, Debug)]
pub enum Gesture {
    Marquee {
        start: Vec2,
        current: Vec2,
    },
    Lasso(Vec<Vec2>),
    Move {
        start: Vec2,
        current: Vec2,
        originals: Vec<Arc<Element>>,
    },
    Resize {
        handle: Handle,
        start: Vec2,
        current: Vec2,
        bounds: Rect,
        originals: Vec<Arc<Element>>,
    },
}

/// A select-tool press at `world`. `handle` is the selection handle under the pointer (from
/// [`handle_at`]); `tolerance` the pick distance in world units. Shift adds to the selection (a Shift
/// click toggles one element); Alt drags a lasso instead of a marquee. Updates `selection` and returns
/// the drag that follows, if any.
pub fn begin(
    doc: &Document,
    selection: &mut Vec<ElementId>,
    world: Vec2,
    handle: Option<Handle>,
    shift: bool,
    alt: bool,
    tolerance: f32,
) -> Option<Gesture> {
    if let (Some(handle), Some(b)) = (handle, bounds(doc, selection)) {
        return Some(Gesture::Resize {
            handle,
            start: world,
            current: world,
            bounds: b,
            originals: snapshot(doc, selection),
        });
    }
    let inside = bounds(doc, selection)
        .is_some_and(|b| b.contains(Point::new(world.x as f64, world.y as f64)));
    match pick(doc, world, tolerance) {
        Some(id) if shift => {
            match selection.iter().position(|s| *s == id) {
                Some(i) => {
                    selection.remove(i);
                }
                None => selection.push(id),
            }
            None
        }
        Some(id) => {
            if !selection.contains(&id) {
                *selection = vec![id];
            }
            Some(Gesture::Move {
                start: world,
                current: world,
                originals: snapshot(doc, selection),
            })
        }
        None if inside && !shift => Some(Gesture::Move {
            start: world,
            current: world,
            originals: snapshot(doc, selection),
        }),
        None => {
            if !shift {
                selection.clear();
            }
            Some(if alt {
                Gesture::Lasso(vec![world])
            } else {
                Gesture::Marquee {
                    start: world,
                    current: world,
                }
            })
        }
    }
}

impl Gesture {
    /// The pointer moved to `world`.
    pub fn update(&mut self, world: Vec2) {
        match self {
            Self::Marquee { current, .. }
            | Self::Move { current, .. }
            | Self::Resize { current, .. } => *current = world,
            Self::Lasso(points) => {
                if points.last() != Some(&world) {
                    points.push(world);
                }
            }
        }
    }

    /// For a move or resize, the map from where the selection was at the press to where it is shown
    /// now. The document is edited only by [`finish`](Self::finish), so a drag costs the same per frame
    /// however much is selected: the renderer applies this map to the selection on the GPU.
    pub fn transform(&self) -> Option<Affine> {
        match self {
            Self::Move { start, current, .. } => Some(translate(*current - *start)),
            Self::Resize {
                handle,
                start,
                current,
                bounds,
                ..
            } => Some(fit_affine(
                *bounds,
                resized(*bounds, *handle, *current - *start),
            )),
            Self::Marquee { .. } | Self::Lasso(_) => None,
        }
    }

    /// The elements a move or resize applies to.
    pub fn moved(&self) -> &[Arc<Element>] {
        match self {
            Self::Move { originals, .. } | Self::Resize { originals, .. } => originals,
            Self::Marquee { .. } | Self::Lasso(_) => &[],
        }
    }

    /// The drag ended. A move or resize returns its edit (`None` if nothing moved); marquee and lasso
    /// select what they enclose (added to the selection with Shift).
    pub fn finish(
        self,
        doc: &Document,
        selection: &mut Vec<ElementId>,
        shift: bool,
    ) -> Option<Command> {
        let found = match &self {
            Self::Marquee { start, current } => in_rect(
                doc,
                Rect::from_points(
                    (start.x as f64, start.y as f64),
                    (current.x as f64, current.y as f64),
                ),
            ),
            Self::Lasso(points) => in_lasso(doc, points),
            Self::Move { .. } | Self::Resize { .. } => {
                let t = self.transform().expect("moves have a transform");
                return (t != Affine::IDENTITY).then(|| transformed(self.moved(), t));
            }
        };
        if !shift {
            selection.clear();
        }
        for id in found {
            if !selection.contains(&id) {
                selection.push(id);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{ElementKind, Stroke};

    fn line(a: Vec2, b: Vec2) -> Element {
        let mut s = Stroke::new(a, 2.0, [1.0; 4]);
        s.push(b);
        Element::stroke(s)
    }

    fn points(doc: &Document, id: ElementId) -> Vec<Vec2> {
        match &doc.get(id).unwrap().kind {
            ElementKind::Stroke(s) => s.points.clone(),
            _ => panic!(),
        }
    }

    /// Two strokes: `a` from (0,0) to (10,10), `b` from (50,0) to (60,10).
    fn two() -> (Document, ElementId, ElementId) {
        let a = line(Vec2::ZERO, Vec2::splat(10.0));
        let b = line(Vec2::new(50.0, 0.0), Vec2::new(60.0, 10.0));
        let (ida, idb) = (a.id, b.id);
        (Document::from_elements([a, b].map(Arc::new)), ida, idb)
    }

    fn press(doc: &Document, sel: &mut Vec<ElementId>, at: Vec2, shift: bool) -> Option<Gesture> {
        begin(doc, sel, at, None, shift, false, 3.0)
    }

    #[test]
    fn click_selects_shift_click_toggles_empty_click_clears() {
        let (doc, a, b) = two();
        let mut sel = Vec::new();
        assert!(matches!(
            press(&doc, &mut sel, Vec2::splat(5.0), false),
            Some(Gesture::Move { .. })
        ));
        assert_eq!(sel, [a]);
        assert!(press(&doc, &mut sel, Vec2::new(55.0, 5.0), true).is_none());
        assert_eq!(sel, [a, b]);
        press(&doc, &mut sel, Vec2::splat(5.0), true);
        assert_eq!(sel, [b]);
        let g = press(&doc, &mut sel, Vec2::new(30.0, 40.0), false).unwrap();
        assert!(sel.is_empty());
        g.finish(&doc, &mut sel, false);
        assert!(sel.is_empty(), "a click on empty canvas selects nothing");
    }

    #[test]
    fn marquee_and_lasso_select_on_release() {
        let (doc, a, b) = two();
        let mut sel = Vec::new();
        let mut g = press(&doc, &mut sel, Vec2::new(-5.0, -5.0), false).unwrap();
        g.update(Vec2::new(20.0, 20.0));
        g.finish(&doc, &mut sel, false);
        assert_eq!(sel, [a]);

        let mut g = begin(
            &doc,
            &mut sel,
            Vec2::new(40.0, -10.0),
            None,
            true,
            true,
            3.0,
        )
        .unwrap();
        for p in [(70.0, -10.0), (70.0, 20.0), (40.0, 20.0)] {
            g.update(Vec2::new(p.0, p.1));
        }
        g.finish(&doc, &mut sel, true);
        assert_eq!(sel, [a, b], "Shift + Alt lasso adds");
    }

    #[test]
    fn moving_is_relative_to_the_press_and_one_undo_step() {
        let (doc, a, b) = two();
        let mut session = crate::Session::new(Uuid::new_v4(), doc);
        let original = session.doc().elements().to_vec();
        let mut sel = vec![a, b];
        let mut g = press(session.doc(), &mut sel, Vec2::splat(5.0), false).unwrap();
        for x in 1..=30 {
            g.update(Vec2::new(5.0 + x as f32, 5.0));
        }
        assert_eq!(g.transform(), Some(Affine::translate((30.0, 0.0))));
        assert_eq!(
            session.doc().elements(),
            original.as_slice(),
            "untouched until release"
        );
        let edit = g.finish(session.doc(), &mut sel, false).unwrap();
        session.execute("Move", edit).unwrap();
        assert_eq!(
            points(session.doc(), a),
            [Vec2::new(30.0, 0.0), Vec2::new(40.0, 10.0)]
        );
        assert_eq!(points(session.doc(), b)[0], Vec2::new(80.0, 0.0));
        assert_eq!(session.history().undo_label(), Some("Move"));
        session.undo().unwrap();
        assert_eq!(session.doc().elements(), original.as_slice());
    }

    #[test]
    fn dragging_inside_the_box_moves_even_off_ink() {
        let (doc, a, b) = two();
        let mut sel = vec![a, b];
        // (30, 5) is between the strokes, on no ink, inside the selection box.
        let g = press(&doc, &mut sel, Vec2::new(30.0, 5.0), false);
        assert!(matches!(g, Some(Gesture::Move { .. })));
        assert_eq!(sel, [a, b]);
    }

    #[test]
    fn handles_are_found_and_resize_maps_the_box() {
        let b = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(
            handle_at(b, Vec2::new(101.0, 51.0)),
            Some(Handle::SouthEast)
        );
        assert_eq!(handle_at(b, Vec2::new(50.0, -3.0)), Some(Handle::North));
        assert_eq!(handle_at(b, Vec2::new(50.0, 25.0)), None);
        assert_eq!(
            resized(b, Handle::SouthEast, Vec2::new(100.0, 50.0)),
            Rect::new(0.0, 0.0, 200.0, 100.0)
        );
        assert_eq!(
            resized(b, Handle::West, Vec2::new(500.0, 9.0)),
            Rect::new(99.0, 0.0, 100.0, 50.0),
            "never flips"
        );
        let t = fit_affine(b, Rect::new(10.0, 10.0, 210.0, 110.0));
        assert_eq!(t * Point::new(100.0, 50.0), Point::new(210.0, 110.0));
    }

    #[test]
    fn resize_gesture_scales_from_the_opposite_corner() {
        let (doc, a, _) = two();
        let mut sel = vec![a];
        let b = bounds(&doc, &sel).unwrap();
        assert_eq!(b, Rect::new(-1.0, -1.0, 11.0, 11.0));
        let mut g = begin(
            &doc,
            &mut sel,
            Vec2::splat(11.0),
            Some(Handle::SouthEast),
            false,
            false,
            3.0,
        )
        .unwrap();
        let mut doc = doc;
        g.update(Vec2::splat(23.0));
        g.finish(&doc, &mut sel, false)
            .unwrap()
            .apply(&mut doc)
            .unwrap();
        // Box 12 → 24 wide, anchored at (-1, -1): (0,0) → (1,1), (10,10) → (21,21).
        assert_eq!(points(&doc, a), [Vec2::splat(1.0), Vec2::splat(21.0)]);
    }

    #[test]
    fn delete_duplicate_and_nudge() {
        let (mut doc, a, b) = two();
        let (cmd, copies) = duplicate(&doc, &[b, a], Vec2::splat(16.0));
        cmd.apply(&mut doc).unwrap();
        assert_eq!(doc.elements().len(), 4);
        assert_eq!(
            points(&doc, copies[0])[0],
            Vec2::splat(16.0),
            "copies keep z-order: a first"
        );
        assert!(!copies.contains(&a) && !copies.contains(&b));
        translated(&doc, &copies, Vec2::new(10.0, 0.0))
            .apply(&mut doc)
            .unwrap();
        assert_eq!(points(&doc, copies[0])[0], Vec2::new(26.0, 16.0));
        delete(&copies).apply(&mut doc).unwrap();
        let ids: Vec<_> = doc.elements().iter().map(|e| e.id).collect();
        assert_eq!(ids, [a, b]);
    }

    #[test]
    fn clipboard_round_trip_and_paste_centres_on_the_pointer() {
        let (doc, a, b) = two();
        let bytes = encode(&selected(&doc, &[b, a]));
        let elements = decode(&bytes).unwrap();
        assert_eq!(elements[0].id, a, "z-order kept");
        assert!(decode(b"not ours").is_none());
        assert!(decode(b"NCv1\xff\xff").is_none());

        let mut doc = doc;
        let (cmd, ids) = paste(elements, Vec2::new(500.0, 500.0));
        cmd.apply(&mut doc).unwrap();
        assert_eq!(doc.elements().len(), 4);
        let c = bounds(&doc, &ids).unwrap().center();
        assert_eq!((c.x, c.y), (500.0, 500.0));
    }
}
