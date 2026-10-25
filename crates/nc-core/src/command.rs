//! Edits as values. Applying a command returns its exact inverse, so undo never re-derives state
//! (Mac bug #6: undo off by one). History lives in a [`Session`], one per open sketch (Mac bug #2).

use crate::document::Document;
use crate::element::{Element, ElementId};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Put the element on top, whatever the length is when the command runs.
    Append {
        element: Arc<Element>,
    },
    /// Insert at z-index `index` (inverse of `Remove`; restores the exact position).
    Insert {
        index: usize,
        element: Arc<Element>,
    },
    Remove {
        id: ElementId,
    },
    /// Swap in new content for the element with the same id.
    Replace {
        element: Arc<Element>,
    },
    /// Applied in order, undone in reverse. All or nothing.
    Batch(Vec<Command>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    UnknownElement(ElementId),
    DuplicateElement(ElementId),
    IndexOutOfRange { index: usize, len: usize },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownElement(id) => write!(f, "no element {id}"),
            Self::DuplicateElement(id) => write!(f, "element {id} already exists"),
            Self::IndexOutOfRange { index, len } => {
                write!(f, "index {index} out of range for {len} elements")
            }
        }
    }
}

impl std::error::Error for CommandError {}

impl Command {
    pub fn append(element: Element) -> Self {
        Self::Append {
            element: Arc::new(element),
        }
    }

