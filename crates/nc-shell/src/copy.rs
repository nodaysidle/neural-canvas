//! Every string the shell shows, in one place, so the copy audit (UX principle 10) can read all of it.
//! Plain, specific, sentence case; no internal names, no debug text, no em dashes.

use nc_core::recognize::QualityStatus;
use nc_core::{ShapeKind, Tool};

pub const APP_NAME: &str = "NeuralCanvas";
pub const EMPTY_HINT: &str = "Draw with P, pan with Space";
pub const SKETCHES: &str = "Sketches";
pub const PROJECTS: &str = "Projects";
pub const NEW_PROJECT: &str = "New project";
pub const NEW_SKETCH: &str = "New sketch";
pub const DEFAULT_PROJECT: &str = "My Project";
pub const DEFAULT_SKETCH: &str = "Untitled Sketch";
pub const INSPECTOR: &str = "Inspector";
pub const EXPORT: &str = "Export";
pub const EXPORT_WIREFRAME: &str = "Export wireframe";
pub const SHORTCUTS: &str = "Keyboard shortcuts";
pub const PREFERENCES: &str = "Preferences";
pub const CLOSE: &str = "Close";
pub const DISMISS: &str = "Dismiss";
pub const RETRY: &str = "Retry";
pub const REVIEW: &str = "Review";
pub const OPEN_FOLDER: &str = "Open folder";
pub const SAVED: &str = "Saved";
pub const SAVING: &str = "Saving…";
pub const NOT_SAVED: &str = "Not saved";
pub const READ_ONLY: &str = "Read-only";
pub const NOTHING_SELECTED: &str = "Select something on the canvas to edit it here.";
pub const INSPECT_TYPE: &str = "Type";
pub const INSPECT_LABEL: &str = "Label";
pub const INSPECT_POSITION: &str = "Position";
pub const INSPECT_SIZE: &str = "Size";
pub const INSPECT_STROKE: &str = "Stroke width";
pub const INSPECT_CONFIDENCE: &str = "Confidence";
pub const INSPECT_UNCERTAIN: &str = "Uncertain: check this one";
pub const RESET_WIREFRAME: &str = "Use recognised type";
pub const LOADING_SKETCH: &str = "Opening sketch…";
pub const ZOOM_TO_FIT: &str = "Zoom to fit";
pub const COLOR: &str = "Colour";
pub const WIDTH: &str = "Width";

/// Tool names and their keys, as the palette and the shortcuts sheet show them.
pub const TOOLS: [(Tool, &str, &str); 7] = [
    (Tool::Select, "Select", "V"),
    (Tool::Pen, "Pen", "P"),
    (Tool::Eraser, "Eraser", "E"),
    (Tool::Hand, "Hand", "H"),
    (Tool::Shape(ShapeKind::Rectangle), "Rectangle", "R"),
    (Tool::Shape(ShapeKind::Ellipse), "Ellipse", "O"),
    (Tool::Shape(ShapeKind::Line), "Line", "L"),
];

pub const INK_NAMES: [&str; 5] = ["Graphite", "Coral", "Sky", "Mint", "Sun"];
pub const WIDTH_NAMES: [&str; 4] = ["Fine", "Medium", "Bold", "Marker"];

/// The wireframe view modes, left to right in the top bar.
pub const MODES: [&str; 4] = ["Sketch", "Hybrid", "Wireframe", "Side by side"];

pub const THEME: &str = "Theme";
pub const THEMES: [&str; 3] = ["Match system", "Dark", "Light"];
pub const GRID: &str = "Grid";
pub const SMOOTHING: &str = "Ink smoothing";
pub const PRESSURE: &str = "Pen pressure";
pub const PRESSURES: [&str; 3] = ["Soft", "Linear", "Firm"];
pub const ON_OFF: [&str; 2] = ["On", "Off"];

/// The shortcuts sheet: (group, [(what, keys)]).
pub const SHORTCUT_GROUPS: [(&str, &[(&str, &str)]); 5] = [
    (
        "Tools",
        &[
            ("Select", "V"),
            ("Pen", "P"),
            ("Eraser", "E"),
            ("Hand", "H"),
            ("Hand while held", "Space"),
            ("Rectangle, ellipse, line", "R  O  L"),
            ("Square, circle, 45°", "Shift + drag"),
            ("From the centre", "Alt + drag"),
        ],
    ),
    (
        "Edit",
        &[
            ("Undo", "Ctrl+Z"),
            ("Redo", "Ctrl+Shift+Z"),
            ("Select all", "Ctrl+A"),
            ("Lasso", "Alt + drag"),
            ("Duplicate", "Ctrl+D"),
            ("Copy, paste", "Ctrl+C  Ctrl+V"),
            ("Delete", "Delete"),
            ("Nudge, 10 px", "Arrows, Shift+Arrows"),
        ],
    ),
    (
        "View",
        &[
            ("Zoom to fit", "Ctrl+0"),
            ("Actual size", "Ctrl+1"),
            ("Zoom to selection", "Ctrl+2"),
            ("Zoom in, out", "Ctrl+=  Ctrl+-"),
            ("Sketches", "Ctrl+B"),
            ("Inspector", "Ctrl+I"),
            ("Move between controls", "Tab"),
        ],
    ),
    (
        "Wireframe",
        &[
            ("Sketch, hybrid, wireframe", "W"),
            ("More, less wireframe", "]  ["),
            ("Side by side", "Shift+W"),
            ("Review uncertain items", "U"),
        ],
    ),
    (
        "File",
        &[
            ("Export sketch", "Ctrl+E"),
            ("Export wireframe", "Ctrl+Shift+E"),
            ("Preferences", "Ctrl+,"),
            ("This sheet", "?"),
        ],
    ),
];

