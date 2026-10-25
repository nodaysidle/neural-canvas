//! GPU canvas renderer. Owns no window: it draws into any `TextureView`, so the same code serves the
//! window surface, offscreen tests and PNG export.

use bytemuck::{Pod, Zeroable};
use nc_core::document::stroke_segments;
use nc_core::{Document, Segment, Stroke, Viewport};
use std::mem::{offset_of, size_of};

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

pub struct Renderer {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    grid: wgpu::RenderPipeline,
    strokes: wgpu::RenderPipeline,
    instances: wgpu::Buffer,
    capacity: u64,
    uploaded: u64,
    generation: u64,
    /// The stroke being drawn, rewritten every frame from `live_scratch` (no per-frame allocation).
    live: wgpu::Buffer,
    live_capacity: u64,
    live_count: u32,
    live_scratch: Vec<SegmentInstance>,
}

fn instance(s: Segment) -> SegmentInstance {
    SegmentInstance {
        a: s.a.to_array(),
        b: s.b.to_array(),
        half_width: s.half_width,
        color: s.color,
    }
}

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
                array_stride: size_of::<SegmentInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &SEGMENT_ATTRS,
            })],
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
            generation: u64::MAX,
            live: Self::instance_buffer(device, 1024),
            live_capacity: 1024,
            live_count: 0,
            live_scratch: Vec::new(),
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("segments"),
            size: capacity * size_of::<SegmentInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads only segments appended since the last call; rebuilds when `doc.generation` changes.
    // shortcut: walks the segment iterator from the start to find the tail (O(points) CPU per sync, ~0.2 ms
    // at 200k points); upgrade to per-stroke GPU caches + tile cache in Phase 2 (docs/PLAN.md §5).
    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, doc: &Document) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("Renderer::sync");
        if doc.generation() != self.generation {
            self.generation = doc.generation();
            self.uploaded = 0;
        }
        let total = doc.segments().count() as u64;
        if total > self.capacity {
            self.capacity = total.next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
            self.uploaded = 0;
        }
        if total < self.uploaded {
            self.uploaded = 0;
        }
        let tail: Vec<SegmentInstance> = doc
            .segments()
            .skip(self.uploaded as usize)
            .map(instance)
            .collect();
        if !tail.is_empty() {
            let at = self.uploaded * size_of::<SegmentInstance>() as u64;
            queue.write_buffer(&self.instances, at, bytemuck::cast_slice(&tail));
        }
        self.uploaded = total;
    }

    fn sync_live(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, live: Option<&Stroke>) {
        self.live_scratch.clear();
        if let Some(stroke) = live {
            self.live_scratch
                .extend(stroke_segments(stroke).map(instance));
        }
        let n = self.live_scratch.len() as u64;
        if n > self.live_capacity {
            self.live_capacity = n.next_power_of_two();
            self.live = Self::instance_buffer(device, self.live_capacity);
        }
        if n > 0 {
            queue.write_buffer(&self.live, 0, bytemuck::cast_slice(&self.live_scratch));
        }
        self.live_count = n as u32;
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
        live: Option<&Stroke>,
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
        if self.uploaded > 0 {
            pass.set_pipeline(&self.strokes);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..6, 0..self.uploaded as u32);
        }
        if self.live_count > 0 {
            pass.set_pipeline(&self.strokes);
            pass.set_vertex_buffer(0, self.live.slice(..));
            pass.draw(0..6, 0..self.live_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use nc_core::Element;
    use std::sync::Arc;

    #[test]
    fn segment_layout_is_tight() {
        assert_eq!(size_of::<SegmentInstance>(), 36);
        assert_eq!(offset_of!(SegmentInstance, color), 20);
        assert_eq!(size_of::<ViewUniform>() % 16, 0);
    }

    /// Renders a committed and a live stroke offscreen and checks ink lands at the top-left-origin position.
    #[test]
    fn draws_stroke_offscreen() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter; skipping offscreen render test");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let (w, h) = (64u32, 64u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut stroke = Stroke::new(Vec2::new(8.0, 16.0), 6.0, [1.0, 0.0, 0.0, 1.0]);
        stroke.push(Vec2::new(56.0, 16.0));
        let doc = Document::from_elements([Arc::new(Element::stroke(stroke))]);
        let mut live = Stroke::new(Vec2::new(8.0, 40.0), 6.0, [0.0, 1.0, 0.0, 1.0]);
        live.push(Vec2::new(56.0, 40.0));

        let mut r = Renderer::new(&device, format);
        let mut enc = device.create_command_encoder(&Default::default());
        r.render(
            &device,
            &queue,
            &mut enc,
            &view,
            &doc,
            Some(&live),
            &Viewport::default(),
            [w as f32, h as f32],
            1.0,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (256 * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let px = readback.slice(..).get_mapped_range().unwrap();
        let at = |x: u32, y: u32| &px[(y * 256 + x * 4) as usize..][..4];
        assert!(
            at(32, 16)[0] > 240 && at(32, 16)[1] < 20,
            "ink on the stroke: {:?}",
            at(32, 16)
        );
        assert!(
            at(32, 40)[1] > 240 && at(32, 40)[0] < 20,
            "live stroke drawn: {:?}",
            at(32, 40)
        );
        assert!(
            at(32, 56)[0] < 40 && at(32, 56)[1] < 40,
            "no ink far below the strokes (y-down): {:?}",
            at(32, 56)
        );
    }
}
