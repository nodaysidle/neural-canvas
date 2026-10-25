//! Phase 0 spike: the S2k pan/zoom run of `nc-bench window panzoom` hosted in an iced 0.14 `shader`
//! widget instead of raw winit. iced 0.14 pins wgpu 27, so `nc-render` (wgpu 30) cannot be used as is:
//! this is a line-for-line wgpu 27 port of its pipeline, same WGSL, same O(points) sync, so the two
//! runs differ only in the host.
//!
//! `ICED_PRESENT_MODE=fifo|mailbox|immediate spike-iced [secs]`

use glam::Vec2;
use iced::widget::shader::{self, Viewport};
use iced::{Element, Length, Rectangle, Task, mouse, wgpu, window};
use nc_bench::stats::Summary;
use nc_core::{Document, scene};
use nc_wayland::{SideChannel, mono_ns};
use std::collections::HashMap;
use std::mem::{offset_of, size_of};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WARMUP: Duration = Duration::from_secs(1);

static SIDE: Mutex<Option<SideChannel>> = Mutex::new(None);
/// Per recorded frame (index = feedback id): redraw event ns, prepare start ns, prepare CPU ms.
static FRAMES: Mutex<Vec<(u64, u64, f64)>> = Mutex::new(Vec::new());

fn main() -> iced::Result {
    let secs: f64 = std::env::args()
        .nth(1)
        .map_or(20.0, |s| s.parse().expect("seconds"));
    iced::application(move || Bench::new(secs), Bench::update, Bench::view)
        .subscription(|_| window::frames().map(Message::Frame))
        .window(window::Settings {
            fullscreen: true,
            ..Default::default()
        })
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Frame(Instant),
    Window(Option<window::Id>),
    SideReady,
}

struct Bench {
    secs: f64,
    doc: Arc<Document>,
    base: (Instant, u64),
    started: Option<Instant>,
    t: f64,
    redraw_ns: u64,
    recording: bool,
}

impl Bench {
    fn new(secs: f64) -> (Self, Task<Message>) {
        let bench = Self {
            secs,
            doc: Arc::new(scene::s2k(1)),
            base: (Instant::now(), mono_ns()),
            started: None,
            t: 0.0,
            redraw_ns: 0,
            recording: false,
        };
        (bench, window::oldest().map(Message::Window))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Window(Some(id)) => window::run(id, |w| {
                *SIDE.lock().unwrap() = SideChannel::new(w, false);
            })
            .map(|_| Message::SideReady),
            Message::Window(None) => Task::none(),
            Message::SideReady => {
                let side = SIDE.lock().unwrap();
                println!(
                    "wayland side channel: {}, wp_presentation: {}",
                    side.is_some(),
                    side.as_ref().is_some_and(|s| s.has_presentation())
                );
                Task::none()
            }
            Message::Frame(at) => {
                let started = *self.started.get_or_insert(at);
                let t = at.duration_since(started);
                if let Some(side) = SIDE.lock().unwrap().as_mut() {
                    side.dispatch();
                }
                if t > WARMUP + Duration::from_secs_f64(self.secs) {
                    self.report();
                    return iced::exit();
                }
                self.t = t.as_secs_f64();
                self.recording = t >= WARMUP;
                self.redraw_ns = self.base.1 + at.duration_since(self.base.0).as_nanos() as u64;
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        shader::Shader::new(Canvas {
            doc: self.doc.clone(),
            t: self.t,
            redraw_ns: self.redraw_ns,
            recording: self.recording,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn report(&self) {
        let mut side = SIDE.lock().unwrap();
        if let Some(s) = side.as_mut() {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(20));
                s.roundtrip();
            }
        }
        let frames = FRAMES.lock().unwrap();
        println!(
            "host iced 0.14 shader widget, present {:?}",
            std::env::var("ICED_PRESENT_MODE").unwrap_or_else(|_| "default (AutoVsync)".into())
        );
        println!(
            "frames rendered: {} in {:.0} s ({:.1}/s)",
            frames.len(),
            self.secs,
            frames.len() as f64 / self.secs
        );
        let prepare: Vec<f64> = frames.iter().map(|f| f.2).collect();
        if let Some(s) = Summary::of(&prepare) {
            println!("cpu in primitive prepare (uniform + sync): {s}");
        }
        match side.as_mut() {
            Some(s) => {
                let mut presented = HashMap::new();
                s.drain_presented(|id, p| {
                    presented.insert(id, p);
                });
                let clock = s.state.clock_id;
                println!("-- start = the RedrawRequested whose frames() message set this state:");
                let starts: Vec<u64> = frames.iter().map(|f| f.0).collect();
                nc_bench::print_scanout(clock, &presented, &starts);
                println!("-- start = primitive prepare (the draw itself) began:");
                let starts: Vec<u64> = frames.iter().map(|f| f.1).collect();
                nc_bench::print_scanout(clock, &presented, &starts);
            }
            None => println!("no wp_presentation: scanout timing unavailable"),
        }
        // Proxies must go before iced closes the display (see nc-bench window.rs).
        *side = None;
    }
}

struct Canvas {
    doc: Arc<Document>,
    t: f64,
    redraw_ns: u64,
    recording: bool,
}

impl<M> shader::Program<M> for Canvas {
    type State = ();
    type Primitive = Frame;

    fn draw(&self, _: &(), _: mouse::Cursor, _: Rectangle) -> Frame {
        Frame {
            doc: self.doc.clone(),
            t: self.t,
            redraw_ns: self.redraw_ns,
            recording: self.recording,
        }
    }
}

#[derive(Debug)]
struct Frame {
    doc: Arc<Document>,
    t: f64,
    redraw_ns: u64,
    recording: bool,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    offset: [f32; 2],
    scale: f32,
    px_per_pt: f32,
    size: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SegmentInstance {
    a: [f32; 2],
    b: [f32; 2],
    half_width: f32,
    color: [f32; 4],
}

const SEGMENT_ATTRS: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: offset_of!(SegmentInstance, a) as u64,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: offset_of!(SegmentInstance, b) as u64,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32,
        offset: offset_of!(SegmentInstance, half_width) as u64,
        shader_location: 2,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: offset_of!(SegmentInstance, color) as u64,
        shader_location: 3,
    },
];

struct Pipeline {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    grid: wgpu::RenderPipeline,
    strokes: wgpu::RenderPipeline,
    instances: wgpu::Buffer,
    capacity: u64,
    uploaded: u64,
}

impl Pipeline {
    fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("segments"),
            size: capacity * size_of::<SegmentInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../nc-render/src/canvas.wgsl").into(),
            ),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = |label, vs, fs, buffers: &[wgpu::VertexBufferLayout], blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            })
        };
        let grid = pipeline("grid", "vs_grid", "fs_grid", &[], None);
        let strokes = pipeline(
            "strokes",
            "vs_seg",
            "fs_seg",
            &[wgpu::VertexBufferLayout {
                array_stride: size_of::<SegmentInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &SEGMENT_ATTRS,
            }],
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let capacity = 4096;
        Self {
            uniform,
            bind_group,
            grid,
            strokes,
            instances: Self::instance_buffer(device, capacity),
            capacity,
            uploaded: 0,
        }
    }
}