pub fn quality(status: QualityStatus) -> &'static str {
    status.display_name()
}

pub fn review_count(n: usize) -> String {
    match n {
        1 => "Review 1 uncertain item".into(),
        n => format!("Review {n} uncertain items"),
    }
}

pub fn zoom(scale: f32) -> String {
    format!("{:.0} %", scale * 100.0)
}

pub fn opacity(level: f32) -> String {
    format!("{:.0} %", level * 100.0)
}

pub fn selection_count(n: usize) -> String {
    match n {
        1 => "1 item".into(),
        n => format!("{n} items"),
    }
}

/// What the canvas holds, for the inspector and screen readers (UX principle 9).
pub fn canvas_summary(strokes: usize, shapes: usize, recognised: usize) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    format!(
        "Sketch: {}, {}, {}",
        plural(strokes, "stroke", "strokes"),
        plural(shapes, "shape", "shapes"),
        plural(recognised, "recognised element", "recognised elements"),
    )
}

pub fn not_saved(reason: &str) -> String {
    format!("Changes are not being saved: {reason}")
}

pub const READ_ONLY_BANNER: &str = "Part of this sketch could not be read. What could be read is shown \
     read-only, and nothing will be saved over it.";

/// The blocking dialog when the sketch store can't be opened (Mac bug #4: no silent fallback).
pub const STORE_TITLE: &str = "Your sketches can't be saved";
pub fn store_unavailable(reason: &str) -> String {
    format!(
        "NeuralCanvas can't open the place it keeps your sketches ({reason}). Drawing stays off \
         until it can, so nothing is lost. Try again, or choose another folder."
    )
}
pub const CHOOSE_LOCATION: &str = "Choose location…";
pub const CHOOSE_LOCATION_TITLE: &str = "Choose a folder for your sketches";
pub const QUIT: &str = "Quit";

pub fn exported(name: &str) -> String {
    format!("Exported “{name}” as SVG, PDF, PNG and JPEG")
}

pub fn export_failed(reason: &str) -> String {
    format!("Export failed: {reason}")
}

pub fn copy_failed(reason: &str) -> String {
    format!("Copy failed: {reason}")
}

pub fn paste_failed(reason: &str) -> String {
    format!("Paste failed: {reason}")
}

pub const EXPORTING: &str = "Exporting…";

/// Tooltips: what a control does and its key (UX principle 4: every tooltip shows its shortcut).
pub const TIP_SKETCHES: &str = "Show or hide sketches · Ctrl+B";
pub const TIP_INSPECTOR: &str = "Show or hide the inspector · Ctrl+I";
pub const TIP_EXPORT: &str = "Export SVG, PDF, PNG and JPEG · Ctrl+E, wireframe Ctrl+Shift+E";
pub const TIP_SHORTCUTS: &str = "Keyboard shortcuts · ?";
pub const TIP_MODES: [&str; 4] = [
    "The sketch alone · W cycles",
    "Sketch and wireframe together · W cycles, [ ] fade",
    "The wireframe alone · W cycles",
    "Sketch and wireframe side by side · Shift+W",
];
pub const TIP_REVIEW: &str = "Zoom to each uncertain item · U";
pub const TIP_ZOOM: &str = "Zoom to fit · Ctrl+0, actual size Ctrl+1";

pub const NOTHING_TO_REVIEW: &str = "Nothing uncertain to review";
pub const NOTHING_TO_EXPORT: &str = "Nothing to export yet: draw something first";

pub fn reviewing(i: usize, n: usize) -> String {
    format!("Uncertain item {i} of {n}")
}

pub fn sketch_failed(reason: &str) -> String {
    format!("Could not open the sketch: {reason}")
}

