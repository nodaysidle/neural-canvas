//! The sketch document. It changes only through [`Command`]s, so every edit is undoable and the
//! store sees every edit through `revision` (Mac bug #1: eraser edits never saved).

use crate::element::{Element, ElementId, Stroke};
use glam::Vec2;
use kurbo::Rect;
use rstar::RTree;
use rstar::primitives::{GeomWithData, Rectangle};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

type Entry = GeomWithData<Rectangle<[f64; 2]>, ElementId>;

/// The index behind a lock, so a document can be shared with other threads as a snapshot. Queries come
/// from one thread at a time in practice, so the lock is never contended.
#[derive(Debug, Default)]
struct Spatial(Mutex<Index>);

impl Spatial {
    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        // A panic mid-update leaves at worst stale entries; queries stay usable.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clone for Spatial {
    fn clone(&self) -> Self {
        Self(Mutex::new(self.lock().clone()))
    }
}

/// An R-tree of element bounds plus the edits it has not seen yet. Updates wait for the next query and
/// collapse to one per element: committing a 500-stroke move costs one batch, and an eraser drag's
/// repeated cuts of the same stroke cost one update each query.
#[derive(Clone, Debug, Default)]
struct Index {
    tree: RTree<Entry>,
    /// Changed elements → the entry the tree still holds for each (`None`: not in the tree).
    stale: HashMap<ElementId, Option<Entry>>,
}

fn entry(e: &Element) -> Option<Entry> {
    let b = e.bounds()?;
    Some(Entry::new(
        Rectangle::from_corners([b.x0, b.y0], [b.x1, b.y1]),
        e.id,
    ))
}

/// Elements in z-order (last is on top). Elements are immutable behind `Arc`: an edit swaps the `Arc`,
/// so a snapshot for the store is a cheap clone and unchanged elements keep their pointer.
#[derive(Clone, Debug, Default)]
pub struct Document {
    elements: Vec<Arc<Element>>,
    revision: u64,
    /// id → z-index, so edits by id are O(1) (committing a move replaces hundreds of elements).
    index: HashMap<ElementId, usize>,
    /// Element bounds for hit tests and the eraser, brought up to date on the next query.
    index_tree: Spatial,
}

/// A capsule from `a` to `b`. A single-point stroke yields one zero-length segment (a dot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: Vec2,
    pub b: Vec2,
    pub half_width: f32,
    pub color: [f32; 4],
}

impl Document {
    pub fn from_elements(elements: impl IntoIterator<Item = Arc<Element>>) -> Self {
        let elements: Vec<_> = elements.into_iter().collect();
        let tree = RTree::bulk_load(elements.iter().filter_map(|e| entry(e)).collect());
        let mut doc = Self {
            elements,
            index_tree: Spatial(Mutex::new(Index {
                tree,
                stale: HashMap::new(),
            })),
            ..Self::default()
        };
        doc.reindex(0);
        doc
    }

    fn reindex(&mut self, from: usize) {
        for (i, e) in self.elements.iter().enumerate().skip(from) {
            self.index.insert(e.id, i);
        }
    }

    pub fn elements(&self) -> &[Arc<Element>] {
        &self.elements
    }

    /// Bumped by every applied command.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn index_of(&self, id: ElementId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// Ids of elements whose bounds overlap `area`, in no particular order.
    pub fn query(&self, area: Rect) -> Vec<ElementId> {
        let mut index = self.index_tree.lock();
        let Index { tree, stale } = &mut *index;
        for (id, held) in stale.drain() {
            if let Some(old) = held {
                tree.remove(&old);
            }
            if let Some(e) = self.get(id).and_then(|e| entry(e)) {
                tree.insert(e);
            }
        }
        let envelope = rstar::AABB::from_corners([area.x0, area.y0], [area.x1, area.y1]);
        tree.locate_in_envelope_intersecting(envelope)
            .map(|e| e.data)
            .collect()
    }

    /// Notes that `id` changed; `held` is what the tree holds for it if it has not changed before.
    fn stale(&mut self, id: ElementId, held: impl FnOnce() -> Option<Entry>) {
        self.index_tree
            .0
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .stale
            .entry(id)
            .or_insert_with(held);
    }

    pub fn get(&self, id: ElementId) -> Option<&Arc<Element>> {
        self.index_of(id).map(|i| &self.elements[i])
    }

    /// Union of all element bounds; `None` when there is nothing to fit (Mac bug #18).
    pub fn bounds(&self) -> Option<Rect> {
        self.elements
            .iter()
            .filter_map(|e| e.bounds())
            .reduce(|a, b| a.union(b))
    }

