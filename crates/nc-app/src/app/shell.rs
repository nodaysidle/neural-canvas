//! The chrome's side of the controller: builds the shell's status from the app, routes pointer and
//! keys to it, and performs what its controls ask for.

use super::{App, Drag};
use glam::Vec2;
use nc_core::kurbo::Affine;
use nc_core::recognize::{Recognized, draw};
use nc_core::select::{self, fit_affine};
use nc_core::{Action, Command, Element, ElementKind, Rect, Tool};
use nc_render::ui::{Font, Measure, UiLayer, UiText, text_height};
use nc_shell::theme::Palette;
use nc_shell::{
    Dialog, EditKey, Field, Hit, INKS, Inspect, Pref, SaveStatus, Status, ThemeChoice, ToastAction,
    ToastKind, WIDTHS, copy,
};
use std::sync::Arc;
use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};

impl App {
    pub(super) fn dark(&self) -> bool {
        match self.settings.theme {
            ThemeChoice::System => self.system_dark,
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
        }
    }

    pub(super) fn palette(&self) -> Palette {
        Palette::of(self.dark())
    }

    pub(super) fn toast_info(&mut self, message: String, action: Option<ToastAction>) {
        let now = self.now();
        self.shell.toast(ToastKind::Info, message, action, now);
        self.redraw();
    }

    pub(super) fn toast_error(&mut self, message: String) {
        let now = self.now();
        self.shell.toast(ToastKind::Error, message, None, now);
        self.redraw();
    }

    pub(super) fn sketch_name(&self) -> &str {
        self.projects
            .iter()
            .flat_map(|p| &p.sketches)
            .find(|s| s.id == self.session.sketch_id)
            .map_or(copy::DEFAULT_SKETCH, |s| s.name.as_str())
    }

    pub(super) fn update_title(&self) {
        if let Some(g) = &self.gpu {
            g.window
                .set_title(&format!("{} · {}", self.sketch_name(), copy::APP_NAME));
        }
    }

    /// The wireframe element the inspector edits, found again after every recognition pass by its
    /// sources (or the edit that pinned it).
    pub(super) fn selected_wire(&self) -> Option<&Recognized> {
        let key = self.wire_selected.as_ref()?;
        self.wireframe
            .as_ref()?
            .shapes
            .iter()
            .find(|s| &s.sources == key)
    }

    /// With the wireframe showing, a select-tool click picks the smallest wireframe element under it.
    pub(super) fn pick_wire(&mut self, world: Vec2) -> bool {
        let Some(wire) = &self.wireframe else {
            return false;
        };
        let p = nc_core::kurbo::Point::new(world.x as f64, world.y as f64);
        let found = wire
            .shapes
            .iter()
            .filter(|s| s.bounds.inflate(4.0, 4.0).contains(p))
            .min_by(|a, b| a.bounds.area().total_cmp(&b.bounds.area()))
            .map(|s| s.sources.clone());
        self.wire_selected = found;
        self.wire_selected.is_some()
    }

    fn inspect(&self) -> Inspect {
        if let Some(s) = self.selected_wire() {
            return Inspect::Wire {
                shape_type: s.shape_type,
                bounds: s.bounds,
                confidence: s.confidence,
                uncertain: s.is_uncertain(),
                label: s.label.clone(),
                pinned: s.pinned.is_some(),
            };
        }
        let doc = self.session.doc();
        let Some(bounds) = select::bounds(doc, &self.selection) else {
            return Inspect::Nothing;
        };
        let widths = self
            .selection
            .iter()
            .filter_map(|id| match &doc.get(*id)?.kind {
                ElementKind::Stroke(s) => Some(s.width),
                ElementKind::Shape(s) => Some(s.width),
                ElementKind::Recognized(_) => None,
            });
        let mut first = None;
        let mut common = true;
        for w in widths {
            match first {
                None => first = Some(w),
                Some(f) => common &= f == w,
            }
        }
        Inspect::Elements {
            count: self.selection.len(),
            bounds,
            width: first.filter(|_| common),
        }
    }

