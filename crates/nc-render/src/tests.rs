//! Offscreen GPU tests. They skip (with a message) where no adapter exists; CI runs them on llvmpipe.

use super::*;
use glam::Vec2;
use nc_core::{Command, Session};
use std::path::PathBuf;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Gpu {
    fn new() -> Option<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU adapter; skipping offscreen render test");
            return None;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        Some(Self { device, queue })
    }

    fn frame(
        &self,
        r: &mut Renderer,
        doc: &Document,
        live: Option<LiveStroke>,
        viewport: &Viewport,
        logical: u32,
        scale: f32,
    ) -> Image {
        let scene = Scene {
            live,
            ..Scene::doc(doc)
        };
        self.frame_with(r, &scene, viewport, logical, scale)
    }

    /// Renders one frame at `logical` px × `scale` and returns tightly packed RGBA rows.
    fn frame_with(
        &self,
        r: &mut Renderer,
        scene: &Scene,
        viewport: &Viewport,
        logical: u32,
        scale: f32,
    ) -> Image {
        let px = (logical as f32 * scale).round() as u32;
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: px,
                height: px,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let row = (px * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * px) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        let size = px as f32 / scale;
        r.render(
            &self.device,
            &self.queue,
            &mut enc,
            &view,
            scene,
            viewport,
            [size, size],
            scale,
        );
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: px,
                height: px,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let mapped = readback.slice(..).get_mapped_range().unwrap();
        let mut rgba = Vec::with_capacity((px * px * 4) as usize);
        for y in 0..px {
            let at = (y * row) as usize;
            rgba.extend_from_slice(&mapped[at..at + (px * 4) as usize]);
        }
        Image { size: px, rgba }
    }
}

struct Image {
    size: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.size + x) * 4) as usize;
        self.rgba[i..i + 4].try_into().unwrap()
    }
}

