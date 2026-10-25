//! NeuralCanvas window: winit event loop + wgpu surface hosting the nc-render canvas.
//! Frames are drawn only when something changed (idle = 0% CPU, docs/PERFORMANCE-BUDGET.md).

use glam::Vec2;
use nc_core::{Document, Stroke, Viewport};
use nc_render::Renderer;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{CursorIcon, Window, WindowId};

const APP_ID: &str = "sh.nodaysidle.NeuralCanvas";
const INK: [f32; 4] = [0.85, 0.87, 0.93, 1.0];
const PEN_WIDTH: f32 = 2.5;

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

#[derive(Default)]
struct App {
    gpu: Option<Gpu>,
    doc: Document,
    viewport: Viewport,
    cursor: Vec2, // logical px
    modifiers: ModifiersState,
    space_held: bool,
    drawing: bool,
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

    fn draw(&mut self, doc: &Document, viewport: &Viewport) {
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            let gpu = Gpu::new(event_loop);
            gpu.window.set_cursor(CursorIcon::Crosshair);
            gpu.window.request_redraw();
            self.gpu = Some(gpu);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("window_event");
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::RedrawRequested => {
                if let Some(g) = &mut self.gpu {
                    g.draw(&self.doc, &self.viewport);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                let pressed = event.state == ElementState::Pressed;
                match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Space) => {
                        self.space_held = pressed;
                        self.set_cursor();
                    }
                    Key::Character("0") if pressed && self.modifiers.control_key() => {
                        self.viewport = Viewport::default();
                        self.redraw();
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
                } else if self.drawing {
                    let world = self.viewport.screen_to_world(p);
                    if self.doc.strokes.last_mut().is_some_and(|s| s.push(world)) {
                        self.redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Middle => self.panning = pressed,
                    MouseButton::Left if pressed && self.space_held => self.panning = true,
                    MouseButton::Left if pressed => {
                        let world = self.viewport.screen_to_world(self.cursor);
                        self.doc.strokes.push(Stroke::new(world, PEN_WIDTH, INK));
                        self.drawing = true;
                        self.redraw();
                    }
                    MouseButton::Left => {
                        self.drawing = false;
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

fn main() {
    #[cfg(feature = "profile")]
    tracy_client::Client::start();
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.run_app(&mut App::default()).expect("run app");
}
