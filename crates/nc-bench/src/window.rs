//! Raw winit 0.30 host: S2k under a scripted pan/zoom or live-draw run, fullscreen on the current output.
//! Every frame records CPU time, swapchain acquire wait and (via `wp_presentation`) when it hit the screen.

use glam::Vec2;
use nc_bench::stats::Summary;
use nc_bench::wl::SideChannel;
use nc_core::{Command, Document, Element, Stroke, Viewport, scene};
use nc_render::Renderer;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Window, WindowId};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scenario {
    /// Zoom 0.25×–4× while circling the scene: the whole S2k is on screen at the far end.
    PanZoom,
    /// Fixed view, a 1 kHz synthetic pen appending to a live stroke on top of S2k.
    Draw,
    /// No rendering load: log what the tablet-v2 side channel and winit pointer events receive.
    Tablet,
}

pub struct Config {
    pub scenario: Scenario,
    pub present_mode: wgpu::PresentMode,
    pub secs: f64,
}

const WARMUP: Duration = Duration::from_secs(1);
const PEN_HZ: f64 = 1000.0;

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

struct FrameRec {
    start_ns: u64,
    acquire_ms: f64,
    cpu_ms: f64,
}

struct Bench {
    cfg: Config,
    gpu: Option<Gpu>,
    wl: Option<SideChannel>,
    doc: Document,
    /// The synthetic pen's stroke, committed as a command every 2 s like the app does on pen-up.
    live: Option<Stroke>,
    viewport: Viewport,
    started: Option<Instant>,
    frames: Vec<FrameRec>,
    pen_samples: u64,
    winit_pointer_events: u64,
}

pub fn run(cfg: Config) {
    let event_loop = EventLoop::new().expect("event loop");
    let mut bench = Bench {
        doc: scene::s2k(1),
        live: None,
        cfg,
        gpu: None,
        wl: None,
        viewport: Viewport::default(),
        started: None,
        frames: Vec::new(),
        pen_samples: 0,
        winit_pointer_events: 0,
    };
    event_loop.run_app(&mut bench).expect("run app");
}

impl Gpu {
    fn new(event_loop: &ActiveEventLoop, present_mode: wgpu::PresentMode) -> Self {
        let attrs = Window::default_attributes()
            .with_title("NeuralCanvas bench")
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
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
        let caps = surface.get_capabilities(&adapter);
        let info = adapter.get_info();
        println!(
            "adapter: {} ({:?}, {})",
            info.name, info.backend, info.driver_info
        );
        println!("surface present modes: {:?}", caps.present_modes);
        assert!(
            caps.present_modes.contains(&present_mode),
            "{present_mode:?} not supported by this surface"
        );
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        config.present_mode = present_mode;
        config.desired_maximum_frame_latency = 1; // same as the app
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
}

impl Bench {
    fn script(&mut self, t: f64, size: Vec2) {
        match self.cfg.scenario {
            Scenario::PanZoom => {
                let center = scene::S2K_EXTENT * 0.5;
                let angle = (t / 10.0 * std::f64::consts::TAU) as f32;
                let focus = center + Vec2::new(angle.cos(), angle.sin()) * 1_500.0;
                let scale = (4f32.ln() * (t / 6.0 * std::f64::consts::TAU).sin() as f32).exp();
                self.viewport = Viewport {
                    scale,
                    offset: size * 0.5 - focus * scale,
                };
            }
            Scenario::Draw => {
                let center = scene::S2K_EXTENT * 0.5;
                self.viewport = Viewport {
                    scale: 1.0,
                    offset: size * 0.5 - center,
                };
                // A new stroke every 2 s, sampled at 1 kHz along a wobbly loop.
                let due = (t * PEN_HZ) as u64;
                while self.pen_samples < due {
                    let s = self.pen_samples as f64 / PEN_HZ;
                    let p = center
                        + Vec2::new(
                            (s * 2.1).cos() as f32 * 400.0,
                            (s * 3.3).sin() as f32 * 250.0,
                        );
                    if self.pen_samples.is_multiple_of(2_000) {
                        if let Some(done) = self.live.take() {
                            Command::append(Element::stroke(done))
                                .apply(&mut self.doc)
                                .expect("append always applies");
                        }
                        self.live = Some(Stroke::new(p, 2.5, [0.9, 0.6, 0.3, 1.0]));
                    } else if let Some(stroke) = &mut self.live {
                        stroke.push(p);
                    }
                    self.pen_samples += 1;
                }
            }
            Scenario::Tablet => {}
        }
    }

