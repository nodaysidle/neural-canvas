//! GPU canvas renderer. Owns no window: it draws into any `TextureView`, so the same code serves the
//! window surface, offscreen tests and PNG export.

pub mod ui;

use bytemuck::{Pod, Zeroable};
use nc_core::document::{element_segments, stroke_segment_count, stroke_segments_from};
use nc_core::kurbo::Affine;
use nc_core::recognize::draw::{Mark, Painted, painted_segments};
use nc_core::{Document, Element, ElementId, ElementKind, Segment, Stroke, Viewport};
use std::collections::{HashMap, HashSet};
use std::mem::{offset_of, size_of};
use std::ops::Range;
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    offset: [f32; 2],
    scale: f32,
    px_per_pt: f32,
    size: [f32; 2],
    /// Top-left of the pane being drawn, physical px (side-by-side draws two panes).
    origin: [f32; 2],
    /// Affine for lifted segments, kurbo coefficient order.
    lift: [f32; 6],
    /// Opacity of the raw sketch and of the wireframe.
    alpha: [f32; 2],
    /// Canvas background (linear RGB) and 1 for the light theme.
    background: [f32; 4],
    /// Grid line colour (linear RGB), and 1 to draw the grid.
    grid: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SegmentInstance {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub half_width: f32,
    pub color: [f32; 4],
    /// [`LIFTED`], [`WIRE`] or [`CHROME`]; 0 for ordinary ink.
    pub flags: u32,
}

// Offsets come from the compiler, never hand-typed (Mac bug #8: wrong vertex attribute offset).
const SEGMENT_ATTRS: [wgpu::VertexAttribute; 5] = [
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
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint32,
        offset: offset_of!(SegmentInstance, flags) as u64,
        shader_location: 4,
    },
];

/// The canvas's colours. Ink is stored as drawn on the dark canvas; the light theme shows it with its
/// lightness inverted (`nc_core::theme::ink_on_light`, mirrored in the shader).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasTheme {
    pub light: bool,
    /// Linear RGB.
    pub background: [f32; 3],
    pub grid: [f32; 3],
}

impl CanvasTheme {
    pub const DARK: Self = Self {
        light: false,
        background: [0.012, 0.013, 0.018],
        grid: [0.035, 0.038, 0.05],
    };
    pub const LIGHT: Self = Self {
        light: true,
        background: [0.89, 0.89, 0.87],
        grid: [0.74, 0.75, 0.76],
    };

    /// The background as RGBA, for exports that should look like the canvas.
    pub fn background_rgba(&self) -> [f32; 4] {
        let [r, g, b] = self.background;
        [r, g, b, 1.0]
    }
}

/// The dark canvas background, as a clear colour.
pub const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.012,
    g: 0.013,
    b: 0.018,
    a: 1.0,
};

/// The stroke under the pen. `serial` changes for every new stroke, so the renderer knows to restart its
/// append-only upload.
#[derive(Clone, Copy)]
pub struct LiveStroke<'a> {
    pub serial: u64,
    pub stroke: &'a Stroke,
}

/// Part of the lifted selection: drawn through the lift transform.
pub const LIFTED: u32 = 1;
/// Part of the wireframe: drawn at the wireframe's opacity, not the sketch's.
pub const WIRE: u32 = 2;
/// Selection and other chrome: always opaque, colours used as given in both themes.
pub const CHROME: u32 = 4;
/// Ink drawn over everything (the live stroke, predicted ink, a shape being dragged): always opaque,
/// lightness-inverted on the light canvas like all ink.
pub const INK_ON_TOP: u32 = 8;
/// A filled rounded box from `a` (top-left) to `b` (bottom-right), `half_width` its corner radius.
pub const FILL: u32 = 16;
/// With [`FILL`]: the ellipse inscribed in the box instead.
pub const ELLIPSE: u32 = 32;

/// How much of the raw sketch and of the wireframe to show, 0..=1 each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    pub raw: f32,
    pub wire: f32,
}

impl Blend {
    pub const RAW: Self = Self {
        raw: 1.0,
        wire: 0.0,
    };
    pub const WIRE: Self = Self {
        raw: 0.0,
        wire: 1.0,
    };
}

