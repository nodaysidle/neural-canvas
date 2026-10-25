//! Interface chrome on top of the canvas: flat rounded rectangles, sketch thumbnails, and text in the
//! NODAYSIDLE faces (Manrope for interface text, DM Mono for numbers and shortcuts, both SIL OFL 1.1,
//! bundled). Everything is in logical px; text is shaped at the device scale, so it stays crisp at
//! fractional scales.

use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use std::collections::HashMap;
use std::mem::{offset_of, size_of};

/// The bundled faces (`fonts/README.md`), also used by export to render wireframe labels.
pub mod fonts {
    pub const MANROPE: &[u8] = include_bytes!("../fonts/Manrope-Regular.ttf");
    pub const MANROPE_SEMIBOLD: &[u8] = include_bytes!("../fonts/Manrope-SemiBold.ttf");
    pub const DM_MONO: &[u8] = include_bytes!("../fonts/DMMono.ttf");
}
use fonts::{DM_MONO, MANROPE, MANROPE_SEMIBOLD};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    /// Manrope: interface text.
    Sans,
    /// Manrope semibold: titles and the active item.
    SansStrong,
    /// DM Mono: numbers, shortcuts, measurements.
    Mono,
}

/// A rectangle in logical px: x, y, width, height.
pub type Bounds = [f32; 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiRect {
    pub rect: Bounds,
    /// Linear RGBA.
    pub fill: [f32; 4],
    pub border: [f32; 4],
    pub radius: f32,
    /// 0 for no border.
    pub border_width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiText {
    pub text: String,
    /// Top-left of the first line, logical px.
    pub at: [f32; 2],
    pub size: f32,
    pub font: Font,
    /// Linear RGBA.
    pub color: [f32; 4],
    /// Wrap (and clip) at this width; `None` for one unbounded line.
    pub width: Option<f32>,
}

/// A registered image (see [`UiRenderer::image_target`]) drawn into `rect`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiImage {
    pub id: u64,
    pub rect: Bounds,
    pub radius: f32,
}

/// One stacking level: rectangles, then images, then text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiLayer {
    pub rects: Vec<UiRect>,
    pub images: Vec<UiImage>,
    pub texts: Vec<UiText>,
}

impl UiLayer {
    pub fn clear(&mut self) {
        self.rects.clear();
        self.images.clear();
        self.texts.clear();
    }
}

/// Chrome for one frame: canvas labels, the base layer, then popups (menus, toasts, overlays).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiScene {
    /// Things that belong to the canvas (wireframe labels): under all chrome.
    pub canvas: UiLayer,
    pub base: UiLayer,
    pub popup: UiLayer,
}

/// Measures text the way [`UiRenderer`] will draw it; layout code depends on this, tests fake it.
pub trait Measure {
    /// Width of one line of `text`, logical px.
    fn width(&mut self, text: &str, size: f32, font: Font) -> f32;
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4],
    fill: [f32; 4],
    border: [f32; 4],
    shape: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ImageInstance {
    rect: [f32; 4],
    radius: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct View {
    size: [f32; 2],
    px_per_pt: f32,
    _pad: f32,
}

const RECT_ATTRS: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: offset_of!(RectInstance, rect) as u64,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: offset_of!(RectInstance, fill) as u64,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: offset_of!(RectInstance, border) as u64,
        shader_location: 2,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: offset_of!(RectInstance, shape) as u64,
        shader_location: 3,
    },
];

const IMAGE_ATTRS: [wgpu::VertexAttribute; 2] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: offset_of!(ImageInstance, rect) as u64,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32,
        offset: offset_of!(ImageInstance, radius) as u64,
        shader_location: 1,
    },
];

/// Shaped text kept between frames: the chrome redraws with mostly the same strings.
#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    font: Font,
    /// Device px, as bits: f32 is not `Hash`.
    size: u32,
    width: Option<u32>,
}

struct Image {
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// Sketch thumbnails and other images are this format, so the canvas renderer can draw into them.
pub const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct UiRenderer {
    view_buffer: wgpu::Buffer,
    view_group: wgpu::BindGroup,
    image_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    rects: wgpu::RenderPipeline,
    images: wgpu::RenderPipeline,
    rect_buffer: wgpu::Buffer,
    rect_capacity: usize,
    image_buffer: wgpu::Buffer,
    image_capacity: usize,
    registered: HashMap<u64, Image>,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    text: [TextRenderer; LAYERS],
    buffers: HashMap<TextKey, (Buffer, u64)>,
    frame: u64,
    measured: HashMap<(String, Font, u32), f32>,
}

fn srgb_u8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

fn attrs(font: Font) -> Attrs<'static> {
    match font {
        Font::Sans => Attrs::new().family(Family::Name("Manrope")),
        Font::SansStrong => Attrs::new()
            .family(Family::Name("Manrope"))
            .weight(Weight::SEMIBOLD),
        Font::Mono => Attrs::new().family(Family::Name("DM Mono")),
    }
}

