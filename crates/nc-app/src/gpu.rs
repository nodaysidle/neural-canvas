//! The window's wgpu surface and the canvas renderer.

use nc_core::Viewport;
use nc_render::{Renderer, Scene};
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::Window;

const APP_ID: &str = "sh.nodaysidle.NeuralCanvas";

pub struct Gpu {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

impl Gpu {
    pub fn new(event_loop: &ActiveEventLoop) -> Self {
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
        // Fifo: Phase 0 measured Mailbox and Immediate identical on Hyprland (docs/PLAN.md §1).
        config.present_mode = wgpu::PresentMode::Fifo;
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

    pub fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Draws and presents one frame. `before_present` runs right before the commit (presentation
    /// feedback must be requested then). Returns false if no frame was presented.
    pub fn draw(
        &mut self,
        scene: &Scene,
        viewport: &Viewport,
        before_present: impl FnOnce(),
    ) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return false;
            }
            _ => return false,
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
            scene,
            viewport,
            size,
            scale,
        );
        self.queue.submit([encoder.finish()]);
        before_present();
        self.window.pre_present_notify();
        self.queue.present(frame);
        #[cfg(feature = "profile")]
        tracy_client::frame_mark();
        true
    }
}
