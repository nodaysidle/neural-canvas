//! Raw winit 0.30 host: S2k fullscreen under a scripted pan/zoom, a synthetic 1 kHz pen, or real pointer
//! input. Every frame records CPU time, swapchain acquire wait and (via `wp_presentation`) when it hit
//! the screen. Frames can start at a fixed delay after the frame callback, or on the same adaptive
//! schedule the app uses (`nc_core::FrameScheduler`).

use glam::Vec2;
use nc_bench::stats::Summary;
use nc_core::{Command, Document, Element, FrameScheduler, Stroke, Viewport, scene};
use nc_render::{LiveStroke, Renderer};
use nc_wayland::{Presented, SideChannel, mono_ns};
use std::collections::HashMap;
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
    /// Event-driven like the app: a live stroke follows the real pointer over S2k at 100 %. Measures
    /// each motion event's compositor timestamp to the scanout of the first frame that includes it.
    Input,
    /// No rendering load: log what the tablet-v2 side channel and winit pointer events receive.
    Tablet,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Latch {
    /// Start drawing this long after the frame callback (the bench blocks; Phase 2 sweep).
    Fixed(Duration),
    /// The app's adaptive schedule.
    Adaptive,
}

pub struct Config {
    pub scenario: Scenario,
    pub present_mode: wgpu::PresentMode,
    pub secs: f64,
    pub latch: Latch,
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
    on_schedule: bool,
}

struct Bench {
    cfg: Config,
    gpu: Option<Gpu>,
    wl: Option<SideChannel>,
    presented: HashMap<u64, Presented>,
    doc: Document,
    /// The pen's stroke: committed as a command every 2 s (Draw), or following the pointer (Input).
    live: Option<Stroke>,
    live_serial: u64,
    viewport: Viewport,
    started: Option<Instant>,
    frames: Vec<FrameRec>,
    pen_samples: u64,
    winit_pointer_events: u64,
    scheduler: FrameScheduler,
    draw_at: Option<u64>,
    /// Input scenario: compositor timestamps (ms) of motion not yet drawn, and (event ms, frame) pairs.
    pending_motion: Vec<u32>,
    motion_frames: Vec<(u32, u64)>,
    /// Receipt time minus compositor timestamp, ms: checks both clocks are CLOCK_MONOTONIC.
    motion_delivery_ms: Vec<f64>,
    tablet: (usize, Vec<nc_wayland::ToolFrame>),
}