fn stroke(from: Vec2, to: Vec2, width: f32, color: [f32; 4]) -> Arc<Element> {
    let mut s = Stroke::new(from, width, color);
    s.push(to);
    Arc::new(Element::stroke(s))
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

#[test]
fn segment_layout_is_tight() {
    assert_eq!(size_of::<SegmentInstance>(), 40);
    assert_eq!(offset_of!(SegmentInstance, color), 20);
    assert_eq!(size_of::<ViewUniform>() % 16, 0);
}

/// A committed and a live stroke land at their top-left-origin positions (Mac bug #11: y-down).
#[test]
fn draws_stroke_offscreen() {
    let Some(gpu) = Gpu::new() else { return };
    let doc = Document::from_elements([stroke(
        Vec2::new(8.0, 16.0),
        Vec2::new(56.0, 16.0),
        6.0,
        RED,
    )]);
    let mut live = Stroke::new(Vec2::new(8.0, 40.0), 6.0, GREEN);
    live.push(Vec2::new(56.0, 40.0));
    let mut r = Renderer::new(&gpu.device, FORMAT);
    let live = LiveStroke {
        serial: 1,
        stroke: &live,
    };
    let img = gpu.frame(&mut r, &doc, Some(live), &Viewport::default(), 64, 1.0);
    assert!(
        img.at(32, 16)[0] > 240 && img.at(32, 16)[1] < 20,
        "{:?}",
        img.at(32, 16)
    );
    assert!(
        img.at(32, 40)[1] > 240 && img.at(32, 40)[0] < 20,
        "{:?}",
        img.at(32, 40)
    );
    let below = img.at(32, 56);
    // Grid lines encode to ~50/255 in sRGB; ink is 240+.
    assert!(
        below[0] < 80 && below[1] < 80,
        "no ink below the strokes: {below:?}"
    );
}

/// Edits upload only what changed; an undo that restores a stroke into the middle keeps z-order.
#[test]
fn per_stroke_cache_follows_edits() {
    let Some(gpu) = Gpu::new() else { return };
    // Green crosses red, blue is elsewhere; z-order red < green < blue.
    let red = stroke(Vec2::new(32.0, 4.0), Vec2::new(32.0, 60.0), 6.0, RED);
    let green = stroke(Vec2::new(4.0, 32.0), Vec2::new(60.0, 32.0), 6.0, GREEN);
    let blue = stroke(Vec2::new(4.0, 52.0), Vec2::new(60.0, 52.0), 6.0, BLUE);
    let mut session = Session::new(
        ElementId::nil(),
        Document::from_elements([red.clone(), green.clone(), blue]),
    );
    let mut r = Renderer::new(&gpu.device, FORMAT);
    let v = Viewport::default();

    let img = gpu.frame(&mut r, session.doc(), None, &v, 64, 1.0);
    assert!(
        img.at(32, 32)[1] > 240,
        "green on top at the crossing: {:?}",
        img.at(32, 32)
    );
    assert_eq!(r.stats().stroke_draws, 1);
    let uploaded = r.stats().arena_segments;
    gpu.frame(&mut r, session.doc(), None, &v, 64, 1.0);
    assert_eq!(
        r.stats().arena_segments,
        uploaded,
        "an unchanged frame uploads nothing"
    );

    session
        .execute("Erase", Command::Remove { id: red.id })
        .unwrap();
    let img = gpu.frame(&mut r, session.doc(), None, &v, 64, 1.0);
    assert!(
        img.at(32, 12)[0] < 80,
        "erased stroke is gone: {:?}",
        img.at(32, 12)
    );
    assert_eq!(
        (r.stats().holes, r.stats().rebuilds),
        (1, 0),
        "erasing leaves a hole, no rebuild"
    );

    session.undo().unwrap();
    let img = gpu.frame(&mut r, session.doc(), None, &v, 64, 1.0);
    assert!(
        img.at(32, 12)[0] > 240,
        "undo brings it back: {:?}",
        img.at(32, 12)
    );
    assert!(
        img.at(32, 32)[1] > 240,
        "and still under green: {:?}",
        img.at(32, 32)
    );
    assert_eq!(r.stats().rebuilds, 0);
}

/// Shapes draw as ink, the overlay draws on top of everything, and a move rewrites strokes in place.
#[test]
fn shapes_overlay_and_moves() {
    let Some(gpu) = Gpu::new() else { return };
    let rect = Arc::new(Element::new(ElementKind::Shape(nc_core::Shape {
        kind: nc_core::ShapeKind::Rectangle,
        from: Vec2::new(8.0, 8.0),
        to: Vec2::new(56.0, 56.0),
        width: 4.0,
        color: RED,
    })));
    let line = stroke(Vec2::new(4.0, 32.0), Vec2::new(60.0, 32.0), 4.0, GREEN);
    let mut session = Session::new(
        ElementId::nil(),
        Document::from_elements([rect, line.clone()]),
    );
    let mut r = Renderer::new(&gpu.device, FORMAT);
    let v = Viewport::default();
    let overlay = [Segment {
        a: Vec2::new(32.0, 4.0),
        b: Vec2::new(32.0, 60.0),
        half_width: 2.0,
        color: BLUE,
    }];
    let scene = Scene {
        overlay: &overlay,
        ..Scene::doc(session.doc())
    };
    let img = gpu.frame_with(&mut r, &scene, &v, 64, 1.0);
    assert!(
        img.at(32, 8)[2] > 240,
        "overlay over the rect's top edge: {:?}",
        img.at(32, 8)
    );
    assert!(img.at(20, 8)[0] > 240, "rect top edge: {:?}", img.at(20, 8));
    assert!(
        img.at(56, 20)[0] > 240,
        "rect right edge: {:?}",
        img.at(56, 20)
    );
    assert!(
        img.at(20, 20)[0] < 80,
        "rect is an outline: {:?}",
        img.at(20, 20)
    );

    // Lifted (being dragged): drawn moved, the document untouched, uploaded once.
    let lifted_ids = HashSet::from([line.id]);
    for dy in [4.0, 12.0] {
        let scene = Scene {
            lifted: Some(Lifted {
                ids: &lifted_ids,
                transform: nc_core::kurbo::Affine::translate((0.0, dy)),
            }),
            ..Scene::doc(session.doc())
        };
        let img = gpu.frame_with(&mut r, &scene, &v, 64, 1.0);
        let y = 32 + dy as u32;
        assert!(img.at(20, y)[1] > 240, "lifted to {y}: {:?}", img.at(20, y));
        assert!(
            img.at(20, 32)[1] < 80,
            "not at the press: {:?}",
            img.at(20, 32)
        );
    }
    assert_eq!(
        r.stats().edited_uploaded,
        0,
        "following the drag uploads nothing"
    );

    let moved = nc_core::select::translated(session.doc(), &[line.id], Vec2::new(0.0, 12.0));
    session.execute("Move", moved).unwrap();
    let img = gpu.frame(&mut r, session.doc(), None, &v, 64, 1.0);
    assert!(img.at(20, 44)[1] > 240, "moved down: {:?}", img.at(20, 44));
    assert!(
        img.at(20, 32)[1] < 80,
        "gone from the old place: {:?}",
        img.at(20, 32)
    );
    let stats = r.stats();
    assert_eq!(
        (stats.holes, stats.rebuilds, stats.edited_uploaded),
        (0, 0, 1),
        "rewritten in place"
    );
}

/// The wireframe layer fades against the raw sketch, and side by side shows each in its own half.
#[test]
fn wireframe_blend_and_side_by_side() {
    let Some(gpu) = Gpu::new() else { return };
    // Raw ink: a red horizontal stroke at y = 16. Wireframe: a blue one at y = 48. Both span x 4..60.
    let doc = Document::from_elements([stroke(
        Vec2::new(4.0, 16.0),
        Vec2::new(60.0, 16.0),
        6.0,
        RED,
    )]);
    let wire = [Segment {
        a: Vec2::new(4.0, 48.0),
        b: Vec2::new(60.0, 48.0),
        half_width: 3.0,
        color: BLUE,
    }];
    let mut r = Renderer::new(&gpu.device, FORMAT);
    let v = Viewport::default();
    let frame = |r: &mut Renderer, blend, split| {
        let scene = Scene {
            wire: Some(WireLayer {
                serial: 1,
                segments: &wire,
            }),
            blend,
            split,
            ..Scene::doc(&doc)
        };
        gpu.frame_with(r, &scene, &v, 64, 1.0)
    };
    let raw = frame(&mut r, Blend::RAW, false);
    assert!(
        raw.at(32, 16)[0] > 240 && raw.at(32, 48)[2] < 80,
        "raw only"
    );
    let both = frame(
        &mut r,
        Blend {
            raw: 1.0,
            wire: 1.0,
        },
        false,
    );
    assert!(
        both.at(32, 16)[0] > 240 && both.at(32, 48)[2] > 240,
        "hybrid"
    );
    let wire_only = frame(&mut r, Blend::WIRE, false);
    assert!(
        wire_only.at(32, 16)[0] < 80 && wire_only.at(32, 48)[2] > 240,
        "wireframe only"
    );

    // Side by side: each 32 px half shows world x 0..32. Raw left, wireframe right.
    let split = frame(&mut r, Blend::RAW, true);
    assert!(
        split.at(16, 16)[0] > 240 && split.at(16, 48)[2] < 80,
        "left: raw"
    );
    assert!(
        split.at(48, 48)[2] > 240 && split.at(48, 16)[0] < 80,
        "right: wireframe"
    );
}

/// The live stroke uploads only its new tail, and a new stroke starts over.
#[test]
fn live_stroke_uploads_only_its_tail() {
    let Some(gpu) = Gpu::new() else { return };
    let doc = Document::default();
    let mut r = Renderer::new(&gpu.device, FORMAT);
    let v = Viewport::default();
    let mut live = Stroke::new(Vec2::new(4.0, 10.0), 4.0, RED);
    let mut uploads = Vec::new();
    for i in 1..40 {
        live.push(Vec2::new(4.0 + i as f32, 10.0 + (i % 3) as f32));
        let img = gpu.frame(
            &mut r,
            &doc,
            Some(LiveStroke {
                serial: 7,
                stroke: &live,
            }),
            &v,
            64,
            1.0,
        );
        uploads.push(r.stats().live_uploaded);
        if i == 39 {
            assert!(
                img.at(30, 11)[0] > 200,
                "live ink drawn: {:?}",
                img.at(30, 11)
            );
        }
    }
    assert!(
        uploads[1..].iter().all(|&n| n <= 2),
        "per-frame uploads {uploads:?}"
    );
    let fresh = Stroke::new(Vec2::new(50.0, 50.0), 4.0, GREEN);
    let img = gpu.frame(
        &mut r,
        &doc,
        Some(LiveStroke {
            serial: 8,
            stroke: &fresh,
        }),
        &v,
        64,
        1.0,
    );
    assert!(
        img.at(30, 11)[0] < 80,
        "the previous live stroke is gone: {:?}",
        img.at(30, 11)
    );
    assert!(
        img.at(50, 50)[1] > 200,
        "the new one is a dot: {:?}",
        img.at(50, 50)
    );
}

// ---- goldens (docs/PLAN.md §7: ±2/255 per channel on at most 0.1 % of pixels) ----

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.png"))
}