    /// What the canvas holds, for the inspector and screen readers.
    pub(super) fn summary(&self) -> String {
        let doc = self.session.doc();
        let (mut strokes, mut shapes) = (0, 0);
        for e in doc.elements() {
            match e.kind {
                ElementKind::Stroke(_) => strokes += 1,
                ElementKind::Shape(_) => shapes += 1,
                ElementKind::Recognized(_) => {}
            }
        }
        let recognised = self
            .wireframe
            .as_ref()
            .map_or(0, |w| w.report.semantic_count);
        copy::canvas_summary(strokes, shapes, recognised)
    }

    fn save_status(&self) -> SaveStatus {
        if let Some(e) = &self.store_error {
            SaveStatus::Failed(e.clone())
        } else if self.read_only {
            SaveStatus::ReadOnly
        } else if let Some(e) = &self.save_error {
            SaveStatus::Failed(e.clone())
        } else if self.saved_revision < self.session.doc().revision() {
            SaveStatus::Saving
        } else {
            SaveStatus::Saved
        }
    }

    /// Lays out the chrome for this frame, plus wireframe labels on the canvas.
    pub(super) fn build_chrome(&mut self) {
        // Measured before the GPU is taken out: the size comes from its surface.
        let size = self.logical_size();
        let Some(mut gpu) = self.gpu.take() else {
            return;
        };
        let status = Status {
            dark: self.dark(),
            tool: self.tools.active(),
            ink: self.ink,
            width: self.width,
            zoom: self.viewport.scale,
            wire_level: self.wire_level,
            split: self.split,
            projects: &self.projects,
            sketch: self.session.sketch_id,
            save: self.save_status(),
            quality: self
                .wireframe
                .as_ref()
                .filter(|_| self.wire_visible())
                .map(|w| (w.report.status(), w.report.uncertain_count)),
            inspect: self.inspect(),
            empty: self.session.doc().elements().is_empty(),
            drawing: matches!(
                self.drag,
                Some(Drag::Draw | Drag::Erase | Drag::Shape { .. } | Drag::Select(_))
            ),
            summary: self.summary(),
            settings: self.settings,
            loading: self.loading,
            store_error: self.store_error.as_deref(),
            style: nc_shell::StyleView {
                reading: self.style_reading,
                pending: self
                    .style_pending
                    .as_ref()
                    .map(|p| (&p.extracted.style, p.extracted.notes.as_slice())),
                applied: self.style.as_ref(),
                presets: &self.presets,
            },
        };
        self.shell.build(&status, size, &mut gpu.ui);
        let mut labels = std::mem::take(self.shell.canvas_layer());
        labels.clear();
        self.canvas_labels(&mut labels, &mut gpu.ui);
        *self.shell.canvas_layer() = labels;
        self.gpu = Some(gpu);
        self.update_access();
    }

    /// Wireframe labels, placed on screen where their elements are (in the wireframe pane).
    fn canvas_labels(&self, out: &mut UiLayer, measure: &mut dyn Measure) {
        let (Some(wire), true) = (&self.wireframe, self.wire_visible()) else {
            return;
        };
        let alpha = if self.split { 1.0 } else { self.blend().wire };
        let dx = if self.split { self.pane_size().x } else { 0.0 };
        let look = self.look();
        let size = draw::LABEL_SIZE * self.viewport.scale;
        if !(4.0..=200.0).contains(&size) {
            return; // too small to read, or absurdly large
        }
        for s in &wire.shapes {
            let Some((at, centred)) = draw::label_anchor(s) else {
                continue;
            };
            let mut color = draw::label_color(s, &look);
            color[3] *= alpha;
            let p = self.viewport.world_to_screen(at);
            let w = measure.width(&s.label, size, Font::Sans);
            let x = if centred { p.x - w / 2.0 } else { p.x };
            out.texts.push(UiText {
                text: s.label.clone(),
                at: [x + dx, p.y - text_height(size) / 2.0],
                size,
                font: Font::Sans,
                color,
                width: None,
            });
        }
    }

    /// Pointer moved: hover feedback. True when the pointer is over chrome.
    pub(super) fn chrome_hover(&mut self, p: Vec2) -> bool {
        if self.shell.pointer_moved(p) {
            self.redraw();
        }
        self.shell.covers(p)
    }

