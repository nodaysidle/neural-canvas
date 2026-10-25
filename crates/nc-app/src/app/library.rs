//! Projects and sketches: the sidebar's list, switching sketches, creating and renaming them, and
//! thumbnails. Every store call goes through `Store::request`, so the UI thread never waits on disk;
//! results come back as user events.

use super::App;
use crate::UserEvent;
use nc_core::{Document, Session, Viewport};
use nc_shell::{copy, thumbnail_id};
use nc_store::Db;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

/// Thumbnail size in the sidebar, logical px.
const THUMB: [f32; 2] = [70.0, 44.0];
/// The open sketch's thumbnail follows edits at most this often.
const THUMB_EVERY: Duration = Duration::from_secs(1);

impl App {
    /// A store call off this thread; nothing happens while there is no store (its dialog is up).
    fn request<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> nc_store::Result<T> + Send + 'static,
        done: impl FnOnce(nc_store::Result<T>) + Send + 'static,
    ) {
        if let Some(store) = &self.store {
            store.request(f, done);
        }
    }

    /// Reloads the project list for the sidebar.
    pub(super) fn refresh_projects(&self) {
        let proxy = self.proxy.clone();
        self.request(
            |db| db.projects(),
            move |r| {
                let _ = proxy.send_event(UserEvent::Projects(r.map_err(|e| e.to_string())));
            },
        );
    }

    pub(super) fn projects_loaded(&mut self, result: Result<Vec<nc_store::ProjectInfo>, String>) {
        match result {
            Ok(projects) => self.projects = projects,
            Err(e) => self.toast_error(copy::project_failed(&e)),
        }
        self.redraw();
    }

    /// Switches to `sketch`: what is drawn so far is saved first (the store writes queued saves before
    /// it loads), then the new sketch opens with its own history (Mac bug #2).
    pub(super) fn open_sketch(&mut self, sketch: Uuid) {
        if sketch == self.session.sketch_id || self.loading {
            return;
        }
        if self.drag.is_some() {
            self.release();
        }
        self.session.end_gesture();
        self.persist();
        self.loading = true;
        let proxy = self.proxy.clone();
        self.request(
            move |db| db.load(sketch),
            move |r| {
                let _ = proxy.send_event(UserEvent::SketchOpened {
                    sketch,
                    result: r.map_err(|e| e.to_string()),
                });
            },
        );
        self.redraw();
    }

    pub(super) fn sketch_opened(&mut self, sketch: Uuid, result: Result<Document, String>) {
        self.loading = false;
        match result {
            Ok(doc) => {
                self.show(Session::new(sketch, doc));
                self.read_only = false;
            }
            Err(e) => self.toast_error(copy::sketch_failed(&e)),
        }
        self.redraw();
    }

    /// Makes `session` the open sketch, with a fresh selection, wireframe and view.
    fn show(&mut self, session: Session) {
        let fit = session.doc().bounds();
        self.session = session;
        self.selection.clear();
        self.lifted.clear();
        self.wire_selected = None;
        self.wireframe = None;
        self.wire_segments.clear();
        self.wire_serial += 1;
        self.requested = None;
        self.reviewing = None;
        self.animation = None;
        self.viewport = fit.map_or(Viewport::default(), |b| {
            Viewport::fit(b, self.pane_size(), super::FIT_PADDING)
        });
        self.refresh_projects();
    }

    /// Retries opening the store where it failed, on a worker (the UI thread never touches disk).
    pub(super) fn retry_store(&mut self) {
        let dir = self.store_dir.clone();
        self.open_store_on_worker(move || Some(Ok(dir)));
    }

    /// Asks the desktop for another folder, then opens the store there, on a worker.
    pub(super) fn choose_store_location(&mut self) {
        self.open_store_on_worker(|| {
            match crate::workers::pick_folder(copy::CHOOSE_LOCATION_TITLE) {
                Ok(Some(dir)) => Some(Ok(Some(dir))),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            }
        });
    }

    /// `pick` gives the folder to open in (`Ok(None)`: the default), `None` when cancelled.
    fn open_store_on_worker(
        &mut self,
        pick: impl FnOnce() -> Option<Result<Option<PathBuf>, String>> + Send + 'static,
    ) {
        if self.store.is_some() || self.store_busy {
            return;
        }
        self.store_busy = true;
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name("store-open".into())
            .spawn(move || {
                let (result, dir) = match pick() {
                    None => (None, None),
                    Some(Err(e)) => {
                        eprintln!("neuralcanvas: the folder chooser failed: {e}");
                        (Some(Err(e)), None)
                    }
                    Some(Ok(dir)) => {
                        let opened = crate::open(proxy.clone(), dir.as_deref());
                        if let Err(e) = &opened {
                            eprintln!("neuralcanvas: cannot open the sketch store: {e}");
                        }
                        (Some(opened.map(Box::new).map_err(|e| e.to_string())), dir)
                    }
                };
                let _ = proxy.send_event(UserEvent::StoreOpened { result, dir });
            })
            .expect("spawn store-open thread");
    }

    pub(super) fn store_opened(
        &mut self,
        result: Option<Result<Box<crate::Opened>, String>>,
        dir: Option<PathBuf>,
    ) {
        self.store_busy = false;
        match result {
            None => {}
            Some(Err(e)) => self.store_error = Some(e),
            Some(Ok(opened)) => {
                let (store, session, read_only) = *opened;
                if let Some(dir) = dir {
                    self.settings_writer.save_store_dir(dir.clone());
                    self.store_dir = Some(dir);
                }
                self.store = Some(store);
                self.store_error = None;
                self.shell.focus = None;
                self.show(session);
                self.read_only = read_only;
                self.update_title();
            }
        }
        self.redraw();
    }

    /// A new project with one empty sketch, opened.
    pub(super) fn new_project(&self) {
        let proxy = self.proxy.clone();
        self.request(
            |db| {
                let project = db.create_project(copy::DEFAULT_PROJECT)?;
                db.create_sketch(project, copy::DEFAULT_SKETCH)
            },
            move |r| {
                let _ = proxy.send_event(UserEvent::SketchCreated(r.map_err(|e| e.to_string())));
            },
        );
    }

    pub(super) fn new_sketch(&self, project: Uuid) {
        let proxy = self.proxy.clone();
        self.request(
            move |db| db.create_sketch(project, copy::DEFAULT_SKETCH),
            move |r| {
                let _ = proxy.send_event(UserEvent::SketchCreated(r.map_err(|e| e.to_string())));
            },
        );
    }

    pub(super) fn sketch_created(&mut self, result: Result<Uuid, String>) {
        match result {
            Ok(sketch) => {
                self.refresh_projects();
                self.open_sketch(sketch);
            }
            Err(e) => self.toast_error(copy::project_failed(&e)),
        }
    }

    /// Renames a sketch (`project` false) or a project, then reloads the list.
    pub(super) fn rename(&self, id: Uuid, project: bool, name: String) {
        if name.is_empty() {
            return;
        }
        let proxy = self.proxy.clone();
        self.request(
            move |db| {
                if project {
                    db.rename_project(id, &name)?;
                } else {
                    db.rename_sketch(id, &name)?;
                }
                db.projects()
            },
            move |r| {
                let _ = proxy.send_event(UserEvent::Projects(r.map_err(|e| e.to_string())));
            },
        );
    }

    /// Keeps sidebar thumbnails current while the sidebar is open: other sketches load once on the
    /// store thread; the open sketch is redrawn from memory at most once a second.
    pub(super) fn update_thumbnails(&mut self) {
        if !self.shell.sidebar {
            return;
        }
        let ids: Vec<Uuid> = self
            .projects
            .iter()
            .flat_map(|p| p.sketches.iter().map(|s| s.id))
            .collect();
        for id in ids {
            if id == self.session.sketch_id {
                let rev = self.session.doc().revision();
                let due = self.thumb_rendered.get(&id) != Some(&rev)
                    && self.thumb_at.is_none_or(|t| t.elapsed() >= THUMB_EVERY);
                let theme = self.palette().canvas;
                if due && let Some(g) = &mut self.gpu {
                    g.thumbnail(thumbnail_id(id), self.session.doc(), THUMB, theme);
                    self.thumb_rendered.insert(id, rev);
                    self.thumb_at = Some(std::time::Instant::now());
                }
            } else if !self.thumb_rendered.contains_key(&id) && self.thumb_pending.insert(id) {
                let proxy = self.proxy.clone();
                self.request(
                    move |db| db.load(id),
                    move |r| {
                        if let Ok(doc) = r {
                            let _ = proxy.send_event(UserEvent::Thumbnail { sketch: id, doc });
                        }
                    },
                );
            }
        }
    }

    pub(super) fn thumbnail_loaded(&mut self, sketch: Uuid, doc: Document) {
        self.thumb_pending.remove(&sketch);
        let theme = self.palette().canvas;
        if let Some(g) = &mut self.gpu {
            g.thumbnail(thumbnail_id(sketch), &doc, THUMB, theme);
            self.thumb_rendered.insert(sketch, doc.revision());
            self.redraw();
        }
    }

    /// Theme changes redraw every thumbnail.
    pub(super) fn forget_thumbnails(&mut self) {
        self.thumb_rendered.clear();
        self.thumb_at = None;
    }
}