/// Compares against `tests/golden/<name>.png`; `NC_UPDATE_GOLDENS=1` rewrites it instead.
fn assert_golden(name: &str, img: &Image) {
    let path = golden_path(name);
    if std::env::var_os("NC_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(
            &path,
            &img.rgba,
            img.size,
            img.size,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        return;
    }
    let golden = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e} (run with NC_UPDATE_GOLDENS=1 to create)",
                path.display()
            )
        })
        .to_rgba8();
    assert_eq!(
        (golden.width(), golden.height()),
        (img.size, img.size),
        "{name}: size"
    );
    let off = golden
        .as_raw()
        .chunks(4)
        .zip(img.rgba.chunks(4))
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 2))
        .count();
    let limit = (img.size * img.size) as usize / 1000;
    assert!(off <= limit, "{name}: {off} pixels differ (limit {limit})");
}

fn render_one(gpu: &Gpu, doc: &Document, scale: f32) -> Image {
    let mut r = Renderer::new(&gpu.device, FORMAT);
    gpu.frame(&mut r, doc, None, &Viewport::default(), 96, scale)
}

const INK: [f32; 4] = [0.85, 0.87, 0.93, 1.0];

#[test]
fn golden_strokes() {
    let Some(gpu) = Gpu::new() else { return };
    let h = render_one(
        &gpu,
        &Document::from_elements([stroke(
            Vec2::new(16.0, 48.0),
            Vec2::new(80.0, 48.0),
            3.0,
            INK,
        )]),
        1.0,
    );
    let v = render_one(
        &gpu,
        &Document::from_elements([stroke(
            Vec2::new(48.0, 16.0),
            Vec2::new(48.0, 80.0),
            3.0,
            INK,
        )]),
        1.0,
    );
    let d = render_one(
        &gpu,
        &Document::from_elements([stroke(
            Vec2::new(16.0, 16.0),
            Vec2::new(80.0, 80.0),
            3.0,
            INK,
        )]),
        1.0,
    );
    assert_golden("stroke-horizontal", &h);
    assert_golden("stroke-vertical", &v);
    assert_golden("stroke-diagonal", &d);
    let d2 = render_one(
        &gpu,
        &Document::from_elements([stroke(
            Vec2::new(16.0, 16.0),
            Vec2::new(80.0, 80.0),
            3.0,
            INK,
        )]),
        2.0,
    );
    assert_golden("stroke-diagonal-2x", &d2);
}