pub fn run(cfg: Config) {
    let event_loop = EventLoop::new().expect("event loop");
    let mut bench = Bench {
        doc: scene::s2k(1),
        live: None,
        live_serial: 0,
        cfg,
        gpu: None,
        wl: None,
        presented: HashMap::new(),
        viewport: Viewport::default(),
        started: None,
        frames: Vec::new(),
        pen_samples: 0,
        winit_pointer_events: 0,
        scheduler: FrameScheduler::default(),
        draw_at: None,
        pending_motion: Vec::new(),
        motion_frames: Vec::new(),
        motion_delivery_ms: Vec::new(),
        tablet: (0, Vec::new()),
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
    fn continuous(&self) -> bool {
        matches!(self.cfg.scenario, Scenario::PanZoom | Scenario::Draw)
    }

    fn script(&mut self, t: f64, size: Vec2) {
        let center = scene::S2K_EXTENT * 0.5;
        match self.cfg.scenario {
            Scenario::PanZoom => {
                let angle = (t / 10.0 * std::f64::consts::TAU) as f32;
                let focus = center + Vec2::new(angle.cos(), angle.sin()) * 1_500.0;
                let scale = (4f32.ln() * (t / 6.0 * std::f64::consts::TAU).sin() as f32).exp();
                self.viewport = Viewport {
                    scale,
                    offset: size * 0.5 - focus * scale,
                };
            }
            Scenario::Draw => {
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
                        self.commit_live();
                        self.live = Some(Stroke::new(p, 2.5, [0.9, 0.6, 0.3, 1.0]));
                        self.live_serial += 1;
                    } else if let Some(stroke) = &mut self.live {
                        stroke.push(p);
                    }
                    self.pen_samples += 1;
                }
            }
            Scenario::Input => {
                self.viewport = Viewport {
                    scale: 1.0,
                    offset: size * 0.5 - center,
                };
            }
            Scenario::Tablet => {}
        }
    }

    fn commit_live(&mut self) {
        if let Some(done) = self.live.take() {
            Command::append(Element::stroke(done))
                .apply(&mut self.doc)
                .expect("append always applies");
        }
    }

    fn dispatch(&mut self) {
        let Some(wl) = &mut self.wl else { return };
        wl.dispatch();
        let (presented, scheduler) = (&mut self.presented, &mut self.scheduler);
        let frames = &self.frames;
        wl.drain_presented(|id, p| {
            presented.insert(id, p);
            if let (
                Some(f),
                Presented::Shown {
                    at_ns, refresh_ns, ..
                },
            ) = (frames.get(id as usize), p)
            {
                scheduler.on_presented(f.start_ns, at_ns, refresh_ns, f.on_schedule);
            }
        });
        let now_ms = mono_ns() / 1_000_000;
        let (pending, delivery) = (&mut self.pending_motion, &mut self.motion_delivery_ms);
        wl.drain_pointer_times(|t| {
            pending.push(t);
            delivery.push(now_ms.wrapping_sub(t as u64) as u32 as f64);
        });
        let tablet = &mut self.tablet;
        wl.drain_tool_frames(|f| {
            tablet.0 += 1;
            if tablet.1.len() < 5 {
                tablet.1.push(f);
            }
        });
    }

    fn report(&mut self) {
        if let Some(wl) = &mut self.wl {
            // Let the last feedback events arrive.
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(20));
                wl.roundtrip();
            }
        }
        self.dispatch();
        let gpu = self.gpu.as_ref().unwrap();
        println!(
            "scenario {:?}, present {:?}, latch {:?}, {}x{} px, scale factor {}",
            self.cfg.scenario,
            self.cfg.present_mode,
            self.cfg.latch,
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
                println!(
                    "tablets {:?}, pads {}, tool frames {} (first: {:?})",
                    w.state.tablets, w.state.pads, self.tablet.0, self.tablet.1
                );
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
        if let (Some(c), Some(a)) = (Summary::of(&cpu), Summary::of(&acquire)) {
            println!("cpu per frame (script + acquire + encode + submit + present): {c}");
            println!("  of which swapchain acquire wait:                          {a}");
        }
        if self.cfg.latch == Latch::Adaptive {
            println!(
                "adaptive latch phase at the end: {:.2} ms after vblank",
                self.scheduler.phase_ns() as f64 / 1e6
            );
        }
        let Some(wl) = &self.wl else {
            println!("no wp_presentation: scanout timing unavailable");
            return;
        };
        let starts: Vec<u64> = self.frames.iter().map(|f| f.start_ns).collect();
        nc_bench::print_scanout(wl.state.clock_id, &self.presented, &starts);
        if self.cfg.scenario == Scenario::Input {
            let latency: Vec<f64> = self
                .motion_frames
                .iter()
                .filter_map(|(t, frame)| match self.presented.get(frame) {
                    Some(Presented::Shown { at_ns, .. }) => {
                        Some(at_ns.saturating_sub(*t as u64 * 1_000_000) as f64 / 1e6)
                    }
                    _ => None,
                })
                .collect();
            println!(
                "winit CursorMoved/MouseInput events: {}",
                self.winit_pointer_events
            );
            if let Some(d) = Summary::of(&self.motion_delivery_ms) {
                println!("pointer event delivery (receipt − compositor timestamp): {d}");
            }
            match Summary::of(&latency) {
                Some(s) => println!("pointer motion event → scanout of its first frame: {s}"),
                None => println!("no pointer motion recorded: move the pointer over the window"),
            }
        }
    }
}

