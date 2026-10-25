//! Which tool a press uses. Holding Space and turning the stylus to its eraser end are temporary: they
//! never overwrite the chosen tool, so releasing restores it (Mac bug #13).

use crate::element::ShapeKind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Pen,
    Eraser,
    Hand,
    Select,
    Shape(ShapeKind),
}

impl Tool {
    /// Single-key tool shortcuts (docs/UX-PRINCIPLES.md §4), unshifted and without Ctrl.
    pub fn for_key(key: char) -> Option<Self> {
        match key {
            'p' => Some(Self::Pen),
            'e' => Some(Self::Eraser),
            'h' => Some(Self::Hand),
            'v' => Some(Self::Select),
            'r' => Some(Self::Shape(ShapeKind::Rectangle)),
            'o' => Some(Self::Shape(ShapeKind::Ellipse)),
            'l' => Some(Self::Shape(ShapeKind::Line)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Tools {
    chosen: Tool,
    space_held: bool,
    stylus_eraser: bool,
}

impl Tools {
    /// The tool a press would use right now.
    pub fn active(&self) -> Tool {
        if self.space_held {
            Tool::Hand
        } else if self.stylus_eraser {
            Tool::Eraser
        } else {
            self.chosen
        }
    }

    pub fn chosen(&self) -> Tool {
        self.chosen
    }

    pub fn choose(&mut self, tool: Tool) {
        self.chosen = tool;
    }

    /// Space pressed or released. Idempotent, so auto-repeat cannot confuse it.
    pub fn set_space(&mut self, held: bool) {
        self.space_held = held;
    }

    /// The stylus's eraser end entered or left proximity.
    pub fn set_stylus_eraser(&mut self, active: bool) {
        self.stylus_eraser = active;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mac bug #13 guard: Space while erasing → release → still the eraser.
    #[test]
    fn space_restores_the_previous_tool() {
        let mut tools = Tools::default();
        tools.choose(Tool::Eraser);
        tools.set_space(true);
        tools.set_space(true); // key repeat
        assert_eq!(tools.active(), Tool::Hand);
        tools.set_space(false);
        assert_eq!(tools.active(), Tool::Eraser);
    }

    #[test]
    fn choosing_while_space_is_held_applies_on_release() {
        let mut tools = Tools::default();
        tools.set_space(true);
        tools.choose(Tool::Eraser);
        assert_eq!(tools.active(), Tool::Hand);
        tools.set_space(false);
        assert_eq!(tools.active(), Tool::Eraser);
    }

    #[test]
    fn stylus_eraser_end_is_temporary() {
        let mut tools = Tools::default();
        tools.set_stylus_eraser(true);
        assert_eq!((tools.active(), tools.chosen()), (Tool::Eraser, Tool::Pen));
        tools.set_space(true);
        assert_eq!(tools.active(), Tool::Hand, "Space wins while held");
        tools.set_space(false);
        tools.set_stylus_eraser(false);
        assert_eq!(tools.active(), Tool::Pen);
    }

    #[test]
    fn tool_keys() {
        assert_eq!(Tool::for_key('p'), Some(Tool::Pen));
        assert_eq!(Tool::for_key('e'), Some(Tool::Eraser));
        assert_eq!(Tool::for_key('h'), Some(Tool::Hand));
        assert_eq!(Tool::for_key('v'), Some(Tool::Select));
        assert_eq!(Tool::for_key('r'), Some(Tool::Shape(ShapeKind::Rectangle)));
        assert_eq!(Tool::for_key('o'), Some(Tool::Shape(ShapeKind::Ellipse)));
        assert_eq!(Tool::for_key('l'), Some(Tool::Shape(ShapeKind::Line)));
        assert_eq!(Tool::for_key('x'), None);
    }
}
