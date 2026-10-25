//! Keyboard shortcuts → actions, and each action's enabled state and label. Menus and shortcuts read the
//! same answers, so Undo can never be greyed out in one and live in the other (Mac bug #7).

use crate::command::History;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Undo,
    Redo,
    /// Fit all content in view (`Ctrl+0`).
    FitToContent,
    /// 100 % zoom around the view centre (`Ctrl+1`).
    ActualSize,
    SelectAll,
    Deselect,
    Delete,
    Duplicate,
    Copy,
    Paste,
    /// Move the selection by this many world units (arrows: 1, with Shift: 10).
    Nudge {
        dx: i8,
        dy: i8,
    },
    /// Raw sketch → hybrid → wireframe → raw.
    CycleWireframe,
    /// Wireframe opacity up or down a step (`]` / `[`).
    WireframeOpacity {
        up: bool,
    },
    SideBySide,
    /// Zoom to the next element the quality gate flagged.
    ReviewUncertain,
    /// Write SVG, PDF, PNG and JPEG of the sketch, or of its wireframe.
    Export {
        wireframe: bool,
    },
}

/// A key, independent of the windowing library. Characters are unshifted and lowercase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Delete,
    Backspace,
    Escape,
    Left,
    Right,
    Up,
    Down,
}

/// A key press with `Ctrl` / `Shift` state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub key: Key,
}

impl Shortcut {
    pub fn action(self) -> Option<Action> {
        let step = if self.shift { 10 } else { 1 };
        let nudge = |dx, dy| Some(Action::Nudge { dx, dy });
        match (self.ctrl, self.shift, self.key) {
            (true, false, Key::Char('z')) => Some(Action::Undo),
            (true, true, Key::Char('z')) | (true, false, Key::Char('y')) => Some(Action::Redo),
            (true, false, Key::Char('0')) => Some(Action::FitToContent),
            (true, false, Key::Char('1')) => Some(Action::ActualSize),
            (true, false, Key::Char('a')) => Some(Action::SelectAll),
            (true, false, Key::Char('d')) => Some(Action::Duplicate),
            (true, false, Key::Char('c')) => Some(Action::Copy),
            (true, false, Key::Char('v')) => Some(Action::Paste),
            (true, shift, Key::Char('e')) => Some(Action::Export { wireframe: shift }),
            (false, false, Key::Char('w')) => Some(Action::CycleWireframe),
            (false, true, Key::Char('w')) => Some(Action::SideBySide),
            (false, false, Key::Char('[')) => Some(Action::WireframeOpacity { up: false }),
            (false, false, Key::Char(']')) => Some(Action::WireframeOpacity { up: true }),
            (false, false, Key::Char('u')) => Some(Action::ReviewUncertain),
            (false, false, Key::Delete | Key::Backspace) => Some(Action::Delete),
            (false, false, Key::Escape) => Some(Action::Deselect),
            (false, _, Key::Left) => nudge(-step, 0),
            (false, _, Key::Right) => nudge(step, 0),
            (false, _, Key::Up) => nudge(0, -step),
            (false, _, Key::Down) => nudge(0, step),
            _ => None,
        }
    }
}

impl Action {
    pub fn enabled(self, history: &History, has_selection: bool) -> bool {
        match self {
            Self::Undo => history.undo_label().is_some(),
            Self::Redo => history.redo_label().is_some(),
            Self::Deselect | Self::Delete | Self::Duplicate | Self::Copy | Self::Nudge { .. } => {
                has_selection
            }
            Self::FitToContent
            | Self::ActualSize
            | Self::SelectAll
            | Self::Paste
            | Self::CycleWireframe
            | Self::WireframeOpacity { .. }
            | Self::SideBySide
            | Self::ReviewUncertain
            | Self::Export { .. } => true,
        }
    }

    /// Whether the action changes the document (refused on a read-only sketch).
    pub fn edits(self) -> bool {
        matches!(
            self,
            Self::Undo
                | Self::Redo
                | Self::Delete
                | Self::Duplicate
                | Self::Paste
                | Self::Nudge { .. }
        )
    }

    /// Menu text, e.g. "Undo Erase".
    pub fn label(self, history: &History) -> String {
        match self {
            Self::Undo => history
                .undo_label()
                .map_or("Undo".into(), |l| format!("Undo {l}")),
            Self::Redo => history
                .redo_label()
                .map_or("Redo".into(), |l| format!("Redo {l}")),
            Self::FitToContent => "Zoom to Fit".into(),
            Self::ActualSize => "Actual Size".into(),
            Self::SelectAll => "Select All".into(),
            Self::Deselect => "Deselect".into(),
            Self::Delete => "Delete".into(),
            Self::Duplicate => "Duplicate".into(),
            Self::Copy => "Copy".into(),
            Self::Paste => "Paste".into(),
            Self::Nudge { .. } => "Nudge".into(),
            Self::CycleWireframe => "Show Wireframe".into(),
            Self::WireframeOpacity { up: true } => "More Wireframe".into(),
            Self::WireframeOpacity { up: false } => "Less Wireframe".into(),
            Self::SideBySide => "Side by Side".into(),
            Self::ReviewUncertain => "Review Uncertain Items".into(),
            Self::Export { wireframe: false } => "Export Sketch".into(),
            Self::Export { wireframe: true } => "Export Wireframe".into(),
        }
    }

