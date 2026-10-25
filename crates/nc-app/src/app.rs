//! The canvas controller: input (mouse, touchpad, tablet) → tools → commands on the open sketch, the
//! view, frame scheduling and autosave. This thread never touches disk and never blocks on a worker.

use crate::UserEvent;
use crate::gpu::Gpu;
use glam::Vec2;
use nc_core::hit::strokes_touched;
use nc_core::{
    Action, Command, Element, ElementId, FrameScheduler, Session, Shortcut, Smoother, Stroke, Tool,
    Tools, ViewAnimation, Viewport,
};
use nc_render::LiveStroke;
use nc_store::{Store, StoreEvent};
use nc_wayland::{Presented, SideChannel, ToolFrame, ToolKind, mono_ns};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, WindowId};

const INK: [f32; 4] = [0.85, 0.87, 0.93, 1.0];
const PEN_WIDTH: f32 = 2.5;
/// Logical px kept clear around content by Zoom to Fit.
const FIT_PADDING: f32 = 48.0;
/// Eraser reach in logical px, the same at every zoom.
const ERASER_RADIUS: f32 = 8.0;
/// Points reserved per live stroke, so the hot input path does not reallocate (a few seconds of 1 kHz
/// input; longer strokes grow once per doubling).
const LIVE_RESERVE: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Draw,
    Erase,
    Pan,
}

/// A presented frame we asked feedback for.
struct InFlight {
    id: u64,
    sample_ns: u64,
    on_schedule: bool,
}

pub struct App {
    gpu: Option<Gpu>,
    wl: Option<SideChannel>,
    store: Store,
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
    /// Logical px.
    cursor: Vec2,
    last_erase: Vec2,
    hits: Vec<ElementId>,
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
}

impl App {
    pub fn new(store: Store, session: Session, read_only: bool) -> Self {
        Self {
            gpu: None,
            wl: None,
            store,
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
            cursor: Vec2::ZERO,
            last_erase: Vec2::ZERO,
            hits: Vec::new(),
            modifiers: ModifiersState::default(),
            tablet_near: false,
            tablet_frames: Vec::new(),
            epoch: Instant::now(),
            scheduler: FrameScheduler::default(),
            draw_at: None,
            next_frame: 0,
            in_flight: VecDeque::new(),
        }
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
        if let Some(g) = &self.gpu {
            let icon = match (self.drag, self.tools.active()) {
                (Some(Drag::Pan), _) => CursorIcon::Grabbing,
                (_, Tool::Hand) => CursorIcon::Grab,
                // shortcut: a stock cursor until Phase 3 draws the eraser ring at its true size.
                (_, Tool::Eraser) => CursorIcon::Cell,
                (_, Tool::Pen) => CursorIcon::Crosshair,
            };
            g.window.set_cursor(icon);
        }
    }