    fn report(&mut self) {
        if let Some(wl) = &mut self.wl {
            // Let the last feedback events arrive.
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(20));
                wl.roundtrip();
            }
        }
        let gpu = self.gpu.as_ref().unwrap();
        println!(
            "scenario {:?}, present {:?}, {}x{} px, scale factor {}",
            self.cfg.scenario,
            self.cfg.present_mode,
            gpu.config.width,
            gpu.config.height,
            gpu.window.scale_factor()
        );
        if self.cfg.scenario == Scenario::Tablet {
            let wl = self.wl.as_ref();
            println!(
                "zwp_tablet_manager_v2 bound: {}",
                wl.is_some_and(|w| w.has_tablet_manager())
            );
            if let Some(w) = wl {
                println!("tablet log: {:#?}", w.state.tablet);
            }
            println!(
                "winit 0.30 pointer events (CursorMoved/MouseInput): {}",
                self.winit_pointer_events
            );
            return;
        }
        let cpu: Vec<f64> = self.frames.iter().map(|f| f.cpu_ms).collect();
        let acquire: Vec<f64> = self.frames.iter().map(|f| f.acquire_ms).collect();
        let secs = self.cfg.secs;
        println!(
            "frames rendered: {} in {secs:.0} s ({:.1}/s)",
            self.frames.len(),
            self.frames.len() as f64 / secs
        );
        println!(
            "cpu per frame (script + acquire + encode + submit + present): {}",
            Summary::of(&cpu).unwrap()
        );
        println!(
            "  of which swapchain acquire wait:                          {}",
            Summary::of(&acquire).unwrap()
        );
        let Some(wl) = &self.wl else {
            println!("no wp_presentation: scanout timing unavailable");
            return;
        };
        let starts: Vec<u64> = self.frames.iter().map(|f| f.start_ns).collect();
        wl.print_scanout(&starts);
    }
}

impl ApplicationHandler for Bench {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            let gpu = Gpu::new(event_loop, self.cfg.present_mode);
            self.wl = SideChannel::new(gpu.window.as_ref());
            println!(
                "wayland side channel: {}, wp_presentation: {}",
                self.wl.is_some(),
                self.wl.as_ref().is_some_and(|w| w.has_presentation())
            );
            gpu.window.request_redraw();
            self.gpu = Some(gpu);
        }
        if self.cfg.scenario == Scenario::Tablet {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_secs_f64(self.cfg.secs),
            ));
            self.started.get_or_insert_with(Instant::now);
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if self.cfg.scenario == Scenario::Tablet
            && matches!(cause, StartCause::ResumeTimeReached { .. })
        {
            self.finish(event_loop);
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(wl) = &mut self.wl {
            wl.dispatch();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } => {
                self.winit_pointer_events += 1;
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
                    g.surface.configure(&g.device, &g.config);
                    g.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }
}

impl Bench {
    /// Wayland proxies and the surface must go before winit closes the display, or teardown segfaults.
    fn finish(&mut self, event_loop: &ActiveEventLoop) {
        self.report();
        self.wl = None;
        self.gpu = None;
        event_loop.exit();
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(g) = &self.gpu else { return };
        if g.config.width == 0 {
            return;
        }
        let scale = g.window.scale_factor() as f32;
        let size = Vec2::new(g.config.width as f32, g.config.height as f32) / scale;
        let started = *self.started.get_or_insert_with(Instant::now);
        let t = started.elapsed();
        if self.cfg.scenario == Scenario::Tablet {
            self.draw_frame(size, scale, None);
            return;
        }
        if t > WARMUP + Duration::from_secs_f64(self.cfg.secs) {
            self.finish(event_loop);
            return;
        }
        let start_ns = nc_bench::wl::mono_ns();
        let t0 = Instant::now();
        self.script(t.as_secs_f64(), size);
        let recording = t >= WARMUP;
        let idx = recording.then_some(self.frames.len() as u64);
        let acquire_ms = self.draw_frame(size, scale, idx);
        if recording {
            self.frames.push(FrameRec {
                start_ns,
                acquire_ms,
                cpu_ms: t0.elapsed().as_secs_f64() * 1e3,
            });
        }
        if let Some(wl) = &mut self.wl {
            wl.dispatch();
        }
        self.gpu.as_ref().unwrap().window.request_redraw();
    }

    /// Returns the swapchain acquire wait in ms.
    fn draw_frame(&mut self, size: Vec2, scale: f32, feedback: Option<u64>) -> f64 {
        let g = self.gpu.as_mut().unwrap();
        let t_acq = Instant::now();
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                g.surface.configure(&g.device, &g.config);
                return 0.0;
            }
            other => panic!("surface acquire failed: {other:?}"),
        };
        let acquire_ms = t_acq.elapsed().as_secs_f64() * 1e3;
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = g.device.create_command_encoder(&Default::default());
        g.renderer.render(
            &g.device,
            &g.queue,
            &mut encoder,
            &view,
            &self.doc,
            self.live.as_ref(),
            &self.viewport,
            size.to_array(),
            scale,
        );
        g.queue.submit([encoder.finish()]);
        if let (Some(i), Some(wl)) = (feedback, &self.wl) {
            wl.request_feedback(i);
        }
        g.window.pre_present_notify();
        g.queue.present(frame);
        acquire_ms
    }
}