    pub fn shortcut_hint(self) -> &'static str {
        match self {
            Self::Undo => "Ctrl+Z",
            Self::Redo => "Ctrl+Shift+Z",
            Self::FitToContent => "Ctrl+0",
            Self::ActualSize => "Ctrl+1",
            Self::SelectAll => "Ctrl+A",
            Self::Deselect => "Esc",
            Self::Delete => "Delete",
            Self::Duplicate => "Ctrl+D",
            Self::Copy => "Ctrl+C",
            Self::Paste => "Ctrl+V",
            Self::Nudge { .. } => "Arrows",
            Self::CycleWireframe => "W",
            Self::WireframeOpacity { up: true } => "]",
            Self::WireframeOpacity { up: false } => "[",
            Self::SideBySide => "Shift+W",
            Self::ReviewUncertain => "U",
            Self::Export { wireframe: false } => "Ctrl+E",
            Self::Export { wireframe: true } => "Ctrl+Shift+E",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Document, Element, Stroke};
    use glam::Vec2;

    fn key(ctrl: bool, shift: bool, key: char) -> Option<Action> {
        Shortcut {
            ctrl,
            shift,
            key: Key::Char(key),
        }
        .action()
    }

    #[test]
    fn linux_undo_redo_shortcuts() {
        assert_eq!(key(true, false, 'z'), Some(Action::Undo));
        assert_eq!(key(true, true, 'z'), Some(Action::Redo));
        assert_eq!(key(true, false, 'y'), Some(Action::Redo));
        assert_eq!(key(false, false, 'z'), None);
    }

    #[test]
    fn editing_shortcuts() {
        assert_eq!(key(true, false, 'a'), Some(Action::SelectAll));
        assert_eq!(key(true, false, 'd'), Some(Action::Duplicate));
        assert_eq!(key(true, false, 'c'), Some(Action::Copy));
        assert_eq!(key(true, false, 'v'), Some(Action::Paste));
        assert_eq!(key(false, false, 'v'), None, "plain V is the select tool");
        assert_eq!(key(false, false, 'w'), Some(Action::CycleWireframe));
        assert_eq!(key(false, true, 'w'), Some(Action::SideBySide));
        assert_eq!(
            key(false, false, ']'),
            Some(Action::WireframeOpacity { up: true })
        );
        assert_eq!(key(false, false, 'u'), Some(Action::ReviewUncertain));
        assert_eq!(
            key(true, true, 'e'),
            Some(Action::Export { wireframe: true })
        );
        assert_eq!(
            key(true, false, 'e'),
            Some(Action::Export { wireframe: false })
        );
        assert_eq!(key(false, false, 'e'), None, "plain E is the eraser");
        let named = |shift, key| {
            Shortcut {
                ctrl: false,
                shift,
                key,
            }
            .action()
        };
        assert_eq!(named(false, Key::Delete), Some(Action::Delete));
        assert_eq!(named(false, Key::Backspace), Some(Action::Delete));
        assert_eq!(named(false, Key::Escape), Some(Action::Deselect));
        assert_eq!(
            named(false, Key::Left),
            Some(Action::Nudge { dx: -1, dy: 0 })
        );
        assert_eq!(
            named(true, Key::Down),
            Some(Action::Nudge { dx: 0, dy: 10 })
        );

        let history = History::default();
        assert!(!Action::Delete.enabled(&history, false));
        assert!(Action::Delete.enabled(&history, true));
        assert!(Action::Paste.enabled(&history, false));
        assert!(Action::Nudge { dx: 1, dy: 0 }.edits() && !Action::Copy.edits());
    }

    /// Mac bug #7 guard: Undo/Redo enabled state and labels follow the history.
    #[test]
    fn undo_enabled_follows_history() {
        let mut doc = Document::default();
        let mut history = History::default();
        assert!(!Action::Undo.enabled(&history, false));
        assert_eq!(Action::Undo.label(&history), "Undo");

        let stroke = Element::stroke(Stroke::new(Vec2::ZERO, 2.0, [1.0; 4]));
        let id = stroke.id;
        let draw = Command::append(stroke);
        history.execute(&mut doc, "Draw", draw).unwrap();
        history
            .execute(&mut doc, "Erase", Command::Remove { id })
            .unwrap();
        assert!(Action::Undo.enabled(&history, false));
        assert!(!Action::Redo.enabled(&history, false));
        assert_eq!(Action::Undo.label(&history), "Undo Erase");

        history.undo(&mut doc).unwrap();
        assert_eq!(Action::Undo.label(&history), "Undo Draw");
        assert_eq!(Action::Redo.label(&history), "Redo Erase");
        assert!(Action::Redo.enabled(&history, false));
    }
}
