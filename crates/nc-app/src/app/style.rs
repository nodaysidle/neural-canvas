//! The style mirror: screenshots are read on the style worker; the inspector reviews what was read
//! (with its WCAG warnings) before it is applied; the applied style is stored with the sketch and
//! presets in the store. Applying a style redraws the wireframe in it, filled.

use super::App;
use crate::UserEvent;
use crate::workers::Screenshot;
use nc_core::style::{Extracted, Style};
use nc_shell::copy;
use uuid::Uuid;

/// What was read off a screenshot and not applied yet.
pub(super) struct Pending {
    pub extracted: Extracted,
    /// The screenshot's name, for a preset saved from it.
    pub name: String,
}

impl App {
    pub(super) fn import_style(&mut self) {
        self.styler.read(Screenshot::Pick);
    }

    pub(super) fn paste_image(&mut self, bytes: Vec<u8>) {
        self.styler.read(Screenshot::Pasted(bytes));
    }

    pub(super) fn style_read(&mut self, result: Result<(Box<Extracted>, String), String>) {
        self.style_reading = false;
        match result {
            Ok((extracted, name)) => {
                self.style_pending = Some(Pending {
                    extracted: *extracted,
                    name,
                });
                // The review lives in the inspector, with nothing selected.
                self.selection.clear();
                self.wire_selected = None;
                if !self.shell.inspector {
                    self.run(nc_core::Action::ToggleInspector);
                }
            }
            Err(e) => self.toast_error(copy::style_failed(&e)),
        }
        self.redraw();
    }

    pub(super) fn apply_style(&mut self) {
        if let Some(p) = self.style_pending.take() {
            self.style_name = p.name;
            self.set_style(Some(p.extracted.style));
        }
    }

    pub(super) fn apply_preset(&mut self, id: Uuid) {
        if let Some(p) = self.presets.iter().find(|p| p.id == id) {
            self.style_name = p.name.clone();
            self.style_pending = None;
            self.set_style(Some(p.style));
        }
    }

    /// Draws the wireframe in `style` (plain when `None`), shows it if it was hidden, and stores the
    /// choice with the sketch.
    pub(super) fn set_style(&mut self, style: Option<Style>) {
        self.style = style;
        if style.is_some() && !self.wire_visible() {
            self.set_wire(1.0, false);
        }
        self.redraw_wireframe();
        let (sketch, proxy) = (self.session.sketch_id, self.proxy.clone());
        self.request(
            move |db| db.set_sketch_style(sketch, style.as_ref()),
            move |r| {
                if let Err(e) = r {
                    let _ = proxy.send_event(UserEvent::Failed(copy::not_saved(&e.to_string())));
                }
            },
        );
        self.redraw();
    }

    pub(super) fn save_preset(&mut self) {
        let Some(style) = self.style else { return };
        let (name, proxy) = (self.style_name.clone(), self.proxy.clone());
        self.request(
            move |db| {
                db.save_preset(&name, &style)?;
                db.presets()
            },
            move |r| {
                let _ = proxy.send_event(UserEvent::Presets(r.map_err(|e| e.to_string())));
            },
        );
        self.toast_info(copy::style_saved(&self.style_name), None);
    }

    pub(super) fn delete_preset(&mut self, id: Uuid) {
        let proxy = self.proxy.clone();
        self.request(
            move |db| {
                db.delete_preset(id)?;
                db.presets()
            },
            move |r| {
                let _ = proxy.send_event(UserEvent::Presets(r.map_err(|e| e.to_string())));
            },
        );
    }

    pub(super) fn refresh_presets(&self) {
        let proxy = self.proxy.clone();
        self.request(
            |db| db.presets(),
            move |r| {
                let _ = proxy.send_event(UserEvent::Presets(r.map_err(|e| e.to_string())));
            },
        );
    }

    pub(super) fn presets_loaded(&mut self, result: Result<Vec<nc_store::Preset>, String>) {
        match result {
            Ok(presets) => self.presets = presets,
            Err(e) => self.toast_error(copy::project_failed(&e)),
        }
        self.redraw();
    }

    /// Loads the open sketch's style from the store.
    pub(super) fn load_style(&self) {
        let (sketch, proxy) = (self.session.sketch_id, self.proxy.clone());
        self.request(
            move |db| db.sketch_style(sketch),
            move |r| {
                let _ = proxy.send_event(UserEvent::SketchStyle {
                    sketch,
                    result: r.map_err(|e| e.to_string()),
                });
            },
        );
    }

    pub(super) fn style_loaded(&mut self, sketch: Uuid, result: Result<Option<Style>, String>) {
        if sketch != self.session.sketch_id {
            return;
        }
        match result {
            Ok(style) => {
                self.style = style;
                self.redraw_wireframe();
            }
            Err(e) => self.toast_error(copy::sketch_failed(&e)),
        }
        self.redraw();
    }
}