/// The wireframe overlay, bottom to top. `serial` changes whenever it does, so it uploads once.
#[derive(Clone, Copy)]
pub struct WireLayer<'a> {
    pub serial: u64,
    pub painted: &'a [Painted],
}

/// Elements being moved or resized: drawn where `transform` puts them, while the document still holds
/// them where they were at the press. Only the uniform changes per frame, however many are lifted.
#[derive(Clone, Copy)]
pub struct Lifted<'a> {
    pub ids: &'a HashSet<ElementId>,
    pub transform: Affine,
}

/// Everything one frame draws, bottom to top.
#[derive(Clone, Copy)]
pub struct Scene<'a> {
    pub doc: &'a Document,
    pub live: Option<LiveStroke<'a>>,
    pub lifted: Option<Lifted<'a>>,
    pub wire: Option<WireLayer<'a>>,
    pub blend: Blend,
    /// Raw sketch on the left half, wireframe on the right, same view in each.
    pub split: bool,
    /// Ink drawn on top: predicted ink, a shape being dragged.
    pub ink_overlay: &'a [Segment],
    /// World-space chrome (selection, marquee, highlights), on top of everything.
    pub overlay: &'a [Segment],
    pub theme: CanvasTheme,
    /// Show the background grid (a preference).
    pub grid: bool,
}

impl<'a> Scene<'a> {
    pub fn doc(doc: &'a Document) -> Self {
        Self {
            doc,
            live: None,
            lifted: None,
            wire: None,
            blend: Blend::RAW,
            split: false,
            ink_overlay: &[],
            overlay: &[],
            theme: CanvasTheme::DARK,
            grid: true,
        }
    }
}

/// What the last frame cost, for tests and the bench.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Instanced draws for committed strokes (one per contiguous run in z-order).
    pub stroke_draws: u32,
    /// Arena slots in use, including holes left by erased or edited strokes.
    pub arena_segments: u32,
    pub holes: u32,
    /// Full re-uploads of the arena since the renderer was created.
    pub rebuilds: u64,
    /// Segments uploaded for the live stroke in the last frame.
    pub live_uploaded: u32,
    /// Segments uploaded for edited elements in the last frame.
    pub edited_uploaded: u32,
}

/// A committed stroke's segments in the arena. Valid while the document still holds this exact `Arc`:
/// every edit replaces the `Arc`, so pointer identity is the element's revision.
struct Cached {
    element: Arc<Element>,
    start: u32,
    count: u32,
    seen: u64,
    lifted: bool,
}

/// Compact when z-order is split into more draws than this.
const MAX_RUNS: usize = 64;
const MIN_ARENA: u32 = 4096;

pub struct Renderer {
    /// One view uniform per pane: side-by-side draws the scene twice.
    panes: [(wgpu::Buffer, wgpu::BindGroup); 2],
    grid: wgpu::RenderPipeline,
    strokes: wgpu::RenderPipeline,
    /// Every committed stroke's segments, one contiguous range per stroke.
    arena: wgpu::Buffer,
    arena_capacity: u32,
    arena_len: u32,
    holes: u32,
    cache: HashMap<ElementId, Cached>,
    frame: u64,
    runs: Vec<Range<u32>>,
    rebuilds: u64,
    /// The stroke being drawn: only its new tail is uploaded each frame.
    live: wgpu::Buffer,
    live_capacity: u32,
    live_serial: Option<u64>,
    live_len: u32,
    live_uploaded: u32,
    edited_uploaded: u32,
    /// Chrome and predicted ink, re-sent every frame (a few hundred segments at most).
    overlay: wgpu::Buffer,
    overlay_capacity: u32,
    overlay_len: u32,
    wire: wgpu::Buffer,
    wire_capacity: u32,
    wire_len: u32,
    wire_serial: Option<u64>,
    scratch: Vec<SegmentInstance>,
}

fn instance(s: Segment) -> SegmentInstance {
    SegmentInstance {
        a: s.a.to_array(),
        b: s.b.to_array(),
        half_width: s.half_width,
        color: s.color,
        flags: 0,
    }
}