/// Mac bug #10 guard: anti-aliasing does not depend on direction. The vertical stroke is the
/// horizontal one transposed (the grid behind them is symmetric about the diagonal too).
#[test]
fn antialiasing_is_direction_independent() {
    let Some(gpu) = Gpu::new() else { return };
    let h = render_one(
        &gpu,
        &Document::from_elements([stroke(
            Vec2::new(16.0, 47.3),
            Vec2::new(80.0, 47.3),
            2.2,
            INK,
        )]),
        1.0,
    );
    let v = render_one(
        &gpu,
        &Document::from_elements([stroke(
            Vec2::new(47.3, 16.0),
            Vec2::new(47.3, 80.0),
            2.2,
            INK,
        )]),
        1.0,
    );
    let worst = (0..h.size)
        .flat_map(|y| (0..h.size).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (a, b) = (h.at(x, y), v.at(y, x));
            (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap()
        })
        .max()
        .unwrap();
    assert!(worst <= 1, "transposed difference up to {worst}/255");
}

#[test]
fn golden_grid_at_fractional_scales() {
    let Some(gpu) = Gpu::new() else { return };
    for (scale, name) in [(1.0, "grid-1x"), (1.25, "grid-1.25x"), (2.0, "grid-2x")] {
        let img = render_one(&gpu, &Document::default(), scale);
        assert_golden(name, &img);
    }
}