    // shortcut: problems show in the window title until the Phase 5 shell has banners and toasts.
    fn update_title(&self) {
        let Some(g) = &self.gpu else { return };
        let title = match (&self.save_error, self.read_only) {
            (Some(e), _) => format!("NeuralCanvas (not saved: {e})"),
            (None, true) => "NeuralCanvas (read-only: this sketch could not be read)".into(),
            (None, false) => "NeuralCanvas".into(),
        };
        g.window.set_title(&title);
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
        let doc = self.session.doc();
        self.store.save(
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

    /// Any direct manipulation of the view stops a running view animation.
    fn take_view(&mut self) {
        self.animation = None;
    }

    // ---- input, shared by mouse and tablet. Positions are logical px. ----

    fn press(&mut self, pos: Vec2, pressure: Option<f32>) {
        self.cursor = pos;
        self.take_view();
        match self.tools.active() {
            Tool::Hand => self.drag = Some(Drag::Pan),
            _ if self.read_only => {}
            Tool::Pen => {
                self.smoother.reset();
                let p = self.smoother.smooth(pos, self.now());
                let world = self.viewport.screen_to_world(p);
                let mut stroke = match pressure {
                    Some(pr) => Stroke::with_pressure(world, pr, PEN_WIDTH, INK),
                    None => Stroke::new(world, PEN_WIDTH, INK),
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
        match self.drag {
            Some(Drag::Pan) => {
                self.viewport.pan_by_screen(delta);
                self.redraw();
            }
            Some(Drag::Draw) => {
                let p = self.smoother.smooth(pos, self.now());
                let world = self.viewport.screen_to_world(p);
                if let Some(stroke) = &mut self.live
                    && stroke.push_sample(world, pressure)
                {
                    self.redraw();
                }
            }
            Some(Drag::Erase) => self.erase_to(pos),
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
            Some(Drag::Pan) | None => {}
        }
        self.set_cursor();
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

    fn erase_to(&mut self, pos: Vec2) {
        let to = self.viewport.screen_to_world(pos);
        let radius = ERASER_RADIUS / self.viewport.scale;
        self.hits.clear();
        strokes_touched(
            self.session.doc(),
            self.last_erase,
            to,
            radius,
            &mut self.hits,
        );
        self.last_erase = to;
        for i in 0..self.hits.len() {
            let id = self.hits[i];
            if let Err(e) = self
                .session
                .apply_in_gesture("Erase", Command::Remove { id })
            {
                eprintln!("neuralcanvas: erase rejected: {e}");
            }
        }
        if !self.hits.is_empty() {
            self.redraw();
        }
    }

    fn handle_tablet(&mut self) {
        let Some(wl) = &mut self.wl else { return };
        // Taken out so input handling can borrow `self`; the buffer keeps its capacity.
        let mut frames = std::mem::take(&mut self.tablet_frames);
        wl.drain_tool_frames(|f| frames.push(f));
        for f in frames.drain(..) {
            let pos = Vec2::new(f.position.0 as f32, f.position.1 as f32);
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
        if !action.enabled(self.session.history())
            || (self.read_only && matches!(action, Action::Undo | Action::Redo))
        {
            return;
        }
        match action {
            Action::Undo | Action::Redo => {
                if self.drag == Some(Drag::Draw) {
                    self.finish_stroke();
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
            }
            Action::FitToContent => {
                let target = self
                    .session
                    .doc()
                    .bounds()
                    .map_or(Viewport::default(), |b| {
                        Viewport::fit(b, self.logical_size(), FIT_PADDING)
                    });
                self.animate_to(target);
            }
            Action::ActualSize => {
                let mut target = self.viewport;
                target.zoom_at(self.logical_size() * 0.5, 1.0 / target.scale);
                self.animate_to(target);
            }
        }
        self.redraw();
    }

    fn animate_to(&mut self, target: Viewport) {
        self.animation = ViewAnimation::new(
            self.viewport,
            target,
            self.now(),
            self.logical_size(),
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
        let (in_flight, scheduler) = (&mut self.in_flight, &mut self.scheduler);
        wl.drain_presented(|id, presented| {
            while in_flight.front().is_some_and(|f| f.id < id) {
                in_flight.pop_front();
            }
            if let Some(f) = in_flight.pop_front().filter(|f| f.id == id)
                && let Presented::Shown {
                    at_ns, refresh_ns, ..
                } = presented
            {
                scheduler.on_presented(f.sample_ns, at_ns, refresh_ns, f.on_schedule);
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
        let Some(g) = &mut self.gpu else { return };
        let id = self.next_frame;
        let live = self.live.as_ref().map(|stroke| LiveStroke {
            serial: self.live_serial,
            stroke,
        });
        let wl = &self.wl;
        let drawn = g.draw(self.session.doc(), live, &self.viewport, || {
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
                on_schedule,
            });
            // Feedback that never comes (e.g. a hidden window) must not grow this forever.
            if self.in_flight.len() > 16 {
                self.in_flight.pop_front();
            }
        }
        if self.animation.is_some() {
            self.redraw();
        }
    }

    /// Saves everything, including a stroke or erase still in progress, then exits. A failed save keeps
    /// the window open once, unless the system is asking us to stop.
    fn quit(&mut self, event_loop: &ActiveEventLoop, forced: bool) {
        if self.drag.is_some() {
            self.release();
        }
        match self.store.flush() {
            Ok(()) => {}
            Err(e) if forced || self.close_blocked => {
                eprintln!("neuralcanvas: quitting with unsaved changes: {e}");
            }
            Err(e) => {
                self.save_error = Some(e.to_string());
                self.close_blocked = true;
                self.update_title();
                return;
            }
        }
        // Wayland proxies must go before winit closes the display.
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
            gpu.window.set_cursor(CursorIcon::Crosshair);
            gpu.window.request_redraw();
            self.gpu = Some(gpu);
            self.update_title();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Store(StoreEvent::Saved { .. }) if self.save_error.is_some() => {
                self.save_error = None;
                self.close_blocked = false;
                self.update_title();
            }
            UserEvent::Store(StoreEvent::Saved { .. }) => {}
            UserEvent::Store(StoreEvent::SaveFailed { message, .. }) => {
                self.save_error = Some(message);
                self.update_title();
            }
            UserEvent::ReducedMotion(reduce) => self.reduced_motion = reduce,
            UserEvent::Quit => self.quit(event_loop, true),
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(wl) = &mut self.wl {
            wl.dispatch();
        }
        self.handle_presented();
        self.handle_tablet();
        if self.draw_at.is_some_and(|at| mono_ns() >= at) {
            self.redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("window_event");
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop, false),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::RedrawRequested => self.frame(event_loop),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                let pressed = event.state == ElementState::Pressed;
                match event.key_without_modifiers().as_ref() {
                    Key::Named(NamedKey::Space) => {
                        self.tools.set_space(pressed);
                        self.set_cursor();
                    }
                    Key::Character(c) if pressed => {
                        let Some(key) = c.chars().next().map(|k| k.to_ascii_lowercase()) else {
                            return;
                        };
                        let ctrl = self.modifiers.control_key();
                        let shortcut = Shortcut {
                            ctrl,
                            shift: self.modifiers.shift_key(),
                            key,
                        };
                        if let Some(action) = shortcut.action() {
                            self.run(action);
                        } else if let (false, Some(tool)) = (ctrl, Tool::for_key(key)) {
                            self.tools.choose(tool);
                            self.set_cursor();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } if !self.tablet_near => {
                let scale = self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor());
                let p = Vec2::new((position.x / scale) as f32, (position.y / scale) as f32);
                self.motion(p, None);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Middle if pressed => {
                        self.take_view();
                        self.drag = Some(Drag::Pan);
                        self.set_cursor();
                    }
                    MouseButton::Middle if self.drag == Some(Drag::Pan) => self.release(),
                    MouseButton::Left if self.tablet_near => {}
                    MouseButton::Left if pressed => self.press(self.cursor, None),
                    MouseButton::Left => self.release(),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.take_view();
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