impl ApplicationHandler for Bench {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            let gpu = Gpu::new(event_loop, self.cfg.present_mode);
            self.wl = SideChannel::new(gpu.window.as_ref(), self.cfg.scenario == Scenario::Input);
            println!(
                "wayland side channel: {}, wp_presentation: {}",
                self.wl.is_some(),
                self.wl.as_ref().is_some_and(|w| w.has_presentation())
            );
            gpu.window.request_redraw();
            self.gpu = Some(gpu);
        }
        if !self.continuous() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + WARMUP + Duration::from_secs_f64(self.cfg.secs),
            ));
            self.started.get_or_insert_with(Instant::now);
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if !self.continuous()
            && matches!(cause, StartCause::ResumeTimeReached { .. })
            && self
                .started
                .is_some_and(|s| s.elapsed() >= WARMUP + Duration::from_secs_f64(self.cfg.secs))
        {
            self.finish(event_loop);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.dispatch();
        if self.draw_at.is_some_and(|at| mono_ns() >= at)
            && let Some(g) = &self.gpu
        {
            g.window.request_redraw();
        }
        if !self.continuous()
            && self.draw_at.is_none()
            && let Some(started) = self.started
        {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                started + WARMUP + Duration::from_secs_f64(self.cfg.secs),
            ));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => {
                self.winit_pointer_events += 1;
                let Some(scale) = self.gpu.as_ref().map(|g| g.window.scale_factor()) else {
                    return;
                };
                if self.cfg.scenario == Scenario::Input {
                    let p = Vec2::new((position.x / scale) as f32, (position.y / scale) as f32);
                    let world = self.viewport.screen_to_world(p);
                    match &mut self.live {
                        Some(s) if s.points.len() < 2_000 => {
                            s.push(world);
                        }
                        _ => {
                            self.commit_live();
                            self.live = Some(Stroke::new(world, 2.5, [0.9, 0.6, 0.3, 1.0]));
                            self.live_serial += 1;
                        }
                    }
                    if let Some(g) = &self.gpu {
                        g.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { .. } => self.winit_pointer_events += 1,
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
        if self.continuous() && t > WARMUP + Duration::from_secs_f64(self.cfg.secs) {
            self.finish(event_loop);
            return;
        }
        let on_schedule = match self.cfg.latch {
            Latch::Fixed(late) => {
                // The bench may block here; the app waits with a timer instead.
                std::thread::sleep(late);
                true
            }
            Latch::Adaptive => {
                let now = mono_ns();
                match self.draw_at {
                    Some(at) if now < at => return,
                    Some(_) => true,
                    None => match self.scheduler.draw_at(now) {
                        Some(at) => {
                            self.draw_at = Some(at);
                            let wait = Duration::from_nanos(at - now);
                            event_loop
                                .set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
                            return;
                        }
                        None => false,
                    },
                }
            }
        };
        self.draw_at = None;
        event_loop.set_control_flow(ControlFlow::Wait);
        self.dispatch();
        let start_ns = mono_ns();
        let t0 = Instant::now();
        self.script(t.as_secs_f64(), size);
        let recording = t >= WARMUP;
        let id = self.frames.len() as u64;
        if recording {
            for t in self.pending_motion.drain(..) {
                self.motion_frames.push((t, id));
            }
        } else {
            self.pending_motion.clear();
        }
        let acquire_ms = self.draw_frame(size, scale, recording.then_some(id));
        if recording {
            self.frames.push(FrameRec {
                start_ns,
                acquire_ms,
                cpu_ms: t0.elapsed().as_secs_f64() * 1e3,
                on_schedule,
            });
        }
        if self.continuous() {
            self.gpu.as_ref().unwrap().window.request_redraw();
        }
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
        let live = self.live.as_ref().map(|stroke| LiveStroke {
            serial: self.live_serial,
            stroke,
        });
        g.renderer.render(
            &g.device,
            &g.queue,
            &mut encoder,
            &view,
            &self.doc,
            live,
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
