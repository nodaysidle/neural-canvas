//! The canvas controller: input (mouse, touchpad, tablet) → chrome or tools → commands on the open
//! sketch, the view, frame scheduling and autosave. This thread never touches disk and never blocks on
//! a worker. The chrome's side lives in `shell`, projects and sketches in `library`, the
//! accessibility tree in `access`, preferences in `settings`.

mod access;
mod library;
pub mod settings;
mod shell;

use crate::UserEvent;
use crate::chrome;
use crate::clipboard::Clipboard;
use crate::gpu::Gpu;
use crate::workers::{Exporter, Recognizer};
use glam::Vec2;
use nc_core::document::stroke_segments;
use nc_core::kurbo::Affine;
use nc_core::recognize::{Wireframe, draw};
use nc_core::select::{self, Gesture, Handle};
use nc_core::{
    Action, Command, Element, ElementId, ElementKind, FrameScheduler, Key as CoreKey, Predictor,
    Rect, Segment, Session, ShapeKind, Shortcut, Smoother, Stroke, Tool, Tools, ViewAnimation,
    Viewport, hit, shape,
};
use nc_render::{Blend, Lifted, LiveStroke, Scene, WireLayer};
use nc_shell::{INKS, Settings, WIDTHS};
use nc_store::{ProjectInfo, Store, StoreEvent};
use nc_wayland::{Presented, SideChannel, ToolFrame, ToolKind, mono_ns};
use std::collections::HashMap;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Cursor, CursorIcon, CustomCursor, WindowId};

/// Logical px kept clear around content by Zoom to Fit.
const FIT_PADDING: f32 = 48.0;
/// Eraser reach in logical px, the same at every zoom.
const ERASER_RADIUS: f32 = 8.0;
/// How close a click must be to ink to pick it, logical px.
const PICK_TOLERANCE: f32 = 4.0;
/// Where Duplicate puts the copy, logical px down and right.
const DUPLICATE_OFFSET: f32 = 16.0;
/// Room around an uncertain item when reviewing it, logical px, and the closest zoom it gets.
const REVIEW_PADDING: f32 = 120.0;
const REVIEW_MAX_ZOOM: f32 = 2.0;
/// Touchpad pan inertia: velocity halves about every 0.17 s, and stops below this speed (px/s).
const INERTIA_DECAY: f32 = 0.25;
const INERTIA_STOP: f32 = 20.0;
/// Refresh period until presentation feedback reports the real one.
const DEFAULT_REFRESH: f64 = 1.0 / 120.0;
/// Points reserved per live stroke, so the hot input path does not reallocate (a few seconds of 1 kHz
/// input; longer strokes grow once per doubling).
const LIVE_RESERVE: usize = 4_096;

enum Drag {
    Draw,
    Erase,
    Pan,
    Select(Gesture),
    /// A rectangle, ellipse or line from `start` (world units).
    Shape {
        kind: ShapeKind,
        start: Vec2,
    },
}

/// A presented frame we asked feedback for.
struct InFlight {
    id: u64,
    sample_ns: u64,
    /// Sample to present, ns.
    work_ns: u64,
    on_schedule: bool,
}

pub struct App {
    gpu: Option<Gpu>,
    wl: Option<SideChannel>,
    /// `None` while the store can't be opened: a blocking dialog is up and nothing can be drawn.
    store: Option<Store>,
    /// Why the store can't be opened, for that dialog.
    store_error: Option<String>,
    /// The folder the store lives in when not the default one (chosen in that dialog).
    store_dir: Option<PathBuf>,
    /// A retry or folder choice is running on a worker.
    store_busy: bool,
    session: Session,
    /// The sketch could not be read: show what loaded, accept no edits, never save over it.
    read_only: bool,
    save_error: Option<String>,
    /// A failed save on close keeps the window open once; closing again quits anyway.
    close_blocked: bool,
    reduced_motion: bool,

    viewport: Viewport,
    animation: Option<ViewAnimation>,
    tools: Tools,
    drag: Option<Drag>,
    live: Option<Stroke>,
    live_serial: u64,
    smoother: Smoother,
    predictor: Predictor,
    /// Seconds per refresh, from presentation feedback: how far ahead ink is predicted.
    refresh: f64,
    selection: Vec<ElementId>,
    /// The selection while a move or resize drags it: drawn through the drag's transform by the GPU,
    /// edited in the document once, on release.
    lifted: HashSet<ElementId>,
    hover_handle: Option<Handle>,
    /// Chrome for the frame being drawn, and ink on top (predicted ink, a shape being dragged);
    /// reused so frames do not allocate.
    overlay: Vec<Segment>,
    ink_overlay: Vec<Segment>,
    clipboard: Clipboard,
    recognizer: Recognizer,
    exporter: Exporter,
    /// 0 = raw sketch only, 0.5 = both, 1 = wireframe only.
    wire_level: f32,
    /// Raw sketch and wireframe side by side.
    split: bool,
    /// The newest wireframe for the current revision, and its capsules (drawn for the light canvas
    /// when `wire_light`).
    wireframe: Option<Arc<Wireframe>>,
    wire_light: bool,
    wire_segments: Vec<Segment>,
    wire_serial: u64,
    /// Revision last sent for recognition.
    requested: Option<u64>,
    /// "Review uncertain items": how many were visited, and the one shown.
    reviewed: usize,
    reviewing: Option<Rect>,
    eraser_cursor: Option<CustomCursor>,
    /// Logical px.
    cursor: Vec2,
    last_erase: Vec2,
    modifiers: ModifiersState,
    /// A tablet tool is over the window: its own events drive input; emulated pointer events are ignored.
    tablet_near: bool,
    /// Reused each frame so tablet input does not allocate.
    tablet_frames: Vec<ToolFrame>,

    epoch: Instant,
    scheduler: FrameScheduler,
    draw_at: Option<u64>,
    next_frame: u64,
    in_flight: VecDeque<InFlight>,