/// Line height for a font size: 1.35 for interface text.
fn line_height(size: f32) -> f32 {
    (size * 1.35).ceil()
}

const LAYERS: usize = 3;

/// Buffers unused for this many frames are dropped.
const KEEP_FRAMES: u64 = 120;

impl UiRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("ui.wgsl"));
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui view"),
            size: size_of::<View>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui view"),
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
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ui image"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline =
            |label, layouts: &[Option<&wgpu::BindGroupLayout>], vs, fs, stride, attrs| {
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: layouts,
                    immediate_size: 0,
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: stride,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: attrs,
                        })],
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
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let rects = pipeline(
            "ui rects",
            &[Some(&view_layout)],
            "vs_rect",
            "fs_rect",
            size_of::<RectInstance>() as u64,
            &RECT_ATTRS[..],
        );
        let images = pipeline(
            "ui images",
            &[Some(&view_layout), Some(&image_layout)],
            "vs_image",
            "fs_image",
            size_of::<ImageInstance>() as u64,
            &IMAGE_ATTRS[..],
        );
        let mut db = glyphon::fontdb::Database::new();
        db.load_font_data(MANROPE.to_vec());
        db.load_font_data(MANROPE_SEMIBOLD.to_vec());
        db.load_font_data(DM_MONO.to_vec());
        let font_system = FontSystem::new_with_locale_and_db("en-US".into(), db);
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = [
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None),
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None),
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None),
        ];
        let viewport = Viewport::new(device, &cache);
        Self {
            view_buffer,
            view_group,
            image_layout,
            sampler,
            rects,
            images,
            rect_buffer: Self::buffer(device, 256 * size_of::<RectInstance>()),
            rect_capacity: 256,
            image_buffer: Self::buffer(device, 64 * size_of::<ImageInstance>()),
            image_capacity: 64,
            registered: HashMap::new(),
            font_system,
            swash: SwashCache::new(),
            atlas,
            viewport,
            text,
            buffers: HashMap::new(),
            frame: 0,
            measured: HashMap::new(),
        }
    }

    fn buffer(device: &wgpu::Device, size: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui instances"),
            size: size as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// A texture to draw image `id` into (`width` × `height` device px), created or reused; draw into
    /// the returned view, then show it with a [`UiImage`] of the same id.
    pub fn image_target(
        &mut self,
        device: &wgpu::Device,
        id: u64,
        width: u32,
        height: u32,
    ) -> &wgpu::TextureView {
        let fresh = self
            .registered
            .get(&id)
            .is_none_or(|i| i.size != (width, height));
        if fresh {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ui image"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: IMAGE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui image"),
                layout: &self.image_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.registered.insert(
                id,
                Image {
                    view,
                    bind_group,
                    size: (width, height),
                },
            );
        }
        &self.registered[&id].view
    }

    pub fn has_image(&self, id: u64) -> bool {
        self.registered.contains_key(&id)
    }

    pub fn forget_image(&mut self, id: u64) {
        self.registered.remove(&id);
    }

    fn shaped(&mut self, t: &UiText, px_per_pt: f32) -> TextKey {
        let size = t.size * px_per_pt;
        let key = TextKey {
            text: t.text.clone(),
            font: t.font,
            size: size.to_bits(),
            width: t.width.map(|w| (w * px_per_pt).to_bits()),
        };
        let frame = self.frame;
        if let Some(entry) = self.buffers.get_mut(&key) {
            entry.1 = frame;
            return key;
        }
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(size, line_height(size)));
        buffer.set_size(t.width.map(|w| w * px_per_pt), None);
        buffer.set_text(&t.text, &attrs(t.font), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        self.buffers.insert(key.clone(), (buffer, frame));
        key
    }

    /// Draws `scene` over whatever `target` holds. `size` is logical px.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scene: &UiScene,
        size: [f32; 2],
        px_per_pt: f32,
    ) {
        #[cfg(feature = "profile")]
        let _span = tracy_client::span!("UiRenderer::render");
        self.frame += 1;
        let view = View {
            size,
            px_per_pt,
            _pad: 0.0,
        };
        queue.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(&view));
        let layers = [&scene.canvas, &scene.base, &scene.popup];

        // Instances for both layers in one buffer each; remember each layer's range.
        let mut rects = Vec::new();
        let mut images = Vec::new();
        let mut ranges: [(std::ops::Range<u32>, std::ops::Range<u32>); LAYERS] = Default::default();
        for (l, layer) in layers.iter().enumerate() {
            let (r0, i0) = (rects.len() as u32, images.len() as u32);
            rects.extend(layer.rects.iter().map(|r| RectInstance {
                rect: r.rect,
                fill: r.fill,
                border: r.border,
                shape: [r.radius, r.border_width],
            }));
            images.extend(
                layer
                    .images
                    .iter()
                    .filter(|i| self.registered.contains_key(&i.id))
                    .map(|i| ImageInstance {
                        rect: i.rect,
                        radius: i.radius,
                    }),
            );
            ranges[l] = (r0..rects.len() as u32, i0..images.len() as u32);
        }
        if rects.len() > self.rect_capacity {
            self.rect_capacity = rects.len().next_power_of_two();
            self.rect_buffer = Self::buffer(device, self.rect_capacity * size_of::<RectInstance>());
        }
        if images.len() > self.image_capacity {
            self.image_capacity = images.len().next_power_of_two();
            self.image_buffer =
                Self::buffer(device, self.image_capacity * size_of::<ImageInstance>());
        }
        if !rects.is_empty() {
            queue.write_buffer(&self.rect_buffer, 0, bytemuck::cast_slice(&rects));
        }
        if !images.is_empty() {
            queue.write_buffer(&self.image_buffer, 0, bytemuck::cast_slice(&images));
        }

        // Text: shape (cached), then prepare each layer's renderer.
        self.viewport.update(
            queue,
            Resolution {
                width: (size[0] * px_per_pt).round() as u32,
                height: (size[1] * px_per_pt).round() as u32,
            },
        );
        let keys: Vec<Vec<TextKey>> = layers
            .iter()
            .map(|layer| {
                layer
                    .texts
                    .iter()
                    .map(|t| self.shaped(t, px_per_pt))
                    .collect()
            })
            .collect();
        for (l, layer) in layers.iter().enumerate() {
            let areas = layer.texts.iter().zip(&keys[l]).map(|(t, key)| {
                let [r, g, b, a] = t.color;
                let left = t.at[0] * px_per_pt;
                let top = t.at[1] * px_per_pt;
                let right = t
                    .width
                    .map_or(i32::MAX, |w| (left + w * px_per_pt).ceil() as i32);
                TextArea {
                    buffer: &self.buffers[key].0,
                    left,
                    top,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: left.floor() as i32,
                        top: top.floor() as i32,
                        right,
                        bottom: i32::MAX,
                    },
                    default_color: Color::rgba(
                        srgb_u8(r),
                        srgb_u8(g),
                        srgb_u8(b),
                        (a * 255.0) as u8,
                    ),
                    custom_glyphs: &[],
                }
            });
            if let Err(e) = self.text[l].prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash,
            ) {
                eprintln!("neuralcanvas: interface text could not be prepared: {e}");
            }
        }
        let frame = self.frame;
        self.buffers
            .retain(|_, (_, used)| frame - *used < KEEP_FRAMES);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        for (l, layer) in layers.iter().enumerate() {
            let (rect_range, image_range) = ranges[l].clone();
            if !rect_range.is_empty() {
                pass.set_pipeline(&self.rects);
                pass.set_bind_group(0, &self.view_group, &[]);
                pass.set_vertex_buffer(0, self.rect_buffer.slice(..));
                pass.draw(0..6, rect_range);
            }
            if !image_range.is_empty() {
                pass.set_pipeline(&self.images);
                pass.set_bind_group(0, &self.view_group, &[]);
                pass.set_vertex_buffer(0, self.image_buffer.slice(..));
                let shown = layer
                    .images
                    .iter()
                    .filter(|i| self.registered.contains_key(&i.id));
                for (k, img) in image_range.zip(shown) {
                    pass.set_bind_group(1, &self.registered[&img.id].bind_group, &[]);
                    pass.draw(0..6, k..k + 1);
                }
            }
            if let Err(e) = self.text[l].render(&self.atlas, &self.viewport, &mut pass) {
                eprintln!("neuralcanvas: interface text could not be drawn: {e}");
            }
        }
        drop(pass);
        self.atlas.trim();
    }
}

impl Measure for UiRenderer {
    fn width(&mut self, text: &str, size: f32, font: Font) -> f32 {
        let key = (text.to_owned(), font, size.to_bits());
        if let Some(w) = self.measured.get(&key) {
            return *w;
        }
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(size, line_height(size)));
        buffer.set_size(None, None);
        buffer.set_text(text, &attrs(font), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let w = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max);
        if self.measured.len() > 4096 {
            self.measured.clear();
        }
        self.measured.insert(key, w);
        w
    }
}

/// Line height the renderer uses for `size`, for layout.
pub fn text_height(size: f32) -> f32 {
    line_height(size)
}