    /// Every stroke's capsules in z-order, without allocating.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.elements
            .iter()
            .filter_map(|e| match &e.kind {
                crate::ElementKind::Stroke(s) => Some(s),
                _ => None,
            })
            .flat_map(stroke_segments)
    }

    pub(crate) fn insert(&mut self, index: usize, element: Arc<Element>) {
        self.stale(element.id, || None);
        self.elements.insert(index, element);
        self.reindex(index);
        self.revision += 1;
    }

    pub(crate) fn remove(&mut self, index: usize) -> Arc<Element> {
        self.revision += 1;
        let old = self.elements.remove(index);
        self.stale(old.id, || entry(&old));
        self.index.remove(&old.id);
        self.reindex(index);
        old
    }

    pub(crate) fn replace(&mut self, index: usize, element: Arc<Element>) -> Arc<Element> {
        self.revision += 1;
        let old = std::mem::replace(&mut self.elements[index], element);
        self.stale(old.id, || entry(&old));
        old
    }
}

/// Calls `f` with every capsule the element draws, in order. Shapes are traced, so this allocates for
/// them; strokes do not.
pub fn element_segments(e: &Element, mut f: impl FnMut(Segment)) {
    if let Some(ink) = e.ink() {
        stroke_segments(&ink).for_each(&mut f);
    }
}

pub fn stroke_segments(s: &Stroke) -> impl Iterator<Item = Segment> + '_ {
    stroke_segments_from(s, 0)
}

/// Capsules a stroke draws as: one per pair of points, or a single dot for a one-point stroke.
pub fn stroke_segment_count(s: &Stroke) -> usize {
    s.points.len().saturating_sub(1).max(s.points.len().min(1))
}

/// The stroke's capsules from index `first` on, in O(1) to reach `first` (the live stroke uploads only
/// its new tail each frame).
pub fn stroke_segments_from(s: &Stroke, first: usize) -> impl Iterator<Item = Segment> + '_ {
    let color = s.color;
    // A single point is a dot: one zero-length capsule. `windows(2)` yields nothing for it.
    let dot = (s.points.len() == 1 && first == 0).then(|| (0, s.points[0], s.points[0]));
    let pairs = s.points.get(first..).unwrap_or(&[]).windows(2);
    dot.into_iter()
        .chain(pairs.enumerate().map(move |(i, w)| (first + i, w[0], w[1])))
        .map(move |(i, a, b)| Segment {
            a,
            b,
            half_width: s.half_width(i),
            color,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_points_dropped_and_segments_counted() {
        let mut s = Stroke::new(Vec2::ZERO, 2.0, [1.0; 4]);
        assert!(!s.push(Vec2::ZERO));
        assert!(s.push(Vec2::X));
        assert!(s.push(Vec2::ONE));
        let doc = Document::from_elements([
            Arc::new(Element::stroke(s)),
            Arc::new(Element::stroke(Stroke::new(Vec2::Y, 2.0, [1.0; 4]))),
        ]);
        let segs: Vec<_> = doc.segments().collect();
        assert_eq!(segs.len(), 3); // 2 segments + 1 dot
        assert!(segs.iter().all(|s| s.a.is_finite() && s.b.is_finite()));
    }

    #[test]
    fn index_and_tree_follow_every_edit() {
        let mut doc = Document::default();
        let el = |x| Arc::new(Element::stroke(Stroke::new(Vec2::splat(x), 1.0, [1.0; 4])));
        let (a, b, c) = (el(1.0), el(2.0), el(3.0));
        doc.insert(0, a.clone());
        doc.insert(1, b.clone());
        doc.insert(0, c.clone());
        assert_eq!(doc.revision(), 3);
        assert_eq!(
            [a.id, b.id, c.id].map(|id| doc.index_of(id)),
            [Some(1), Some(2), Some(0)]
        );
        fn near(doc: &Document, x: f64) -> Vec<ElementId> {
            doc.query(Rect::new(x - 0.1, x - 0.1, x + 0.1, x + 0.1))
        }
        assert_eq!(near(&doc, 2.0), [b.id]);

        doc.remove(1);
        assert_eq!(doc.index_of(a.id), None);
        assert_eq!(doc.index_of(b.id), Some(1));
        assert!(near(&doc, 1.0).is_empty());

        let mut moved = (*b).clone();
        let crate::ElementKind::Stroke(s) = &mut moved.kind else {
            unreachable!()
        };
        s.points[0] = Vec2::splat(9.0);
        doc.replace(1, Arc::new(moved));
        assert!(near(&doc, 2.0).is_empty());
        assert_eq!(near(&doc, 9.0), [b.id]);
        assert_eq!(doc.index_tree.lock().tree.size(), 2);
    }
}