    proxy: EventLoopProxy<UserEvent>,
    shell: nc_shell::Shell,
    settings: Settings,
    settings_writer: settings::SettingsWriter,
    /// The desktop's colour scheme (portal), used when Preferences say "Match system".
    system_dark: bool,
    /// Palette choices for new ink.
    ink: usize,
    width: usize,
    projects: Vec<ProjectInfo>,
    /// A sketch is being opened: input waits for it.
    loading: bool,
    /// Newest revision of the open sketch the store has written.
    saved_revision: u64,
    /// The wireframe element the inspector edits, by its sources.
    wire_selected: Option<Vec<ElementId>>,
    /// Side by side: the pointer is over the wireframe pane.
    pointer_in_right_pane: bool,
    /// Pointer in window px (the chrome's space; `cursor` is the canvas pane's).
    window_cursor: Vec2,
    /// The pointer is over chrome: it gets the default cursor.
    over_chrome: bool,
    thumb_rendered: HashMap<Uuid, u64>,
    thumb_pending: HashSet<Uuid>,
    thumb_at: Option<Instant>,
    export_dir: Option<PathBuf>,
    /// The "Exporting…" toast, while an export runs.
    exporting: Option<u64>,
    access: Option<accesskit_winit::Adapter>,
    /// Touchpad pan velocity, px/s: estimated while panning, coasting after the fingers lift.
    pan_velocity: Vec2,
    last_pan: Option<f64>,
    inertia: Option<f64>,
}

impl App {
    pub fn new(
        opened: Result<crate::Opened, String>,
        proxy: EventLoopProxy<UserEvent>,
        settings: Settings,
        store_dir: Option<PathBuf>,
    ) -> Self {
        // Without a store the app opens on an empty, read-only canvas under the store dialog.
        let (store, session, read_only, store_error) = match opened {
            Ok((store, session, read_only)) => (Some(store), session, read_only, None),
            Err(e) => (
                None,
                Session::new(Uuid::nil(), nc_core::Document::default()),
                true,
                Some(e),
            ),
        };
        let saved_revision = session.doc().revision();
        let mut shell = nc_shell::Shell::new();
        shell.sidebar = settings.sidebar;
        shell.inspector = settings.inspector;
        Self {
            gpu: None,
            wl: None,
            store,
            store_error,
            store_dir: store_dir.clone(),
            store_busy: false,
            session,
            read_only,
            save_error: None,
            close_blocked: false,
            reduced_motion: false,
            viewport: Viewport::default(),
            animation: None,
            tools: Tools::default(),
            drag: None,
            live: None,
            live_serial: 0,
            smoother: Smoother::default(),
            predictor: Predictor::default(),
            refresh: DEFAULT_REFRESH,
            selection: Vec::new(),
            lifted: HashSet::new(),
            hover_handle: None,
            overlay: Vec::new(),
            ink_overlay: Vec::new(),
            clipboard: Clipboard::new(proxy.clone()),
            recognizer: Recognizer::new(proxy.clone()),
            exporter: Exporter::new(proxy.clone()),
            wire_level: 0.0,
            split: false,
            wireframe: None,
            wire_light: false,
            wire_segments: Vec::new(),
            wire_serial: 0,
            requested: None,
            reviewed: 0,
            reviewing: None,
            eraser_cursor: None,
            cursor: Vec2::ZERO,
            last_erase: Vec2::ZERO,
            modifiers: ModifiersState::default(),
            tablet_near: false,
            tablet_frames: Vec::new(),
            epoch: Instant::now(),
            scheduler: FrameScheduler::default(),
            draw_at: None,
            next_frame: 0,
            in_flight: VecDeque::new(),
            proxy,
            shell,
            settings,
            settings_writer: settings::SettingsWriter::new(settings, store_dir),
            system_dark: true,
            ink: 0,
            width: 1,
            projects: Vec::new(),
            loading: false,
            saved_revision,
            wire_selected: None,
            pointer_in_right_pane: false,
            window_cursor: Vec2::ZERO,
            over_chrome: false,
            thumb_rendered: HashMap::new(),
            thumb_pending: HashSet::new(),
            thumb_at: None,
            export_dir: None,
            exporting: None,
            access: None,
            pan_velocity: Vec2::ZERO,
            last_pan: None,
            inertia: None,
        }
    }

