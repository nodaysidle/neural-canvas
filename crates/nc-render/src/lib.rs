//! GPU canvas renderer. Owns no window: it draws into any `TextureView`, so the same code serves the
//! window surface, offscreen tests and PNG export.

use bytemuck::{Pod, Zeroable};
use nc_core::document::{stroke_segment_count, stroke_segments, stroke_segments_from};
use nc_core::{Document, Element, ElementId, ElementKind, Segment, Stroke, Viewport};
use std::collections::HashMap;
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
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SegmentInstance {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub half_width: f32,
    pub color: [f32; 4],
}

// Offsets come from the compiler, never hand-typed (Mac bug #8: wrong vertex attribute offset).
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
}

/// A committed stroke's segments in the arena. Valid while the document still holds this exact `Arc`:
/// every edit replaces the `Arc`, so pointer identity is the element's revision.
struct Cached {
    element: Arc<Element>,
    start: u32,
    count: u32,
    seen: u64,
}

/// Compact when z-order is split into more draws than this.
const MAX_RUNS: usize = 64;
const MIN_ARENA: u32 = 4096;

pub struct Renderer {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
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
    scratch: Vec<SegmentInstance>,
}

fn instance(s: Segment) -> SegmentInstance {
    SegmentInstance {
        a: s.a.to_array(),
        b: s.b.to_array(),
        half_width: s.half_width,
        color: s.color,
    }
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
            uniform,
            bind_group,
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
        }
    }

    /// Brings the arena in line with `doc`: uploads only strokes that are new or edited since the last
    /// frame, and lists the draws in z-order. O(elements) per frame, O(points) only for what changed.
    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("Renderer::sync");
        self.frame += 1;
        self.runs.clear();
        let mut overflow = false;
        for element in doc.elements() {
            let ElementKind::Stroke(stroke) = &element.kind else {
                continue;
            };
            let current = match self.cache.get_mut(&element.id) {
                Some(c) if Arc::ptr_eq(&c.element, element) => Some(c),
                _ => None,
            };
            let (start, count) = match current {
                Some(c) => {
                    c.seen = self.frame;
                    (c.start, c.count)
                }
                None => {
                    let count = stroke_segment_count(stroke) as u32;
                    if self.arena_len + count > self.arena_capacity {
                        overflow = true;
                        break;
                    }
                    self.scratch.clear();
                    self.scratch.extend(stroke_segments(stroke).map(instance));
                    let start = self.arena_len;
                    queue.write_buffer(
                        &self.arena,
                        start as u64 * STRIDE,
                        bytemuck::cast_slice(&self.scratch),
                    );
                    self.arena_len += count;
                    let old = self.cache.insert(
                        element.id,
                        Cached {
                            element: element.clone(),
                            start,
                            count,
                            seen: self.frame,
                        },
                    );
                    self.holes += old.map_or(0, |o| o.count);
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
            self.rebuild(device, queue, doc);
        }
    }

    /// Re-uploads every stroke contiguously in z-order: one draw, no holes.
    fn rebuild(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document) {
        self.cache.clear();
        self.scratch.clear();
        self.runs.clear();
        for element in doc.elements() {
            if let ElementKind::Stroke(stroke) = &element.kind {
                let start = self.scratch.len() as u32;
                self.scratch.extend(stroke_segments(stroke).map(instance));
                let count = self.scratch.len() as u32 - start;
                self.cache.insert(
                    element.id,
                    Cached {
                        element: element.clone(),
                        start,
                        count,
                        seen: self.frame,
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
            .extend(stroke_segments_from(live.stroke, from as usize).map(instance));
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

    /// Draws grid, the document and the stroke being drawn (on top) into `target`. `size` is logical px;
    /// `px_per_pt` the surface scale factor.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        doc: &Document,
        live: Option<LiveStroke>,
        viewport: &Viewport,
        size: [f32; 2],
        px_per_pt: f32,
    ) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("Renderer::render");
        self.sync(device, queue, doc);
        self.sync_live(device, queue, live);
        let u = ViewUniform {
            offset: viewport.offset.to_array(),
            scale: viewport.scale,
            px_per_pt,
            size,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_pipeline(&self.grid);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&self.strokes);
        if !self.runs.is_empty() {
            pass.set_vertex_buffer(0, self.arena.slice(..));
            for run in &self.runs {
                pass.draw(0..6, run.clone());
            }
        }
        if self.live_len > 0 {
            pass.set_vertex_buffer(0, self.live.slice(..));
            pass.draw(0..6, 0..self.live_len);
        }
    }
}

#[cfg(test)]
mod tests;