impl shader::Primitive for Frame {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        p: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let t0 = Instant::now();
        let prepare_ns = mono_ns();
        // Same script as `nc-bench window panzoom`.
        let size = Vec2::new(bounds.width, bounds.height);
        let center = scene::S2K_EXTENT * 0.5;
        let angle = (self.t / 10.0 * std::f64::consts::TAU) as f32;
        let focus = center + Vec2::new(angle.cos(), angle.sin()) * 1_500.0;
        let scale = (4f32.ln() * (self.t / 6.0 * std::f64::consts::TAU).sin() as f32).exp();
        let u = ViewUniform {
            offset: (size * 0.5 - focus * scale).to_array(),
            scale,
            px_per_pt: viewport.scale_factor(),
            size: size.to_array(),
            _pad: [0.0; 2],
        };
        queue.write_buffer(&p.uniform, 0, bytemuck::bytes_of(&u));

        // Same walk as nc-render's `Renderer::sync`.
        let total = self.doc.segments().count() as u64;
        if total > p.capacity {
            p.capacity = total.next_power_of_two();
            p.instances = Pipeline::instance_buffer(device, p.capacity);
            p.uploaded = 0;
        }
        let tail: Vec<SegmentInstance> = self
            .doc
            .segments()
            .skip(p.uploaded as usize)
            .map(|s| SegmentInstance {
                a: s.a.to_array(),
                b: s.b.to_array(),
                half_width: s.half_width,
                color: s.color,
            })
            .collect();
        if !tail.is_empty() {
            let at = p.uploaded * size_of::<SegmentInstance>() as u64;
            queue.write_buffer(&p.instances, at, bytemuck::cast_slice(&tail));
        }
        p.uploaded = total;

        if self.recording {
            let mut frames = FRAMES.lock().unwrap();
            if let Some(side) = SIDE.lock().unwrap().as_ref() {
                // Prepare runs inside iced's present, before `frame.present()`: the next commit.
                side.request_feedback(frames.len() as u64);
            }
            frames.push((self.redraw_ns, prepare_ns, t0.elapsed().as_secs_f64() * 1e3));
        }
    }

    fn draw(&self, p: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pass.set_bind_group(0, &p.bind_group, &[]);
        pass.set_pipeline(&p.grid);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&p.strokes);
        pass.set_vertex_buffer(0, p.instances.slice(..));
        pass.draw(0..6, 0..p.uploaded as u32);
        true
    }
}