    /// Left button pressed at `p` (window px). True if the chrome took it.
    pub(super) fn chrome_press(&mut self, p: Vec2) -> bool {
        let Some(hit) = self.shell.hit(p) else {
            if self.shell.edit.is_some() {
                self.commit_edit();
            }
            self.shell.type_menu = false;
            return false;
        };
        if self
            .shell
            .edit
            .as_ref()
            .is_some_and(|e| Hit::Field(e.field) != hit)
        {
            self.commit_edit();
        }
        self.shell.pressed = Some(hit);
        self.redraw();
        true
    }

    /// Left button released: a control activates when pressed and released on it.
    pub(super) fn chrome_release(&mut self, p: Vec2) -> bool {
        let Some(pressed) = self.shell.pressed.take() else {
            return false;
        };
        if self.shell.hit(p) == Some(pressed) {
            self.activate(pressed);
        }
        self.redraw();
        true
    }

    fn commit_edit(&mut self) {
        if let Some((field, text)) = self.shell.edit_key(EditKey::Enter) {
            self.commit_field(field, text);
        }
    }

    /// Keys for the chrome: typing into a field, Tab between controls, Enter or Space on the focused
    /// one, Escape out of menus and dialogs. True when the key was the chrome's.
    pub(super) fn chrome_key(&mut self, event: &KeyEvent) -> bool {
        let shift = self.modifiers.shift_key();
        if self.shell.edit.is_some() {
            let keys: Vec<EditKey> = match &event.logical_key {
                Key::Named(NamedKey::Enter) => vec![EditKey::Enter],
                Key::Named(NamedKey::Escape) => vec![EditKey::Escape],
                Key::Named(NamedKey::Backspace) => vec![EditKey::Backspace],
                Key::Named(NamedKey::Delete) => vec![EditKey::Delete],
                Key::Named(NamedKey::ArrowLeft) => vec![EditKey::Left],
                Key::Named(NamedKey::ArrowRight) => vec![EditKey::Right],
                Key::Named(NamedKey::Home) => vec![EditKey::Home],
                Key::Named(NamedKey::End) => vec![EditKey::End],
                Key::Named(NamedKey::Tab) => {
                    self.commit_edit();
                    self.shell.focus_next(shift);
                    self.redraw();
                    return true;
                }
                _ if self.modifiers.control_key() => vec![],
                _ => event
                    .text
                    .as_ref()
                    .map(|t| t.chars().map(EditKey::Char).collect())
                    .unwrap_or_default(),
            };
            for k in keys {
                if let Some((field, text)) = self.shell.edit_key(k) {
                    self.commit_field(field, text);
                }
            }
            self.redraw();
            return true;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Tab) => {
                self.shell.focus_next(shift);
                self.redraw();
                true
            }
            Key::Named(NamedKey::Escape) if self.shell.type_menu => {
                self.shell.type_menu = false;
                self.redraw();
                true
            }
            Key::Named(NamedKey::Escape) if self.shell.dialog != Dialog::None => {
                self.shell.dialog = Dialog::None;
                self.redraw();
                true
            }
            Key::Named(NamedKey::Escape) if self.shell.focus.is_some() => {
                self.shell.focus = None;
                self.redraw();
                true
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) if self.shell.focus.is_some() => {
                if let Some(hit) = self.shell.focus {
                    self.activate(hit);
                }
                self.redraw();
                true
            }
            // A dialog takes the keyboard: canvas shortcuts wait until it closes.
            _ => self.shell.dialog != Dialog::None || self.store.is_none(),
        }
    }

    /// Does what a control asks.
    pub(super) fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Tool(tool) => self.choose_tool(tool),
            Hit::Ink(i) => self.set_ink(i),
            Hit::Width(i) => self.set_width(i),
            Hit::Sidebar => self.run(Action::ToggleSidebar),
            Hit::Inspector => self.run(Action::ToggleInspector),
            Hit::Shortcuts => self.run(Action::Shortcuts),
            Hit::Preferences => self.run(Action::Preferences),
            Hit::Mode(m) => match m {
                0 => self.set_wire(0.0, false),
                1 => self.set_wire(0.5, false),
                2 => self.set_wire(1.0, false),
                _ => self.set_wire(self.wire_level, !self.split),
            },
            Hit::Review => self.run(Action::ReviewUncertain),
            Hit::Export => self.run(Action::Export { wireframe: false }),
            Hit::ExportWireframe => self.run(Action::Export { wireframe: true }),
            Hit::ZoomToFit => self.run(Action::FitToContent),
            Hit::NewProject => self.new_project(),
            Hit::NewSketch(project) => self.new_sketch(project),
            Hit::Sketch(id) => self.open_sketch(id),
            Hit::Field(field) => {
                let text = self.field_text(field);
                self.shell.begin_edit(field, text);
                self.shell.focus = Some(hit);
            }
            Hit::TypeMenu => self.shell.type_menu = !self.shell.type_menu,
            Hit::TypeOption(t) => {
                self.shell.type_menu = false;
                self.edit_wire(|s| s.shape_type = t);
            }
            Hit::ResetWireframe => {
                if let Some(id) = self.selected_wire().and_then(|s| s.pinned) {
                    self.execute("Reset Wireframe", Command::Remove { id });
                }
            }
            Hit::Toast(id, action) => {
                self.shell.dismiss(id);
                match action {
                    ToastAction::Retry => self.persist(),
                    ToastAction::Review => self.run(Action::ReviewUncertain),
                    ToastAction::OpenFolder => self.open_export_folder(),
                }
            }
            Hit::DismissToast(id) => self.shell.dismiss(id),
            Hit::Retry if self.store.is_none() => self.retry_store(),
            Hit::Retry => {
                self.save_error = None;
                self.persist();
            }
            Hit::ChooseLocation => self.choose_store_location(),
            Hit::ImportStyle => self.import_style(),
            Hit::ApplyStyle => self.apply_style(),
            Hit::DiscardStyle => self.style_pending = None,
            Hit::RemoveStyle => self.set_style(None),
            Hit::SavePreset => self.save_preset(),
            Hit::Preset(id) => self.apply_preset(id),
            Hit::DeletePreset(id) => self.delete_preset(id),
            Hit::Quit => {
                let _ = self.proxy.send_event(crate::UserEvent::Quit);
            }
            Hit::Pref(p) => self.set_pref(p),
            Hit::CloseDialog => self.shell.dialog = Dialog::None,
            Hit::Panel => {}
        }
        self.redraw();
    }

    fn open_export_folder(&self) {
        if let Some(dir) = &self.export_dir
            && let Err(e) = std::process::Command::new("xdg-open").arg(dir).spawn()
        {
            eprintln!("neuralcanvas: could not open {}: {e}", dir.display());
        }
    }

    fn set_pref(&mut self, pref: Pref) {
        match pref {
            Pref::Theme(t) => {
                self.settings.theme = t;
                self.forget_thumbnails();
            }
            Pref::Grid(on) => self.settings.grid = on,
            Pref::Smoothing(on) => self.settings.smoothing = on,
            Pref::Pressure(c) => self.settings.pressure = c,
        }
        self.settings_writer.save(self.settings);
    }

    /// Picking an ink colours new ink, and the selection if there is one.
    fn set_ink(&mut self, i: usize) {
        self.ink = i;
        self.restyle("Change Colour", |kind| match kind {
            ElementKind::Stroke(s) => s.color = INKS[i],
            ElementKind::Shape(s) => s.color = INKS[i],
            ElementKind::Recognized(_) => {}
        });
    }

    fn set_width(&mut self, i: usize) {
        self.width = i;
        self.restyle("Change Width", |kind| match kind {
            ElementKind::Stroke(s) => s.width = WIDTHS[i],
            ElementKind::Shape(s) => s.width = WIDTHS[i],
            ElementKind::Recognized(_) => {}
        });
    }

    /// Applies `change` to every selected stroke and shape, as one undo step.
    fn restyle(&mut self, label: &'static str, change: impl Fn(&mut ElementKind)) {
        if self.selection.is_empty() || self.read_only {
            return;
        }
        let doc = self.session.doc();
        let edits: Vec<Command> = self
            .selection
            .iter()
            .filter_map(|id| doc.get(*id))
            .map(|e| {
                let mut kind = e.kind.clone();
                change(&mut kind);
                Command::Replace {
                    element: Arc::new(Element { id: e.id, kind }),
                }
            })
            .collect();
        self.execute(label, Command::Batch(edits));
    }

    fn field_text(&self, field: Field) -> String {
        let num = |v: f64| format!("{v:.0}");
        match field {
            Field::SketchName(id) => self
                .projects
                .iter()
                .flat_map(|p| &p.sketches)
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_default(),
            Field::ProjectName(id) => self
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            Field::Label => self
                .selected_wire()
                .map(|s| s.label.clone())
                .unwrap_or_default(),
            Field::StrokeWidth => match self.inspect() {
                Inspect::Elements { width: Some(w), .. } => format!("{w}"),
                _ => String::new(),
            },
            Field::X | Field::Y | Field::W | Field::H => {
                let b = match self.inspect() {
                    Inspect::Elements { bounds, .. } | Inspect::Wire { bounds, .. } => bounds,
                    Inspect::Nothing => return String::new(),
                };
                num(match field {
                    Field::X => b.x0,
                    Field::Y => b.y0,
                    Field::W => b.width(),
                    _ => b.height(),
                })
            }
        }
    }

    /// The new bounds after setting one of X, Y, W, H; `None` if the value is not a usable number.
    fn edited_bounds(b: Rect, field: Field, text: &str) -> Option<Rect> {
        let v: f64 = text.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
        Some(match field {
            Field::X => Rect::new(v, b.y0, v + b.width(), b.y1),
            Field::Y => Rect::new(b.x0, v, b.x1, v + b.height()),
            Field::W if v >= 1.0 => Rect::new(b.x0, b.y0, b.x0 + v, b.y1),
            Field::H if v >= 1.0 => Rect::new(b.x0, b.y0, b.x1, b.y0 + v),
            _ => return None,
        })
    }

    pub(super) fn commit_field(&mut self, field: Field, text: String) {
        match field {
            Field::SketchName(id) => self.rename(id, false, text),
            Field::ProjectName(id) => self.rename(id, true, text),
            Field::Label => self.edit_wire(|s| s.label = text.clone()),
            Field::StrokeWidth => {
                if let Some(w) = text
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|w| (0.5..=64.0).contains(w))
                {
                    self.restyle("Change Width", |kind| match kind {
                        ElementKind::Stroke(s) => s.width = w,
                        ElementKind::Shape(s) => s.width = w,
                        ElementKind::Recognized(_) => {}
                    });
                }
            }
            Field::X | Field::Y | Field::W | Field::H => {
                if self.selected_wire().is_some() {
                    self.edit_wire(|s| {
                        if let Some(b) = Self::edited_bounds(s.bounds, field, &text) {
                            s.bounds = b;
                        }
                    });
                } else if let Some(old) = select::bounds(self.session.doc(), &self.selection)
                    && let Some(new) = Self::edited_bounds(old, field, &text)
                {
                    let originals = select::selected(self.session.doc(), &self.selection);
                    let t: Affine = fit_affine(old, new);
                    let label = if matches!(field, Field::X | Field::Y) {
                        "Move"
                    } else {
                        "Resize"
                    };
                    self.execute(label, select::transformed(&originals, t));
                }
            }
        }
        self.redraw();
    }

    /// Edits the selected wireframe element: the edit is stored as a document element (undoable,
    /// saved) that recognition then respects.
    fn edit_wire(&mut self, change: impl FnOnce(&mut Recognized)) {
        if self.read_only {
            return;
        }
        let Some(current) = self.selected_wire().cloned() else {
            return;
        };
        let mut edited = current.clone();
        change(&mut edited);
        if edited == current {
            return;
        }
        let kind = ElementKind::Recognized(edited.to_pinned());
        let command = match current.pinned {
            Some(id) => Command::Replace {
                element: Arc::new(Element { id, kind }),
            },
            None => Command::append(Element::new(kind)),
        };
        self.execute("Edit Wireframe", command);
    }

    /// Whether a select-tool click should pick wireframe elements rather than ink: when the wireframe
    /// is at least half shown, or side by side in its pane.
    pub(super) fn picks_wire(&self) -> bool {
        self.tools.active() == Tool::Select
            && self.wireframe.is_some()
            && (self.wire_level >= 0.5 || (self.split && self.pointer_in_right_pane))
    }
}