    /// The canvas background as RGBA, for exports and the clipboard (they look like the canvas).
    fn export_look(&self) -> ([f32; 4], bool) {
        let theme = self.palette().canvas;
        (theme.background_rgba(), theme.light)
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn redraw(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    fn set_cursor(&self) {
        let Some(g) = &self.gpu else { return };
        let resize = |h: Handle| match h {
            Handle::NorthWest | Handle::SouthEast => CursorIcon::NwseResize,
            Handle::NorthEast | Handle::SouthWest => CursorIcon::NeswResize,
            Handle::North | Handle::South => CursorIcon::NsResize,
            Handle::East | Handle::West => CursorIcon::EwResize,
        };
        let cursor: Cursor = match (&self.drag, self.tools.active()) {
            (None, _) if self.over_chrome => CursorIcon::Default.into(),
            (Some(Drag::Pan), _) => CursorIcon::Grabbing.into(),
            (Some(Drag::Select(Gesture::Resize { handle, .. })), _) => resize(*handle).into(),
            (Some(Drag::Select(Gesture::Move { .. })), _) => CursorIcon::Move.into(),
            (_, Tool::Hand) => CursorIcon::Grab.into(),
            (_, Tool::Eraser) => match &self.eraser_cursor {
                Some(ring) => ring.clone().into(),
                None => CursorIcon::Cell.into(),
            },
            (_, Tool::Select) => self.hover_handle.map_or(CursorIcon::Default, resize).into(),
            (_, Tool::Pen | Tool::Shape(_)) => CursorIcon::Crosshair.into(),
        };
        g.window.set_cursor(cursor);
    }

    fn wire_visible(&self) -> bool {
        self.split || self.wire_level > 0.0
    }

    /// Raw ink fades out as the wireframe fades in; both are fully shown at the halfway point.
    fn blend(&self) -> Blend {
        Blend {
            raw: (2.0 * (1.0 - self.wire_level)).min(1.0),
            wire: (2.0 * self.wire_level).min(1.0),
        }
    }

    fn set_wire(&mut self, level: f32, split: bool) {
        self.wire_level = level.clamp(0.0, 1.0);
        self.split = split;
        if !self.wire_visible() {
            self.wire_selected = None;
        }
        self.redraw();
    }

    /// Pointer px in the pane under it: side by side, the right pane shows the same view as the left.
    fn to_pane(&self, p: Vec2) -> Vec2 {
        let half = self.logical_size().x / 2.0;
        if self.split && p.x >= half {
            Vec2::new(p.x - half, p.y)
        } else {
            p
        }
    }

    /// The size of one pane: half the window when side by side.
    fn pane_size(&self) -> Vec2 {
        let size = self.logical_size();
        if self.split {
            Vec2::new(size.x / 2.0, size.y)
        } else {
            size
        }
    }

    fn adopt_wireframe(&mut self, sketch: Uuid, revision: u64, wire: Arc<Wireframe>) {
        // Stale: the sketch changed since. The newer request is already queued.
        if sketch != self.session.sketch_id || revision != self.session.doc().revision() {
            return;
        }
        self.wireframe = Some(wire);
        self.redraw_wireframe();
        self.redraw();
    }

    /// Rebuilds the wireframe's capsules in the current theme's colours.
    fn redraw_wireframe(&mut self) {
        self.wire_light = !self.dark();
        self.wire_segments.clear();
        if let Some(wire) = &self.wireframe {
            draw::segments(wire, self.wire_light, &mut self.wire_segments);
        }
        self.wire_serial += 1;
    }

    /// Zooms to the next element the quality gate flagged, in reading order.
    fn review_uncertain(&mut self) {
        // Reviewing means comparing what was drawn with what was recognised: show both.
        if !self.wire_visible() || self.wire_level > 0.5 {
            self.set_wire(0.5, self.split);
        }
        let Some(wire) = &self.wireframe else { return };
        let mut uncertain: Vec<Rect> = wire
            .shapes
            .iter()
            .filter(|s| s.is_uncertain())
            .map(|s| s.bounds)
            .collect();
        if uncertain.is_empty() {
            self.reviewing = None;
            self.toast_info(nc_shell::copy::NOTHING_TO_REVIEW.into(), None);
            return;
        }
        uncertain.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        let target = uncertain[self.reviewed % uncertain.len()];
        self.reviewed += 1;
        self.reviewing = Some(target);
        let mut view = Viewport::fit(target, self.pane_size(), REVIEW_PADDING);
        if view.scale > REVIEW_MAX_ZOOM {
            view.zoom_at(self.pane_size() * 0.5, REVIEW_MAX_ZOOM / view.scale);
        }
        let n = uncertain.len();
        self.toast_info(
            nc_shell::copy::reviewing((self.reviewed - 1) % n + 1, n),
            None,
        );
        self.animate_to(view);
    }

    fn world(&self, screen: Vec2) -> Vec2 {
        self.viewport.screen_to_world(screen)
    }

    /// The selection's bounds in logical screen px, if anything is selected.
    fn selection_screen_bounds(&self) -> Option<Rect> {
        let b = select::bounds(self.session.doc(), &self.selection)?;
        let a = self
            .viewport
            .world_to_screen(Vec2::new(b.x0 as f32, b.y0 as f32));
        let c = self
            .viewport
            .world_to_screen(Vec2::new(b.x1 as f32, b.y1 as f32));
        Some(Rect::new(a.x as f64, a.y as f64, c.x as f64, c.y as f64))
    }

    fn choose_tool(&mut self, tool: Tool) {
        self.tools.choose(tool);
        if !matches!(tool, Tool::Select | Tool::Hand) {
            self.selection.clear();
            self.wire_selected = None;
        }
        self.hover_handle = None;
        self.set_cursor();
        self.redraw();
    }

    fn logical_size(&self) -> Vec2 {
        self.gpu.as_ref().map_or(Vec2::new(1280.0, 800.0), |g| {
            let scale = g.window.scale_factor() as f32;
            Vec2::new(g.config.width as f32, g.config.height as f32) / scale
        })
    }

    /// Queues the current document for the store thread.
    fn persist(&self) {
        if self.read_only {
            return;
        }
        let Some(store) = &self.store else { return };
        let doc = self.session.doc();
        store.save(
            self.session.sketch_id,
            doc.revision(),
            doc.elements().to_vec(),
        );
    }

    fn execute(&mut self, label: &'static str, command: Command) {
        match self.session.execute(label, command) {
            Ok(()) => self.persist(),
            Err(e) => eprintln!("neuralcanvas: edit rejected: {e}"),
        }
        self.redraw();
    }

    /// Any direct manipulation of the view stops a running view animation or glide.
    fn take_view(&mut self) {
        self.animation = None;
        self.inertia = None;
    }

    // ---- input, shared by mouse and tablet. Positions are logical px. ----

    fn press(&mut self, pos: Vec2, pressure: Option<f32>) {
        self.cursor = pos;
        self.take_view();
        if self.loading {
            return;
        }
        match self.tools.active() {
            Tool::Hand => self.drag = Some(Drag::Pan),
            Tool::Select if self.picks_wire() => {
                // The wireframe is showing: clicks pick its elements for the inspector.
                let world = self.world(pos);
                self.pick_wire(world);
                self.selection.clear();
                self.redraw();
            }
            _ if self.read_only => {}
            Tool::Select => {
                self.wire_selected = None;
                let handle = self
                    .selection_screen_bounds()
                    .and_then(|b| select::handle_at(b, pos));
                let world = self.world(pos);
                self.drag = select::begin(
                    self.session.doc(),
                    &mut self.selection,
                    world,
                    handle,
                    self.modifiers.shift_key(),
                    self.modifiers.alt_key(),
                    PICK_TOLERANCE / self.viewport.scale,
                )
                .map(Drag::Select);
                if let Some(Drag::Select(g)) = &self.drag {
                    self.lifted = g.moved().iter().map(|e| e.id).collect();
                }
                self.redraw();
            }
            Tool::Shape(kind) => {
                self.drag = Some(Drag::Shape {
                    kind,
                    start: self.world(pos),
                });
                self.redraw();
            }
            Tool::Pen => {
                self.smoother.reset();
                self.predictor.reset();
                let now = self.now();
                let p = self.smooth(pos, now);
                self.predictor.push(now, p);
                let world = self.viewport.screen_to_world(p);
                let (width, ink) = (WIDTHS[self.width], INKS[self.ink]);
                let mut stroke = match pressure {
                    Some(pr) => Stroke::with_pressure(world, pr, width, ink),
                    None => Stroke::new(world, width, ink),
                };
                stroke.points.reserve(LIVE_RESERVE);
                if pressure.is_some() {
                    stroke.pressure.reserve(LIVE_RESERVE);
                }
                self.live = Some(stroke);
                self.live_serial += 1;
                self.drag = Some(Drag::Draw);
                self.redraw();
            }
            Tool::Eraser => {
                self.drag = Some(Drag::Erase);
                self.last_erase = self.viewport.screen_to_world(pos);
                self.erase_to(pos);
            }
        }
        self.set_cursor();
    }

    fn motion(&mut self, pos: Vec2, pressure: Option<f32>) {
        let delta = pos - self.cursor;
        self.cursor = pos;
        let world = self.world(pos);
        match &mut self.drag {
            Some(Drag::Pan) => {
                self.viewport.pan_by_screen(delta);
                self.redraw();
            }
            Some(Drag::Draw) => {
                let now = self.now();
                let p = self.smooth(pos, now);
                self.predictor.push(now, p);
                let world = self.viewport.screen_to_world(p);
                if let Some(stroke) = &mut self.live
                    && stroke.push_sample(world, pressure)
                {
                    self.redraw();
                }
            }
            Some(Drag::Erase) => self.erase_to(pos),
            Some(Drag::Select(g)) => {
                g.update(world);
                self.redraw();
            }
            Some(Drag::Shape { .. }) => self.redraw(),
            None if self.tools.active() == Tool::Select => {
                let hover = self
                    .selection_screen_bounds()
                    .and_then(|b| select::handle_at(b, pos));
                if hover != self.hover_handle {
                    self.hover_handle = hover;
                    self.set_cursor();
                }
            }
            None => {}
        }
    }

    fn release(&mut self) {
        match self.drag.take() {
            Some(Drag::Draw) => self.finish_stroke(),
            Some(Drag::Erase) => {
                self.session.end_gesture();
                self.persist();
            }
            Some(Drag::Select(g)) => {
                let label = match g {
                    Gesture::Resize { .. } => "Resize",
                    _ => "Move",
                };
                let edit = g.finish(
                    self.session.doc(),
                    &mut self.selection,
                    self.modifiers.shift_key(),
                );
                self.lifted.clear();
                if let Some(edit) = edit {
                    self.execute(label, edit);
                }
                self.redraw();
            }
            Some(Drag::Shape { kind, start }) => {
                let shape = self.shape_to(kind, start);
                if shape.from != shape.to {
                    let label = match kind {
                        ShapeKind::Rectangle => "Draw Rectangle",
                        ShapeKind::Ellipse => "Draw Ellipse",
                        ShapeKind::Line => "Draw Line",
                    };
                    self.execute(
                        label,
                        Command::append(Element::new(ElementKind::Shape(shape))),
                    );
                }
                self.redraw();
            }
            Some(Drag::Pan) | None => {}
        }
        self.set_cursor();
    }

    /// A pen sample through the 1€ filter, unless smoothing is off in Preferences.
    fn smooth(&mut self, pos: Vec2, now: f64) -> Vec2 {
        if self.settings.smoothing {
            self.smoother.smooth(pos, now)
        } else {
            pos
        }
    }

    /// The shape a drag from `start` to the pointer makes, with Shift (constrain) and Alt (from centre).
    fn shape_to(&self, kind: ShapeKind, start: Vec2) -> nc_core::Shape {
        shape::from_drag(
            kind,
            start,
            self.world(self.cursor),
            self.modifiers.shift_key(),
            self.modifiers.alt_key(),
            WIDTHS[self.width],
            INKS[self.ink],
        )
    }

    /// Ends the stroke exactly where the pen lifted (the smoothed tail may trail by a pixel or two) and
    /// commits it as one "Draw".
    fn finish_stroke(&mut self) {
        self.drag = None;
        let Some(mut stroke) = self.live.take() else {
            return;
        };
        stroke.push(self.viewport.screen_to_world(self.cursor));
        self.execute("Draw", Command::append(Element::stroke(stroke)));
    }

    /// Cuts away the ink the eraser disc swept over since the last sample.
    fn erase_to(&mut self, pos: Vec2) {
        let to = self.viewport.screen_to_world(pos);
        let radius = ERASER_RADIUS / self.viewport.scale;
        let from = std::mem::replace(&mut self.last_erase, to);
        if let Some(edit) = hit::erase(self.session.doc(), from, to, radius) {
            if let Err(e) = self.session.apply_in_gesture("Erase", edit) {
                eprintln!("neuralcanvas: erase rejected: {e}");
            }
            self.redraw();
        }
    }

    fn handle_tablet(&mut self) {
        let Some(wl) = &mut self.wl else { return };
        // Taken out so input handling can borrow `self`; the buffer keeps its capacity.
        let mut frames = std::mem::take(&mut self.tablet_frames);
        wl.drain_tool_frames(|f| frames.push(f));
        for f in frames.drain(..) {
            let pos = self.to_pane(Vec2::new(f.position.0 as f32, f.position.1 as f32));
            let curve = self.settings.pressure;
            let f = ToolFrame {
                pressure: f.pressure.map(|p| curve.apply(p)),
                ..f
            };
            if f.entered {
                self.tablet_near = true;
                self.tools.set_stylus_eraser(f.kind == ToolKind::Eraser);
                self.cursor = pos;
                self.set_cursor();
            }
            if f.moved {
                self.motion(pos, f.pressure);
            }
            if f.down {
                self.press(pos, f.pressure);
            }
            if f.up {
                self.release();
            }
            if f.left {
                if self.drag.is_some() {
                    self.release();
                }
                self.tablet_near = false;
                self.tools.set_stylus_eraser(false);
                self.set_cursor();
            }
        }
        self.tablet_frames = frames;
    }

    fn run(&mut self, action: Action) {
        if !action.enabled(self.session.history(), !self.selection.is_empty())
            || (self.read_only && action.edits())
        {
            return;
        }
        let doc = self.session.doc();
        match action {
            Action::Undo | Action::Redo => {
                // Whatever the pointer is doing ends first, as its own step.
                if self.drag.is_some() {
                    self.release();
                }
                let result = if action == Action::Undo {
                    self.session.undo()
                } else {
                    self.session.redo()
                };
                match result {
                    Ok(true) => self.persist(),
                    Ok(false) => {}
                    Err(e) => eprintln!("neuralcanvas: {action:?} failed: {e}"),
                }
                let doc = self.session.doc();
                self.selection.retain(|id| doc.index_of(*id).is_some());
            }
            Action::SelectAll => {
                self.selection = doc.elements().iter().map(|e| e.id).collect();
                self.choose_tool(Tool::Select);
            }
            Action::Deselect => {
                self.selection.clear();
                self.reviewing = None;
                self.wire_selected = None;
            }
            Action::Delete => {
                let edit = select::delete(&self.selection);
                self.selection.clear();
                self.execute("Delete", edit);
            }
            Action::Duplicate => {
                let offset = Vec2::splat(DUPLICATE_OFFSET / self.viewport.scale);
                let (edit, copies) = select::duplicate(doc, &self.selection, offset);
                self.execute("Duplicate", edit);
                self.selection = copies;
            }
            Action::Copy => {
                let (background, light) = self.export_look();
                self.clipboard
                    .copy(select::selected(doc, &self.selection), background, light);
            }
            Action::Paste => self.clipboard.paste(self.world(self.cursor)),
            Action::Nudge { dx, dy } => {
                let edit =
                    select::translated(doc, &self.selection, Vec2::new(dx as f32, dy as f32));
                self.execute("Nudge", edit);
            }
            Action::FitToContent => {
                let target = self
                    .session
                    .doc()
                    .bounds()
                    .map_or(Viewport::default(), |b| {
                        Viewport::fit(b, self.pane_size(), FIT_PADDING)
                    });
                self.animate_to(target);
            }
            Action::ActualSize => {
                let mut target = self.viewport;
                target.zoom_at(self.pane_size() * 0.5, 1.0 / target.scale);
                self.animate_to(target);
            }
            Action::CycleWireframe => {
                let next = match self.wire_level {
                    l if l <= 0.0 => 0.5,
                    l if l < 1.0 => 1.0,
                    _ => 0.0,
                };
                self.set_wire(next, false);
            }
            Action::WireframeOpacity { up } => {
                let step = if up { 0.1 } else { -0.1 };
                self.set_wire(((self.wire_level + step) * 10.0).round() / 10.0, false);
            }
            Action::SideBySide => self.set_wire(self.wire_level, !self.split),
            Action::ReviewUncertain => self.review_uncertain(),
            Action::Export { wireframe } => {
                if doc.elements().is_empty() {
                    self.toast_info(nc_shell::copy::NOTHING_TO_EXPORT.into(), None);
                } else {
                    let (background, light) = self.export_look();
                    let name = self.sketch_name().to_owned();
                    self.exporter.export(
                        doc.elements().to_vec(),
                        &name,
                        wireframe,
                        background,
                        light,
                    );
                    let now = self.now();
                    self.exporting = Some(self.shell.toast(
                        nc_shell::ToastKind::Progress,
                        nc_shell::copy::EXPORTING,
                        None,
                        now,
                    ));
                }
            }
            Action::ZoomToSelection => {
                if let Some(b) = select::bounds(doc, &self.selection) {
                    self.animate_to(Viewport::fit(b, self.pane_size(), FIT_PADDING));
                }
            }
            Action::ZoomIn | Action::ZoomOut => {
                let factor = if action == Action::ZoomIn { 1.25 } else { 0.8 };
                let mut target = self.viewport;
                target.zoom_at(self.pane_size() * 0.5, factor);
                self.animate_to(target);
            }
            Action::ToggleSidebar | Action::ToggleInspector => {
                if action == Action::ToggleSidebar {
                    self.settings.sidebar = !self.settings.sidebar;
                } else {
                    self.settings.inspector = !self.settings.inspector;
                }
                self.shell.sidebar = self.settings.sidebar;
                self.shell.inspector = self.settings.inspector;
                self.settings_writer.save(self.settings);
            }
            Action::Preferences => self.shell.dialog = nc_shell::Dialog::Preferences,
            Action::Shortcuts => {
                self.shell.dialog = if self.shell.dialog == nc_shell::Dialog::Shortcuts {
                    nc_shell::Dialog::None
                } else {
                    nc_shell::Dialog::Shortcuts
                }
            }
        }
        self.redraw();
    }

    fn pasted(&mut self, elements: Vec<Element>, centre: Vec2) {
        if self.read_only {
            return;
        }
        let (edit, ids) = select::paste(elements, centre);
        self.execute("Paste", edit);
        self.selection = ids;
        self.choose_tool(Tool::Select);
    }

    /// Fills `self.overlay` for this frame: predicted ink, shape preview, selection chrome. Returns
    /// whether it holds predicted ink.
    fn build_overlay(&mut self) -> bool {
        let mut overlay = std::mem::take(&mut self.overlay);
        overlay.clear();
        let mut ink = std::mem::take(&mut self.ink_overlay);
        ink.clear();
        let view = self.viewport;
        let dark = self.dark();
        let mut predicted = false;
        match &self.drag {
            Some(Drag::Draw) => {
                if let Some(stroke) = &self.live
                    && let Some(ahead) = self.predictor.predict(self.now(), self.refresh)
                {
                    let last = stroke.points.len() - 1;
                    ink.push(Segment {
                        a: stroke.points[last],
                        b: view.screen_to_world(ahead),
                        half_width: stroke.half_width(last),
                        color: stroke.color,
                    });
                    predicted = true;
                }
            }
            Some(Drag::Shape { kind, start }) => {
                let preview = self.shape_to(*kind, *start).to_stroke();
                ink.extend(stroke_segments(&preview));
            }
            Some(Drag::Select(Gesture::Marquee { start, current })) => {
                let r = Rect::from_points(
                    (start.x as f64, start.y as f64),
                    (current.x as f64, current.y as f64),
                );
                chrome::rect(&mut overlay, r, &view, dark);
            }
            Some(Drag::Select(Gesture::Lasso(points))) => {
                chrome::polyline(&mut overlay, points, true, &view, dark)
            }
            _ => {}
        }
        if self.tools.active() == Tool::Select
            && let Some(b) = select::bounds(self.session.doc(), &self.selection)
        {
            let b = self
                .lift_transform()
                .map_or(b, |t| t.transform_rect_bbox(b));
            chrome::selection(&mut overlay, b, &view, dark);
        }
        if let (true, Some(r)) = (self.wire_visible(), self.reviewing) {
            chrome::highlight(&mut overlay, r, &view, dark);
        }
        if let Some(s) = self.selected_wire() {
            chrome::rect(&mut overlay, s.bounds.inflate(3.0, 3.0), &view, dark);
        }
        if self.split {
            chrome::pane_divider(&mut overlay, self.pane_size(), &view, dark);
        }
        self.overlay = overlay;
        self.ink_overlay = ink;
        predicted
    }

    fn lift_transform(&self) -> Option<Affine> {
        match &self.drag {
            Some(Drag::Select(g)) => g.transform(),
            _ => None,
        }
    }

    /// Touchpad panning: estimates the hand's speed, and lets the view glide on when the fingers lift.
    fn track_pan(&mut self, delta: Vec2, phase: TouchPhase) {
        let now = self.now();
        match phase {
            TouchPhase::Started => {
                self.pan_velocity = Vec2::ZERO;
                self.last_pan = Some(now);
            }
            TouchPhase::Moved => {
                if let Some(last) = self.last_pan {
                    let dt = (now - last).max(1e-3) as f32;
                    // A short running average: the last few events decide the glide.
                    self.pan_velocity = self.pan_velocity * 0.6 + (delta / dt) * 0.4;
                }
                self.last_pan = Some(now);
            }
            TouchPhase::Ended => {
                let fresh = self.last_pan.is_some_and(|l| now - l < 0.05);
                if fresh && !self.reduced_motion && self.pan_velocity.length() > 2.0 * INERTIA_STOP
                {
                    self.inertia = Some(now);
                    self.redraw();
                }
                self.last_pan = None;
            }
            TouchPhase::Cancelled => self.last_pan = None,
        }
    }

    fn animate_to(&mut self, target: Viewport) {
        self.animation = ViewAnimation::new(
            self.viewport,
            target,
            self.now(),
            self.pane_size(),
            self.reduced_motion,
        );
        if self.animation.is_none() {
            self.viewport = target;
        }
    }

    // ---- frames ----

    /// Reads presentation feedback into the scheduler.
    fn handle_presented(&mut self) {
        let Some(wl) = &mut self.wl else { return };
        let (in_flight, scheduler, refresh) =
            (&mut self.in_flight, &mut self.scheduler, &mut self.refresh);
        wl.drain_presented(|id, presented| {
            while in_flight.front().is_some_and(|f| f.id < id) {
                in_flight.pop_front();
            }
            if let Some(f) = in_flight.pop_front().filter(|f| f.id == id)
                && let Presented::Shown {
                    at_ns, refresh_ns, ..
                } = presented
            {
                scheduler.on_presented(f.sample_ns, at_ns, refresh_ns, f.on_schedule, f.work_ns);
                if refresh_ns > 0 {
                    *refresh = refresh_ns as f64 * 1e-9;
                }
            }
        });
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = mono_ns();
        let on_schedule = match self.draw_at {
            Some(at) if now < at => return, // woken early by other input; the timer is still set
            Some(_) => true,
            None => match self.scheduler.draw_at(now) {
                Some(at) => {
                    self.draw_at = Some(at);
                    let wait = Duration::from_nanos(at.saturating_sub(now));
                    event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
                    return;
                }
                None => false,
            },
        };
        self.draw_at = None;
        event_loop.set_control_flow(ControlFlow::Wait);
        // Input that arrived while we waited is already applied: draw it now.
        self.handle_tablet();
        if let Some(a) = self.animation {
            let (view, done) = a.at(self.now());
            self.viewport = view;
            if done {
                self.animation = None;
            }
        }
        self.coast();
        let doc = self.session.doc();
        if self.wire_visible() && self.requested != Some(doc.revision()) {
            self.requested = Some(doc.revision());
            self.recognizer.request(
                self.session.sketch_id,
                doc.revision(),
                doc.elements().to_vec(),
            );
        }
        // A predicted tail must retract once the hand stops: keep drawing until the predictor gives up.
        let predicted = self.build_overlay();
        if self.wireframe.is_some() && self.wire_light == self.dark() {
            self.redraw_wireframe();
        }
        self.update_thumbnails();
        self.build_chrome();
        let lift = self.lift_transform();
        let (show_wire, blend) = (self.wire_visible(), self.blend());
        let (theme, grid) = (self.palette().canvas, self.settings.grid);
        let Some(g) = &mut self.gpu else { return };
        let id = self.next_frame;
        let live = self.live.as_ref().map(|stroke| LiveStroke {
            serial: self.live_serial,
            stroke,
        });
        let lifted = lift.map(|transform| Lifted {
            ids: &self.lifted,
            transform,
        });
        let wire = show_wire.then_some(WireLayer {
            serial: self.wire_serial,
            segments: &self.wire_segments,
        });
        let scene = Scene {
            doc: self.session.doc(),
            live,
            lifted,
            wire,
            blend,
            split: self.split,
            ink_overlay: &self.ink_overlay,
            overlay: &self.overlay,
            theme,
            grid,
        };
        let wl = &self.wl;
        let drawn = g.draw(&scene, self.shell.scene(), &self.viewport, || {
            if let Some(wl) = wl {
                wl.request_feedback(id);
            }
        });
        #[cfg(feature = "profile")]
        if drawn
            && id == 0
            && let Some(start) = crate::STARTED.get()
        {
            eprintln!(
                "profile: first frame presented {:.1} ms after start",
                start.elapsed().as_secs_f64() * 1e3
            );
        }
        if drawn && self.wl.as_ref().is_some_and(|w| w.has_presentation()) {
            self.next_frame += 1;
            self.in_flight.push_back(InFlight {
                id,
                sample_ns: now,
                work_ns: mono_ns().saturating_sub(now),
                on_schedule,
            });
            // Feedback that never comes (e.g. a hidden window) must not grow this forever.
            if self.in_flight.len() > 16 {
                self.in_flight.pop_front();
            }
        }
        if self.animation.is_some() || predicted || self.inertia.is_some() {
            self.redraw();
        }
    }

    /// Touchpad pan inertia: the view keeps gliding after the fingers lift, slowing smoothly. Any
    /// other input stops it (`take_view`); reduced motion never starts it.
    fn coast(&mut self) {
        let Some(last) = self.inertia else { return };
        let now = self.now();
        let dt = (now - last) as f32;
        self.inertia = Some(now);
        self.viewport.pan_by_screen(self.pan_velocity * dt);
        self.pan_velocity *= (-dt / INERTIA_DECAY).exp();
        if self.pan_velocity.length() < INERTIA_STOP {
            self.inertia = None;
        }
    }

    /// Saves everything, including a stroke or erase still in progress, then exits. A failed save keeps
    /// the window open once, unless the system is asking us to stop.
    fn quit(&mut self, event_loop: &ActiveEventLoop, forced: bool) {
        if self.drag.is_some() {
            self.release();
        }
        match self.store.as_ref().map_or(Ok(()), Store::flush) {
            Ok(()) => {}
            Err(e) if forced || self.close_blocked => {
                eprintln!("neuralcanvas: quitting with unsaved changes: {e}");
            }
            Err(e) => {
                self.save_error = Some(e.to_string());
                self.close_blocked = true;
                self.redraw();
                return;
            }
        }
        // Wayland proxies must go before winit closes the display.
        self.access = None;
        self.wl = None;
        self.gpu = None;
        event_loop.exit();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            let gpu = Gpu::new(event_loop);
            self.wl = SideChannel::new(gpu.window.as_ref(), false);
            let (rgba, size) = chrome::ring_cursor(ERASER_RADIUS);
            self.eraser_cursor = CustomCursor::from_rgba(rgba, size, size, size / 2, size / 2)
                .ok()
                .map(|c| event_loop.create_custom_cursor(c));
            gpu.window.set_cursor(CursorIcon::Crosshair);
            // The accessibility adapter must exist before the window is first shown.
            self.access = Some(accesskit_winit::Adapter::with_event_loop_proxy(
                event_loop,
                &gpu.window,
                self.proxy.clone(),
            ));
            gpu.window.set_visible(true);
            gpu.window.request_redraw();
            self.gpu = Some(gpu);
            self.refresh_projects();
            self.update_title();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Store(StoreEvent::Saved { sketch, revision }) => {
                if sketch == self.session.sketch_id {
                    self.saved_revision = self.saved_revision.max(revision);
                }
                self.save_error = None;
                self.close_blocked = false;
                self.redraw();
            }
            UserEvent::Store(StoreEvent::SaveFailed { message, .. }) => {
                self.save_error = Some(message);
                self.redraw();
            }
            UserEvent::ReducedMotion(reduce) => self.reduced_motion = reduce,
            UserEvent::ColorScheme(dark) => {
                if dark != self.system_dark {
                    self.system_dark = dark;
                    self.forget_thumbnails();
                    self.redraw();
                }
            }
            UserEvent::Pasted(elements, centre) => self.pasted(elements, centre),
            UserEvent::ClipboardFailed(e) => self.toast_error(e),
            UserEvent::Projects(result) => {
                self.projects_loaded(result);
                self.update_title();
            }
            UserEvent::SketchOpened { sketch, result } => {
                self.saved_revision = 0;
                self.sketch_opened(sketch, result);
                self.update_title();
            }
            UserEvent::SketchCreated(result) => self.sketch_created(result),
            UserEvent::Thumbnail { sketch, doc } => self.thumbnail_loaded(sketch, doc),
            UserEvent::Access(event) => self.access_event(event.window_event),
            UserEvent::Recognized {
                sketch,
                revision,
                wire,
            } => self.adopt_wireframe(sketch, revision, wire),
            UserEvent::Exported(result) => {
                if let Some(id) = self.exporting.take() {
                    self.shell.dismiss(id);
                }
                match result {
                    Ok(base) => {
                        let name = base
                            .file_name()
                            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
                        self.export_dir = base.parent().map(PathBuf::from);
                        self.toast_info(
                            nc_shell::copy::exported(&name),
                            Some(nc_shell::ToastAction::OpenFolder),
                        );
                    }
                    Err(e) => self.toast_error(nc_shell::copy::export_failed(&e)),
                }
            }
            UserEvent::StoreOpened { result, dir } => self.store_opened(result, dir),
            UserEvent::Quit => self.quit(event_loop, true),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(wl) = &mut self.wl {
            wl.dispatch();
        }
        self.handle_presented();
        self.handle_tablet();
        if self.draw_at.is_some_and(|at| mono_ns() >= at) {
            self.redraw();
        }
        // Toasts leave by themselves: wake when the next one is due, unless a frame is already set.
        let now = self.now();
        let had = self.shell.toasts.len();
        let next = self.shell.tick(now);
        if self.shell.toasts.len() != had {
            self.redraw();
        }
        if let (Some(at), None) = (next, self.draw_at) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                self.epoch + Duration::from_secs_f64(at),
            ));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("window_event");
        if let (Some(access), Some(g)) = (&mut self.access, &self.gpu) {
            access.process_event(&g.window, &event);
        }
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop, false),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.forget_thumbnails();
                self.redraw();
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                // The chrome first: a field being typed in wants repeats too.
                if event.state == ElementState::Pressed && self.chrome_key(&event) {
                    return;
                }
                if event.repeat {
                    return;
                }
                let pressed = event.state == ElementState::Pressed;
                let key = match event.key_without_modifiers().as_ref() {
                    Key::Named(NamedKey::Space) => {
                        self.tools.set_space(pressed);
                        self.set_cursor();
                        self.redraw();
                        return;
                    }
                    // Modifiers change the shape being dragged.
                    Key::Named(NamedKey::Shift | NamedKey::Alt) => {
                        self.redraw();
                        return;
                    }
                    _ if !pressed => return,
                    Key::Named(NamedKey::Delete) => CoreKey::Delete,
                    Key::Named(NamedKey::Backspace) => CoreKey::Backspace,
                    Key::Named(NamedKey::Escape) => CoreKey::Escape,
                    Key::Named(NamedKey::ArrowLeft) => CoreKey::Left,
                    Key::Named(NamedKey::ArrowRight) => CoreKey::Right,
                    Key::Named(NamedKey::ArrowUp) => CoreKey::Up,
                    Key::Named(NamedKey::ArrowDown) => CoreKey::Down,
                    Key::Character(c) => match c.chars().next() {
                        Some(k) => CoreKey::Char(k.to_ascii_lowercase()),
                        None => return,
                    },
                    _ => return,
                };
                let ctrl = self.modifiers.control_key();
                let shortcut = Shortcut {
                    ctrl,
                    shift: self.modifiers.shift_key(),
                    key,
                };
                if let Some(action) = shortcut.action() {
                    self.run(action);
                } else if let (false, CoreKey::Char(k)) = (ctrl, key)
                    && let Some(tool) = Tool::for_key(k)
                {
                    self.choose_tool(tool);
                }
            }
            WindowEvent::CursorMoved { position, .. } if !self.tablet_near => {
                let scale = self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor());
                let p = Vec2::new((position.x / scale) as f32, (position.y / scale) as f32);
                self.window_cursor = p;
                self.pointer_in_right_pane = self.split && p.x >= self.logical_size().x / 2.0;
                // Over chrome, and not mid-drag on the canvas: the chrome has the pointer.
                let over = self.drag.is_none() && self.chrome_hover(p);
                if over != self.over_chrome {
                    self.over_chrome = over;
                    self.set_cursor();
                }
                if !over {
                    self.motion(self.to_pane(p), None);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Middle if pressed => {
                        self.take_view();
                        self.drag = Some(Drag::Pan);
                        self.set_cursor();
                    }
                    MouseButton::Middle if matches!(self.drag, Some(Drag::Pan)) => self.release(),
                    MouseButton::Left if self.tablet_near => {}
                    MouseButton::Left if pressed => {
                        if self.drag.is_none() && self.chrome_press(self.window_cursor) {
                            return;
                        }
                        self.shell.focus = None;
                        self.press(self.cursor, None);
                    }
                    MouseButton::Left if !self.chrome_release(self.window_cursor) => self.release(),
                    MouseButton::Left => {}
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                if self.over_chrome {
                    // Scrolls the sidebar list.
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * 40.0,
                        MouseScrollDelta::PixelDelta(d) => d.y as f32,
                    };
                    self.shell.scroll = (self.shell.scroll - dy).max(0.0);
                    self.redraw();
                    return;
                }
                self.take_view();
                if let MouseScrollDelta::PixelDelta(d) = delta
                    && !self.modifiers.control_key()
                {
                    self.track_pan(Vec2::new(d.x as f32, d.y as f32), phase);
                }
                match delta {
                    // Wheel notches zoom around the cursor; exp() keeps speed device-independent.
                    MouseScrollDelta::LineDelta(_, y) => {
                        self.viewport.zoom_at(self.cursor, (y * 0.15).exp())
                    }
                    MouseScrollDelta::PixelDelta(d) if self.modifiers.control_key() => self
                        .viewport
                        .zoom_at(self.cursor, (d.y as f32 * 0.005).exp()),
                    // Touchpad two-finger scroll pans.
                    MouseScrollDelta::PixelDelta(d) => self
                        .viewport
                        .pan_by_screen(Vec2::new(d.x as f32, d.y as f32)),
                }
                self.redraw();
            }
            // Wayland pinch gestures happen at the pointer, so the cursor is the pinch centroid.
            WindowEvent::PinchGesture { delta, .. } => {
                self.take_view();
                self.viewport.zoom_at(self.cursor, (delta as f32).exp());
                self.redraw();
            }
            _ => {}
        }
    }
}