    /// Applies the command and returns its inverse. On error the document is unchanged.
    pub fn apply(self, doc: &mut Document) -> Result<Command, CommandError> {
        match self {
            Self::Append { element } => Self::Insert {
                index: doc.elements().len(),
                element,
            }
            .apply(doc),
            Self::Insert { index, element } => {
                let len = doc.elements().len();
                if index > len {
                    return Err(CommandError::IndexOutOfRange { index, len });
                }
                if doc.index_of(element.id).is_some() {
                    return Err(CommandError::DuplicateElement(element.id));
                }
                let id = element.id;
                doc.insert(index, element);
                Ok(Self::Remove { id })
            }
            Self::Remove { id } => {
                let index = doc.index_of(id).ok_or(CommandError::UnknownElement(id))?;
                let element = doc.remove(index);
                Ok(Self::Insert { index, element })
            }
            Self::Replace { element } => {
                let index = doc
                    .index_of(element.id)
                    .ok_or(CommandError::UnknownElement(element.id))?;
                let old = doc.replace(index, element);
                Ok(Self::Replace { element: old })
            }
            Self::Batch(commands) => {
                let mut inverses = Vec::with_capacity(commands.len());
                for command in commands {
                    match command.apply(doc) {
                        Ok(inverse) => inverses.push(inverse),
                        Err(e) => {
                            for inverse in inverses.into_iter().rev() {
                                inverse
                                    .apply(doc)
                                    .expect("inverse of an applied command always applies");
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Self::Batch(inverses))
            }
        }
    }
}

/// One undo step: a user-facing label ("Draw", "Erase") and the command that reverts it (or, on the
/// redo stack, re-applies it).
#[derive(Debug)]
struct Step {
    label: &'static str,
    command: Command,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    pub fn execute(
        &mut self,
        doc: &mut Document,
        label: &'static str,
        command: Command,
    ) -> Result<(), CommandError> {
        let inverse = command.apply(doc)?;
        self.record(label, inverse);
        Ok(())
    }

    /// Records a step whose command was already applied; `inverse` is what `apply` returned.
    pub fn record(&mut self, label: &'static str, inverse: Command) {
        self.undo.push(Step {
            label,
            command: inverse,
        });
        self.redo.clear();
    }

    /// Reverts the last step. `Ok(false)` when there is nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Result<bool, CommandError> {
        Self::shift(&mut self.undo, &mut self.redo, doc)
    }

    /// Re-applies the last undone step. `Ok(false)` when there is nothing to redo.
    pub fn redo(&mut self, doc: &mut Document) -> Result<bool, CommandError> {
        Self::shift(&mut self.redo, &mut self.undo, doc)
    }

    fn shift(
        from: &mut Vec<Step>,
        to: &mut Vec<Step>,
        doc: &mut Document,
    ) -> Result<bool, CommandError> {
        let Some(step) = from.pop() else {
            return Ok(false);
        };
        match step.command.clone().apply(doc) {
            Ok(command) => {
                to.push(Step {
                    label: step.label,
                    command,
                });
                Ok(true)
            }
            Err(e) => {
                from.push(step);
                Err(e)
            }
        }
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|s| s.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|s| s.label)
    }
}

/// An open sketch: its document and its own history. Switching sketches means dropping this and
/// opening another, so undo can never reach across sketches.
#[derive(Debug)]
pub struct Session {
    pub sketch_id: Uuid,
    doc: Document,
    history: History,
    gesture: Option<Gesture>,
}

/// A drag in progress: its label and the inverses of what it applied so far, oldest first.
#[derive(Debug)]
struct Gesture {
    label: &'static str,
    inverses: Vec<Command>,
    /// Elements whose content an earlier inverse already restores.
    replaced: HashSet<ElementId>,
}

impl Gesture {
    /// Keeps one content-restoring inverse per element: undo applies the oldest one last, so later ones
    /// would be overwritten anyway. An eraser drag that cuts the same stroke a hundred times keeps one.
    fn push(&mut self, inverse: Command) {
        match inverse {
            // A batch's inverse runs in order; flattened into this oldest-first list it must go reversed.
            Command::Batch(commands) => commands.into_iter().rev().for_each(|c| self.push(c)),
            Command::Replace { element } => {
                if self.replaced.insert(element.id) {
                    self.inverses.push(Command::Replace { element });
                }
            }
            other => self.inverses.push(other),
        }
    }
}

impl Session {
    pub fn new(sketch_id: Uuid, doc: Document) -> Self {
        Self {
            sketch_id,
            doc,
            history: History::default(),
            gesture: None,
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn execute(&mut self, label: &'static str, command: Command) -> Result<(), CommandError> {
        self.end_gesture();
        self.history.execute(&mut self.doc, label, command)
    }

    /// Applies part of a drag (each cut an eraser drag makes) right away, so the
    /// canvas updates live, while the whole drag stays one undo step. Ends with
    /// [`end_gesture`](Self::end_gesture).
    pub fn apply_in_gesture(
        &mut self,
        label: &'static str,
        command: Command,
    ) -> Result<(), CommandError> {
        let inverse = command.apply(&mut self.doc)?;
        if self.gesture.as_ref().is_none_or(|g| g.label != label) {
            self.end_gesture();
            self.gesture = Some(Gesture {
                label,
                inverses: Vec::new(),
                replaced: HashSet::new(),
            });
        }
        let gesture = self.gesture.as_mut().expect("opened above");
        gesture.push(inverse);
        Ok(())
    }

    /// Records the drag in progress as one undo step. Nothing happens if the drag changed nothing.
    pub fn end_gesture(&mut self) {
        if let Some(mut g) = self.gesture.take()
            && !g.inverses.is_empty()
        {
            g.inverses.reverse();
            self.history.record(g.label, Command::Batch(g.inverses));
        }
    }

    pub fn undo(&mut self) -> Result<bool, CommandError> {
        self.end_gesture();
        self.history.undo(&mut self.doc)
    }

    pub fn redo(&mut self) -> Result<bool, CommandError> {
        self.end_gesture();
        self.history.redo(&mut self.doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementKind, Stroke};
    use glam::Vec2;
    use proptest::prelude::*;

    fn stroke_at(x: f32) -> Element {
        let mut s = Stroke::new(Vec2::new(x, 0.0), 2.0, [1.0; 4]);
        s.push(Vec2::new(x, 10.0));
        Element::stroke(s)
    }

    /// Mac bug #6 guard: draw A, B; undo twice is empty; redo twice is A + B.
    #[test]
    fn undo_redo_is_exact() {
        let mut session = Session::new(Uuid::new_v4(), Document::default());
        let (a, b) = (stroke_at(1.0), stroke_at(2.0));
        let (ida, idb) = (a.id, b.id);
        let cmd = Command::append(a);
        session.execute("Draw", cmd).unwrap();
        let cmd = Command::append(b);
        session.execute("Draw", cmd).unwrap();
        assert!(session.undo().unwrap());
        assert!(session.undo().unwrap());
        assert!(session.doc().elements().is_empty());
        assert!(!session.undo().unwrap(), "nothing left to undo");
        assert!(session.redo().unwrap());
        assert!(session.redo().unwrap());
        let ids: Vec<_> = session.doc().elements().iter().map(|e| e.id).collect();
        assert_eq!(ids, [ida, idb]);
        assert!(!session.redo().unwrap());
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let strokes: Vec<Element> = (0..5).map(|i| stroke_at(i as f32)).collect();
        let doc = Document::from_elements(strokes.iter().cloned().map(Arc::new));
        let original = doc.elements().to_vec();
        let mut session = Session::new(Uuid::new_v4(), doc);
        for i in [3, 0, 1] {
            let id = strokes[i].id;
            session
                .apply_in_gesture("Erase", Command::Remove { id })
                .unwrap();
        }
        assert_eq!(session.doc().elements().len(), 2, "applied live");
        session.end_gesture();
        assert_eq!(session.history().undo_label(), Some("Erase"));
        assert!(session.undo().unwrap());
        assert_eq!(
            session.doc().elements(),
            original.as_slice(),
            "every stroke back in place"
        );
        assert!(!session.undo().unwrap(), "the whole drag was one step");
        assert!(session.redo().unwrap());
        assert_eq!(session.doc().elements().len(), 2);
    }

    #[test]
    fn an_empty_drag_records_nothing() {
        let mut session = Session::new(Uuid::new_v4(), Document::default());
        session.end_gesture();
        assert_eq!(session.history().undo_label(), None);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut session = Session::new(Uuid::new_v4(), Document::default());
        let cmd = Command::append(stroke_at(1.0));
        session.execute("Draw", cmd).unwrap();
        session.undo().unwrap();
        assert_eq!(session.history().redo_label(), Some("Draw"));
        let cmd = Command::append(stroke_at(2.0));
        session.execute("Draw", cmd).unwrap();
        assert_eq!(session.history().redo_label(), None);
    }

    #[test]
    fn failed_batch_leaves_document_unchanged() {
        let mut doc = Document::default();
        let a = stroke_at(1.0);
        Command::append(a.clone()).apply(&mut doc).unwrap();
        let before = doc.elements().to_vec();
        let batch = Command::Batch(vec![
            Command::Remove { id: a.id },
            Command::Remove { id: a.id }, // already gone: the whole batch must roll back
        ]);
        assert_eq!(
            batch.apply(&mut doc),
            Err(CommandError::UnknownElement(a.id))
        );
        assert_eq!(doc.elements(), before.as_slice());
    }

    #[test]
    fn rejects_invalid_commands() {
        let mut doc = Document::default();
        let a = Arc::new(stroke_at(1.0));
        let insert = |index| Command::Insert {
            index,
            element: a.clone(),
        };
        assert!(matches!(
            insert(1).apply(&mut doc),
            Err(CommandError::IndexOutOfRange { index: 1, len: 0 })
        ));
        insert(0).apply(&mut doc).unwrap();
        assert_eq!(
            insert(0).apply(&mut doc),
            Err(CommandError::DuplicateElement(a.id))
        );
        assert_eq!(
            doc.revision(),
            1,
            "failed commands do not bump the revision"
        );
    }

    /// One random edit, resolved against whatever the document holds at that moment.
    #[derive(Clone, Debug)]
    enum Op {
        Insert { at: usize, x: f32 },
        Remove { at: usize },
        Move { at: usize, dx: f32 },
        EraseAndRedraw { at: usize, x: f32 },
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (any::<usize>(), -1e4f32..1e4).prop_map(|(at, x)| Op::Insert { at, x }),
            any::<usize>().prop_map(|at| Op::Remove { at }),
            (any::<usize>(), -500f32..500.0).prop_map(|(at, dx)| Op::Move { at, dx }),
            (any::<usize>(), -1e4f32..1e4).prop_map(|(at, x)| Op::EraseAndRedraw { at, x }),
        ]
    }

    fn resolve(op: &Op, doc: &Document) -> Option<(&'static str, Command)> {
        let len = doc.elements().len();
        let pick = |at: usize| (len > 0).then(|| doc.elements()[at % len].clone());
        Some(match *op {
            Op::Insert { at, x } => (
                "Draw",
                Command::Insert {
                    index: at % (len + 1),
                    element: Arc::new(stroke_at(x)),
                },
            ),
            Op::Remove { at } => ("Delete", Command::Remove { id: pick(at)?.id }),
            Op::Move { at, dx } => {
                let mut moved = (*pick(at)?).clone();
                if let ElementKind::Stroke(s) = &mut moved.kind {
                    s.points.iter_mut().for_each(|p| p.x += dx);
                }
                (
                    "Move",
                    Command::Replace {
                        element: Arc::new(moved),
                    },
                )
            }
            Op::EraseAndRedraw { at, x } => (
                "Erase",
                Command::Batch(vec![
                    Command::Remove { id: pick(at)?.id },
                    Command::append(stroke_at(x)),
                ]),
            ),
        })
    }

    proptest! {
        /// Any command sequence, fully undone, gives back the original document; fully redone, the
        /// final one.
        #[test]
        fn undo_all_restores_original(
            initial in prop::collection::vec(-1e4f32..1e4, 0..8),
            ops in prop::collection::vec(op(), 0..40),
        ) {
            let doc = Document::from_elements(initial.into_iter().map(|x| Arc::new(stroke_at(x))));
            let original = doc.elements().to_vec();
            let mut session = Session::new(Uuid::new_v4(), doc);
            let mut steps = 0;
            for op in &ops {
                if let Some((label, command)) = resolve(op, session.doc()) {
                    session.execute(label, command).unwrap();
                    steps += 1;
                }
            }
            let last = session.doc().elements().to_vec();
            for _ in 0..steps {
                prop_assert!(session.undo().unwrap());
            }
            prop_assert!(!session.undo().unwrap());
            prop_assert_eq!(session.doc().elements(), original.as_slice());
            for _ in 0..steps {
                prop_assert!(session.redo().unwrap());
            }
            prop_assert_eq!(session.doc().elements(), last.as_slice());
        }

        /// The id index and the spatial index (which applies edits lazily, at the next query) agree
        /// with a brute-force scan after any mix of edits, undos and queries.
        #[test]
        fn indexes_match_brute_force(
            initial in prop::collection::vec(-1e4f32..1e4, 0..8),
            ops in prop::collection::vec((op(), any::<bool>(), any::<bool>()), 0..40),
            probe in -1e4f64..1e4,
        ) {
            let doc = Document::from_elements(initial.into_iter().map(|x| Arc::new(stroke_at(x))));
            let mut session = Session::new(Uuid::new_v4(), doc);
            let area = kurbo::Rect::new(probe, -5.0, probe + 4e3, 5.0);
            for (op, undo, query) in &ops {
                if let Some((label, command)) = resolve(op, session.doc()) {
                    session.execute(label, command).unwrap();
                }
                if *undo {
                    session.undo().unwrap();
                }
                let doc = session.doc();
                for (i, e) in doc.elements().iter().enumerate() {
                    prop_assert_eq!(doc.index_of(e.id), Some(i));
                }
                if *query {
                    let mut found = doc.query(area);
                    found.sort();
                    let mut expected: Vec<_> = doc
                        .elements()
                        .iter()
                        .filter(|e| e.bounds().is_some_and(|b| b.overlaps(area)))
                        .map(|e| e.id)
                        .collect();
                    expected.sort();
                    prop_assert_eq!(found, expected);
                }
            }
        }
    }
}