pub fn project_failed(reason: &str) -> String {
    format!("Could not update projects: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nc_core::{Action, Key, Shortcut};

    /// Every string above, including the formatted ones with sample values.
    fn all() -> Vec<String> {
        let mut out: Vec<String> = [
            APP_NAME,
            EMPTY_HINT,
            SKETCHES,
            PROJECTS,
            NEW_PROJECT,
            NEW_SKETCH,
            DEFAULT_PROJECT,
            DEFAULT_SKETCH,
            INSPECTOR,
            EXPORT,
            EXPORT_WIREFRAME,
            SHORTCUTS,
            PREFERENCES,
            CLOSE,
            DISMISS,
            RETRY,
            REVIEW,
            OPEN_FOLDER,
            SAVED,
            SAVING,
            NOT_SAVED,
            READ_ONLY,
            NOTHING_SELECTED,
            INSPECT_TYPE,
            INSPECT_LABEL,
            INSPECT_POSITION,
            INSPECT_SIZE,
            INSPECT_STROKE,
            INSPECT_CONFIDENCE,
            INSPECT_UNCERTAIN,
            RESET_WIREFRAME,
            LOADING_SKETCH,
            ZOOM_TO_FIT,
            COLOR,
            WIDTH,
            THEME,
            GRID,
            SMOOTHING,
            PRESSURE,
            READ_ONLY_BANNER,
            NOTHING_TO_REVIEW,
            NOTHING_TO_EXPORT,
            EXPORTING,
            STORE_TITLE,
            CHOOSE_LOCATION,
            CHOOSE_LOCATION_TITLE,
            QUIT,
            TIP_SKETCHES,
            TIP_INSPECTOR,
            TIP_EXPORT,
            TIP_SHORTCUTS,
            TIP_REVIEW,
            TIP_ZOOM,
        ]
        .map(String::from)
        .to_vec();
        out.extend(
            TOOLS
                .iter()
                .flat_map(|(_, n, k)| [n.to_string(), k.to_string()]),
        );
        out.extend(INK_NAMES.map(String::from));
        out.extend(WIDTH_NAMES.map(String::from));
        out.extend(MODES.map(String::from));
        out.extend(TIP_MODES.map(String::from));
        out.extend(THEMES.map(String::from));
        out.extend(PRESSURES.map(String::from));
        for (group, rows) in SHORTCUT_GROUPS {
            out.push(group.into());
            out.extend(
                rows.iter()
                    .flat_map(|(a, b)| [a.to_string(), b.to_string()]),
            );
        }
        for status in [
            QualityStatus::Ready,
            QualityStatus::ReviewRecommended,
            QualityStatus::LowConfidence,
        ] {
            out.push(quality(status).into());
        }
        for t in nc_core::ShapeType::ALL {
            out.push(t.display_name().into());
        }
        out.extend([
            review_count(1),
            review_count(3),
            zoom(1.0),
            opacity(0.5),
            selection_count(2),
            canvas_summary(12, 1, 5),
            not_saved("disk full"),
            exported("Sketch 1"),
            export_failed("disk full"),
            copy_failed("no clipboard"),
            paste_failed("no clipboard"),
            reviewing(1, 3),
            sketch_failed("damaged"),
            project_failed("disk full"),
            store_unavailable("permission denied"),
        ]);
        // Menu labels come from the actions themselves.
        let history = nc_core::History::default();
        for key in "zyacdveuw[]012=+-bi,/".chars() {
            for (ctrl, shift) in [(true, false), (true, true), (false, false), (false, true)] {
                if let Some(a) = (Shortcut {
                    ctrl,
                    shift,
                    key: Key::Char(key),
                })
                .action()
                {
                    out.push(a.label(&history));
                    out.push(Action::shortcut_hint(a).into());
                }
            }
        }
        out
    }

    /// UX principle 10: no internal names, debug text or TODO phrasing anywhere in the interface.
    #[test]
    fn copy_has_no_internal_or_debug_text() {
        let banned = [
            "todo",
            "fixme",
            "xxx",
            "debug",
            "nc-",
            "nc_",
            "phase",
            "shortcut:",
            "placeholder",
            "lorem",
            "test",
            "stub",
            "unwrap",
            "panic",
            "null",
            "undefined",
            "none",
            "some(",
            "err(",
            "ok(",
            "{",
            "}",
            "::",
            "—",
        ];
        for s in all() {
            let lower = s.to_lowercase();
            for b in banned {
                assert!(!lower.contains(b), "“{s}” contains “{b}”");
            }
            assert!(!s.trim().is_empty(), "empty string in the copy");
            assert_eq!(s.trim(), s, "stray whitespace in “{s}”");
            assert!(!s.contains("..."), "“{s}”: use … not ...");
        }
    }

    #[test]
    fn summaries_count_properly() {
        assert_eq!(
            canvas_summary(12, 1, 5),
            "Sketch: 12 strokes, 1 shape, 5 recognised elements"
        );
        assert_eq!(review_count(1), "Review 1 uncertain item");
        assert_eq!(zoom(1.25), "125 %");
    }
}
