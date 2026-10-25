//! The sketch document. It changes only through [`Command`]s, so every edit is undoable and the
//! store sees every edit through `revision` (Mac bug #1: eraser edits never saved).

use crate::element::{Element, ElementId, ElementKind, Stroke};
use glam::Vec2;
use kurbo::Rect;
use std::sync::Arc;

/// Elements in z-order (last is on top). Elements are immutable behind `Arc`: an edit swaps the `Arc`,
/// so a snapshot for the store is a cheap clone and unchanged elements keep their pointer.
#[derive(Clone, Debug, Default)]
pub struct Document {
    elements: Vec<Arc<Element>>,
    revision: u64,
    generation: u64,
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
        Self {
            elements: elements.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn elements(&self) -> &[Arc<Element>] {
        &self.elements
    }

    /// Bumped by every applied command.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Bumped on any edit that is not a pure append, so renderers know to rebuild instead of append.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    // shortcut: linear scan, fine for thousands of elements; add an id → index map if edits on 20k+
    // element sketches show up in tracy.
    pub fn index_of(&self, id: ElementId) -> Option<usize> {
        self.elements.iter().position(|e| e.id == id)
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

    /// Every stroke's capsules in z-order. Allocation-free: the renderer walks this every frame.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.elements
            .iter()
            .filter_map(|e| match &e.kind {
                ElementKind::Stroke(s) => Some(s),
                ElementKind::Shape(_) | ElementKind::Recognized(_) => None,
            })
            .flat_map(stroke_segments)
    }

    pub(crate) fn insert(&mut self, index: usize, element: Arc<Element>) {
        if index != self.elements.len() {
            self.generation += 1;
        }
        self.elements.insert(index, element);
        self.revision += 1;
    }

    pub(crate) fn remove(&mut self, index: usize) -> Arc<Element> {
        self.generation += 1;
        self.revision += 1;
        self.elements.remove(index)
    }

    pub(crate) fn replace(&mut self, index: usize, element: Arc<Element>) -> Arc<Element> {
        self.generation += 1;
        self.revision += 1;
        std::mem::replace(&mut self.elements[index], element)
    }
}

pub fn stroke_segments(s: &Stroke) -> impl Iterator<Item = Segment> + '_ {
    let (half_width, color) = (s.width * 0.5, s.color);
    // A single point is a dot: one zero-length capsule. `windows(2)` yields nothing for it.
    let dot = (s.points.len() == 1).then(|| (s.points[0], s.points[0]));
    dot.into_iter()
        .chain(s.points.windows(2).map(|w| (w[0], w[1])))
        .map(move |(a, b)| Segment {
            a,
            b,
            half_width,
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
    fn append_keeps_generation_other_edits_bump_it() {
        let mut doc = Document::default();
        let el = |x| Arc::new(Element::stroke(Stroke::new(Vec2::splat(x), 1.0, [1.0; 4])));
        doc.insert(0, el(1.0));
        doc.insert(1, el(2.0));
        assert_eq!((doc.generation(), doc.revision()), (0, 2));
        doc.insert(0, el(3.0));
        assert_eq!((doc.generation(), doc.revision()), (1, 3));
        doc.remove(2);
        assert_eq!((doc.generation(), doc.revision()), (2, 4));
    }
}
