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
}

/// A key press with `Ctrl` / `Shift` state. `key` is the unshifted character, lowercase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub key: char,
}

impl Shortcut {
    pub fn action(self) -> Option<Action> {
        match (self.ctrl, self.shift, self.key) {
            (true, false, 'z') => Some(Action::Undo),
            (true, true, 'z') | (true, false, 'y') => Some(Action::Redo),
            (true, false, '0') => Some(Action::FitToContent),
            (true, false, '1') => Some(Action::ActualSize),
            _ => None,
        }
    }
}

impl Action {
    pub fn enabled(self, history: &History) -> bool {
        match self {
            Self::Undo => history.undo_label().is_some(),
            Self::Redo => history.redo_label().is_some(),
            Self::FitToContent | Self::ActualSize => true,
        }
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
        }
    }

    pub fn shortcut_hint(self) -> &'static str {
        match self {
            Self::Undo => "Ctrl+Z",
            Self::Redo => "Ctrl+Shift+Z",
            Self::FitToContent => "Ctrl+0",
            Self::ActualSize => "Ctrl+1",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Document, Element, Stroke};
    use glam::Vec2;

    fn key(ctrl: bool, shift: bool, key: char) -> Option<Action> {
        Shortcut { ctrl, shift, key }.action()
    }

    #[test]
    fn linux_undo_redo_shortcuts() {
        assert_eq!(key(true, false, 'z'), Some(Action::Undo));
        assert_eq!(key(true, true, 'z'), Some(Action::Redo));
        assert_eq!(key(true, false, 'y'), Some(Action::Redo));
        assert_eq!(key(false, false, 'z'), None);
    }

    /// Mac bug #7 guard: Undo/Redo enabled state and labels follow the history.
    #[test]
    fn undo_enabled_follows_history() {
        let mut doc = Document::default();
        let mut history = History::default();
        assert!(!Action::Undo.enabled(&history));
        assert_eq!(Action::Undo.label(&history), "Undo");

        let stroke = Element::stroke(Stroke::new(Vec2::ZERO, 2.0, [1.0; 4]));
        let id = stroke.id;
        let draw = Command::append(stroke);
        history.execute(&mut doc, "Draw", draw).unwrap();
        history
            .execute(&mut doc, "Erase", Command::Remove { id })
            .unwrap();
        assert!(Action::Undo.enabled(&history));
        assert!(!Action::Redo.enabled(&history));
        assert_eq!(Action::Undo.label(&history), "Undo Erase");

        history.undo(&mut doc).unwrap();
        assert_eq!(Action::Undo.label(&history), "Undo Draw");
        assert_eq!(Action::Redo.label(&history), "Redo Erase");
        assert!(Action::Redo.enabled(&history));
    }
}