fn flagged(flags: u32) -> impl Fn(Segment) -> SegmentInstance {
    move |s| SegmentInstance {
        flags,
        ..instance(s)
    }
}

/// Appends the element's capsules, flagged as part of the lifted selection or not.
fn push_instances(out: &mut Vec<SegmentInstance>, element: &Element, lifted: bool) {
    element_segments(element, |s| {
        out.push(flagged(if lifted { LIFTED } else { 0 })(s))
    });
}

fn push_run(runs: &mut Vec<Range<u32>>, start: u32, count: u32) {
    match runs.last_mut() {
        Some(last) if last.end == start => last.end += count,
        _ => runs.push(start..start + count),
    }
}

const STRIDE: u64 = size_of::<SegmentInstance>() as u64;

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("canvas.wgsl"));
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
        let pane = || {
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("view"),
                size: size_of::<ViewUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("view"),
                layout: &bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            });
            (uniform, bind_group)
        };
        let panes = [pane(), pane()];
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = |label, vs, fs, buffers: &[Option<wgpu::VertexBufferLayout>], blend| {
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
                multiview_mask: None,
                cache: None,
            })
        };
        let grid = pipeline("grid", "vs_grid", "fs_grid", &[], None);
        let strokes = pipeline(
            "strokes",
            "vs_seg",
            "fs_seg",
            &[Some(wgpu::VertexBufferLayout {
                array_stride: STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &SEGMENT_ATTRS,
            })],
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        Self {
            panes,
            grid,
            strokes,
            arena: Self::instance_buffer(device, MIN_ARENA),
            arena_capacity: MIN_ARENA,
            arena_len: 0,
            holes: 0,
            cache: HashMap::new(),
            frame: 0,
            runs: Vec::new(),
            rebuilds: 0,
            live: Self::instance_buffer(device, 1024),
            live_capacity: 1024,
            live_serial: None,
            live_len: 0,
            live_uploaded: 0,
            edited_uploaded: 0,
            overlay: Self::instance_buffer(device, 256),
            overlay_capacity: 256,
            overlay_len: 0,
            wire: Self::instance_buffer(device, 1024),
            wire_capacity: 1024,
            wire_len: 0,
            wire_serial: None,
            scratch: Vec::new(),
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("segments"),
            size: capacity as u64 * STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn stats(&self) -> RenderStats {
        RenderStats {
            stroke_draws: self.runs.len() as u32,
            arena_segments: self.arena_len,
            holes: self.holes,
            rebuilds: self.rebuilds,
            live_uploaded: self.live_uploaded,
            edited_uploaded: self.edited_uploaded,
        }
    }

    /// Brings the arena in line with `doc`: uploads only elements that are new or edited since the last
    /// frame, and lists the draws in z-order. O(elements) per frame, O(points) only for what changed. An
    /// edit that keeps the segment count (a move, a resize) is rewritten in place, so it leaves no hole.
    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        lifted: Option<&HashSet<ElementId>>,
    ) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("Renderer::sync");
        self.frame += 1;
        self.runs.clear();
        self.edited_uploaded = 0;
        let mut overflow = false;
        for element in doc.elements() {
            if matches!(element.kind, ElementKind::Recognized(_)) {
                continue;
            }
            let lift = lifted.is_some_and(|ids| ids.contains(&element.id));
            let (start, count) = match self.cache.get_mut(&element.id) {
                Some(c) if Arc::ptr_eq(&c.element, element) && c.lifted == lift => {
                    c.seen = self.frame;
                    (c.start, c.count)
                }
                cached => {
                    let old = cached.map(|c| (c.start, c.count));
                    self.scratch.clear();
                    push_instances(&mut self.scratch, element, lift);
                    let count = self.scratch.len() as u32;
                    let start = match old {
                        Some((start, n)) if n == count => start,
                        _ if self.arena_len + count > self.arena_capacity => {
                            overflow = true;
                            break;
                        }
                        _ => {
                            self.holes += old.map_or(0, |(_, n)| n);
                            self.arena_len += count;
                            self.arena_len - count
                        }
                    };
                    queue.write_buffer(
                        &self.arena,
                        start as u64 * STRIDE,
                        bytemuck::cast_slice(&self.scratch),
                    );
                    self.edited_uploaded += count;
                    self.cache.insert(
                        element.id,
                        Cached {
                            element: element.clone(),
                            start,
                            count,
                            seen: self.frame,
                            lifted: lift,
                        },
                    );
                    (start, count)
                }
            };
            push_run(&mut self.runs, start, count);
        }
        let frame = self.frame;
        let mut freed = 0;
        self.cache.retain(|_, c| {
            let keep = c.seen == frame;
            if !keep {
                freed += c.count;
            }
            keep
        });
        self.holes += freed;
        if overflow || self.holes * 2 > self.arena_len.max(MIN_ARENA) || self.runs.len() > MAX_RUNS
        {
            self.rebuild(device, queue, doc, lifted);
        }
    }

    /// Re-uploads every stroke contiguously in z-order: one draw, no holes.
    fn rebuild(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        lifted: Option<&HashSet<ElementId>>,
    ) {
        self.cache.clear();
        self.scratch.clear();
        self.runs.clear();
        for element in doc.elements() {
            if !matches!(element.kind, ElementKind::Recognized(_)) {
                let lift = lifted.is_some_and(|ids| ids.contains(&element.id));
                let start = self.scratch.len() as u32;
                push_instances(&mut self.scratch, element, lift);
                let count = self.scratch.len() as u32 - start;
                self.cache.insert(
                    element.id,
                    Cached {
                        element: element.clone(),
                        start,
                        count,
                        seen: self.frame,
                        lifted: lift,
                    },
                );
            }
        }
        let total = self.scratch.len() as u32;
        // Room to append half as much again before the next rebuild.
        let needed = (total + total / 2).max(MIN_ARENA).next_power_of_two();
        if needed > self.arena_capacity {
            self.arena_capacity = needed;
            self.arena = Self::instance_buffer(device, needed);
        }
        if total > 0 {
            queue.write_buffer(&self.arena, 0, bytemuck::cast_slice(&self.scratch));
            self.runs.push(0..total);
        }
        self.arena_len = total;
        self.holes = 0;
        self.rebuilds += 1;
    }

    fn sync_live(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, live: Option<LiveStroke>) {
        let Some(live) = live else {
            self.live_serial = None;
            self.live_len = 0;
            self.live_uploaded = 0;
            return;
        };
        if self.live_serial != Some(live.serial) {
            self.live_serial = Some(live.serial);
            self.live_len = 0;
        }
        let total = stroke_segment_count(live.stroke) as u32;
        if total > self.live_capacity {
            self.live_capacity = total.next_power_of_two();
            self.live = Self::instance_buffer(device, self.live_capacity);
            self.live_len = 0;
        }
        // The last uploaded segment is sent again: a lone dot becomes a real segment once a second point
        // arrives.
        let from = self.live_len.saturating_sub(1).min(total);
        self.scratch.clear();
        self.scratch
            .extend(stroke_segments_from(live.stroke, from as usize).map(flagged(INK_ON_TOP)));
        if !self.scratch.is_empty() {
            queue.write_buffer(
                &self.live,
                from as u64 * STRIDE,
                bytemuck::cast_slice(&self.scratch),
            );
        }
        self.live_uploaded = self.scratch.len() as u32;
        self.live_len = total;
    }

    fn sync_overlay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ink: &[Segment],
        overlay: &[Segment],
    ) {
        let n = (ink.len() + overlay.len()) as u32;
        if n > self.overlay_capacity {
            self.overlay_capacity = n.next_power_of_two();
            self.overlay = Self::instance_buffer(device, self.overlay_capacity);
        }
        self.scratch.clear();
        self.scratch
            .extend(ink.iter().copied().map(flagged(INK_ON_TOP)));
        self.scratch
            .extend(overlay.iter().copied().map(flagged(CHROME)));
        if n > 0 {
            queue.write_buffer(&self.overlay, 0, bytemuck::cast_slice(&self.scratch));
        }
        self.overlay_len = n;
    }

    fn sync_wire(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, wire: Option<WireLayer>) {
        let Some(wire) = wire else {
            self.wire_len = 0;
            self.wire_serial = None;
            return;
        };
        if self.wire_serial == Some(wire.serial) {
            return;
        }
        // Painter's order: each mark's fill, then its outline.
        self.scratch.clear();
        let mut segments = Vec::new();
        for p in wire.painted {
            let fill = match (p.mark, p.fill) {
                (Mark::Rect { rect, radius }, Some(c)) => Some((rect, radius, c, FILL)),
                (Mark::Ellipse { rect }, Some(c)) => Some((rect, 0.0, c, FILL | ELLIPSE)),
                _ => None,
            };
            if let Some((r, radius, color, kind)) = fill {
                self.scratch.push(SegmentInstance {
                    a: [r.x0 as f32, r.y0 as f32],
                    b: [r.x1 as f32, r.y1 as f32],
                    half_width: radius,
                    color,
                    flags: WIRE | kind,
                });
            }
            segments.clear();
            painted_segments(p, &mut segments);
            self.scratch
                .extend(segments.iter().copied().map(flagged(WIRE)));
        }
        let n = self.scratch.len() as u32;
        if n > self.wire_capacity {
            self.wire_capacity = n.next_power_of_two();
            self.wire = Self::instance_buffer(device, self.wire_capacity);
        }
        if n > 0 {
            queue.write_buffer(&self.wire, 0, bytemuck::cast_slice(&self.scratch));
        }
        self.wire_len = n;
        self.wire_serial = Some(wire.serial);
    }

    /// Draws the grid and `scene` into `target`. `size` is logical px; `px_per_pt` the surface scale
    /// factor.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scene: &Scene,
        viewport: &Viewport,
        size: [f32; 2],
        px_per_pt: f32,
    ) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("Renderer::render");
        self.sync(device, queue, scene.doc, scene.lifted.map(|l| l.ids));
        self.sync_live(device, queue, scene.live);
        self.sync_overlay(device, queue, scene.ink_overlay, scene.overlay);
        self.sync_wire(device, queue, scene.wire);
        let lift = scene
            .lifted
            .map_or(Affine::IDENTITY, |l| l.transform)
            .as_coeffs()
            .map(|v| v as f32);
        // Side by side: the same view in each half, raw sketch left, wireframe right.
        let half = [size[0] / 2.0, size[1]];
        let panes: &[([f32; 2], [f32; 2], Blend)] = if scene.split {
            &[
                ([0.0, 0.0], half, Blend::RAW),
                ([half[0], 0.0], half, Blend::WIRE),
            ]
        } else {
            &[([0.0, 0.0], size, scene.blend)]
        };
        for (i, (origin, pane_size, blend)) in panes.iter().enumerate() {
            let u = ViewUniform {
                offset: viewport.offset.to_array(),
                scale: viewport.scale,
                px_per_pt,
                size: *pane_size,
                origin: origin.map(|v| v * px_per_pt),
                lift,
                alpha: [blend.raw, blend.wire],
                background: {
                    let [r, g, b] = scene.theme.background;
                    [r, g, b, scene.theme.light as u32 as f32]
                },
                grid: {
                    let [r, g, b] = scene.theme.grid;
                    [r, g, b, scene.grid as u32 as f32]
                },
            };
            queue.write_buffer(&self.panes[i].0, 0, bytemuck::bytes_of(&u));
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear({
                        let [r, g, b] = scene.theme.background;
                        wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: 1.0,
                        }
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        for (i, (origin, pane_size, _)) in panes.iter().enumerate() {
            let (x, y) = (origin[0] * px_per_pt, origin[1] * px_per_pt);
            let (w, h) = (pane_size[0] * px_per_pt, pane_size[1] * px_per_pt);
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_scissor_rect(x as u32, y as u32, w.round() as u32, h.round() as u32);
            pass.set_bind_group(0, &self.panes[i].1, &[]);
            pass.set_pipeline(&self.grid);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.strokes);
            if !self.runs.is_empty() {
                pass.set_vertex_buffer(0, self.arena.slice(..));
                for run in &self.runs {
                    pass.draw(0..6, run.clone());
                }
            }
            for (buffer, len) in [
                (&self.wire, self.wire_len),
                (&self.live, self.live_len),
                (&self.overlay, self.overlay_len),
            ] {
                if len > 0 {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..6, 0..len);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
