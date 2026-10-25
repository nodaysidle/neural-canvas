//! NeuralCanvas window: winit event loop + wgpu surface hosting the nc-render canvas.
//! Frames are drawn only when something changed (idle = 0% CPU, docs/PERFORMANCE-BUDGET.md).
//! Every edit is a command on the open sketch's session and goes to the store thread; this thread
//! never touches disk.

use glam::Vec2;
use nc_core::{Action, Command, Document, Element, Session, Shortcut, Stroke, Viewport};
use nc_render::Renderer;
use nc_store::{Store, StoreError, StoreEvent};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{CursorIcon, Window, WindowId};

const APP_ID: &str = "sh.nodaysidle.NeuralCanvas";
const INK: [f32; 4] = [0.85, 0.87, 0.93, 1.0];
const PEN_WIDTH: f32 = 2.5;
/// Logical px kept clear around content by Zoom to Fit.
const FIT_PADDING: f32 = 48.0;

/// Wakes the event loop from other threads.
enum UserEvent {
    Store(StoreEvent),
    /// SIGTERM / SIGINT / SIGHUP: save and quit.
    Quit,
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

struct App {
    gpu: Option<Gpu>,
    store: Store,
    session: Session,
    /// The stroke under the pen. It becomes a "Draw" command on release.
    live: Option<Stroke>,
    /// The sketch could not be read: show what loaded, accept no edits, never save over it.
    read_only: bool,
    save_error: Option<String>,
    /// A failed save on close keeps the window open once; closing again quits anyway.
    close_blocked: bool,
    viewport: Viewport,
    cursor: Vec2, // logical px
    modifiers: ModifiersState,
    space_held: bool,
    panning: bool,
}

impl Gpu {
    fn new(event_loop: &ActiveEventLoop) -> Self {
        let attrs = Window::default_attributes()
            .with_title("NeuralCanvas")
            .with_name(APP_ID, "")
            .with_min_inner_size(winit::dpi::LogicalSize::new(640.0, 480.0))
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
                Box::new(event_loop.owned_display_handle()),
            ));
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("no compatible GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .expect("request device");
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        let caps = surface.get_capabilities(&adapter);
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 1; // shortest input-to-photon queue
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, config.format);
        Self {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    fn draw(&mut self, doc: &Document, live: Option<&Stroke>, viewport: &Viewport) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let scale = self.window.scale_factor() as f32;
        let size = [
            self.config.width as f32 / scale,
            self.config.height as f32 / scale,
        ];
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.renderer.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            doc,
            live,
            viewport,
            size,
            scale,
        );
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
        #[cfg(feature = "profile")]
        tracy_client::frame_mark();
    }
}

impl App {
    fn redraw(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    fn set_cursor(&self) {
        if let Some(g) = &self.gpu {
            let icon = match (self.panning, self.space_held) {
                (true, _) => CursorIcon::Grabbing,
                (false, true) => CursorIcon::Grab,
                _ => CursorIcon::Crosshair,
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

    fn finish_stroke(&mut self) {
        if let Some(stroke) = self.live.take() {
            self.execute("Draw", Command::append(Element::stroke(stroke)));
        }
    }

    fn run(&mut self, action: Action) {
        if !action.enabled(self.session.history())
            || (self.read_only && matches!(action, Action::Undo | Action::Redo))
        {
            return;
        }
        match action {
            Action::Undo | Action::Redo => {
                let result = if action == Action::Undo {
                    self.session.undo()
                } else {
                    self.session.redo()
                };
                match result {
                    Ok(true) => self.persist(),
                    Ok(false) => {}
                    Err(e) => eprintln!(
                        "neuralcanvas: {} failed: {e}",
                        action.label(self.session.history())
                    ),
                }
            }
            Action::FitToContent => {
                self.viewport = self
                    .session
                    .doc()
                    .bounds()
                    .map_or(Viewport::default(), |b| {
                        Viewport::fit(b, self.logical_size(), FIT_PADDING)
                    });
            }
            Action::ActualSize => {
                let center = self.logical_size() * 0.5;
                self.viewport.zoom_at(center, 1.0 / self.viewport.scale);
            }
        }
        self.redraw();
    }

    /// Saves everything, including a stroke still under the pen, then exits. A failed save keeps the
    /// window open once, unless the system is asking us to stop.
    fn quit(&mut self, event_loop: &ActiveEventLoop, forced: bool) {
        self.finish_stroke();
        match self.store.flush() {
            Ok(()) => event_loop.exit(),
            Err(e) if forced || self.close_blocked => {
                eprintln!("neuralcanvas: quitting with unsaved changes: {e}");
                event_loop.exit();
            }
            Err(e) => {
                self.save_error = Some(e.to_string());
                self.close_blocked = true;
                self.update_title();
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            let gpu = Gpu::new(event_loop);
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
            UserEvent::Quit => self.quit(event_loop, true),
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
            WindowEvent::RedrawRequested => {
                if let Some(g) = &mut self.gpu {
                    g.draw(self.session.doc(), self.live.as_ref(), &self.viewport);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                let pressed = event.state == ElementState::Pressed;
                match event.key_without_modifiers().as_ref() {
                    Key::Named(NamedKey::Space) => {
                        self.space_held = pressed;
                        self.set_cursor();
                    }
                    Key::Character(c) if pressed => {
                        let shortcut = c.chars().next().map(|key| Shortcut {
                            ctrl: self.modifiers.control_key(),
                            shift: self.modifiers.shift_key(),
                            key: key.to_ascii_lowercase(),
                        });
                        if let Some(action) = shortcut.and_then(Shortcut::action) {
                            self.run(action);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor());
                let p = Vec2::new((position.x / scale) as f32, (position.y / scale) as f32);
                let delta = p - self.cursor;
                self.cursor = p;
                if self.panning {
                    self.viewport.pan_by_screen(delta);
                    self.redraw();
                } else if let Some(stroke) = &mut self.live
                    && stroke.push(self.viewport.screen_to_world(p))
                {
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Middle => self.panning = pressed,
                    MouseButton::Left if pressed && self.space_held => self.panning = true,
                    MouseButton::Left if pressed && !self.read_only => {
                        let world = self.viewport.screen_to_world(self.cursor);
                        self.live = Some(Stroke::new(world, PEN_WIDTH, INK));
                        self.redraw();
                    }
                    MouseButton::Left if !pressed => {
                        self.finish_stroke();
                        self.panning = false;
                    }
                    _ => {}
                }
                self.set_cursor();
            }
            WindowEvent::MouseWheel { delta, .. } => {
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
            WindowEvent::PinchGesture { delta, .. } => {
                self.viewport.zoom_at(self.cursor, (delta as f32).exp());
                self.redraw();
            }
            _ => {}
        }
    }
}

/// Opens the store and the most recent sketch. A damaged sketch opens read-only and empty; anything
/// else that stops us saving is fatal, before any drawing can happen (Mac bug #4).
fn open(proxy: EventLoopProxy<UserEvent>) -> Result<(Store, Session, bool), StoreError> {
    let path = dirs::data_dir()
        .ok_or_else(|| {
            StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no home or XDG data directory",
            ))
        })?
        .join("neuralcanvas")
        .join("neuralcanvas.db");
    let store = Store::open(path, move |event| {
        let _ = proxy.send_event(UserEvent::Store(event));
    })?;
    let sketch = store.call(|db| db.last_or_new_sketch())?;
    match store.call(move |db| db.load(sketch)) {
        Ok(doc) => Ok((store, Session::new(sketch, doc), false)),
        // shortcut: opens empty instead of showing the readable rows; a partial load needs the Phase 5
        // UI to explain what is missing.
        Err(e @ StoreError::Corrupt { .. }) => {
            eprintln!("neuralcanvas: opening read-only: {e}");
            Ok((store, Session::new(sketch, Document::default()), true))
        }
        Err(e) => Err(e),
    }
}

fn main() {
    #[cfg(feature = "profile")]
    tracy_client::Client::start();
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("event loop");
    let (store, session, read_only) = match open(event_loop.create_proxy()) {
        Ok(opened) => opened,
        // shortcut: stderr and exit, before any window; the Phase 5 shell turns this into the blocking
        // Retry / Choose location / Quit dialog (docs/PLAN.md §4, bug #4).
        Err(e) => {
            eprintln!("neuralcanvas: cannot open the sketch store, nothing would be saved: {e}");
            std::process::exit(1);
        }
    };
    let signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ])
    .expect("install signal handlers");
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        let mut signals = signals;
        if signals.forever().next().is_some() {
            let _ = proxy.send_event(UserEvent::Quit);
        }
    });
    let mut app = App {
        gpu: None,
        store,
        session,
        live: None,
        read_only,
        save_error: None,
        close_blocked: false,
        viewport: Viewport::default(),
        cursor: Vec2::ZERO,
        modifiers: ModifiersState::default(),
        space_held: false,
        panning: false,
    };
    event_loop.run_app(&mut app).expect("run app");
}
