//! Headless renderer cost vs scene size at 2560×1440: picks the tile-cache threshold.

use glam::Vec2;
use nc_bench::stats::Summary;
use nc_core::{Viewport, scene};
use nc_render::Renderer;
use std::time::Instant;

const W: u32 = 2560;
const H: u32 = 1440;

pub fn run(stroke_counts: &[usize], frames: usize) {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("no GPU adapter");
    let ts = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let features = adapter.features() & ts;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        ..Default::default()
    }))
    .expect("request device");
    let info = adapter.get_info();
    println!(
        "adapter: {} ({:?}, {})",
        info.name, info.backend, info.driver_info
    );
    println!(
        "target {W}x{H} Bgra8UnormSrgb, {frames} frames per row, GPU timestamps: {}",
        features == ts
    );

    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let timer = (features == ts).then(|| {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: None,
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        (set, resolve, read)
    });
    let period_ns = queue.get_timestamp_period() as f64;

    let size = Vec2::new(W as f32, H as f32);
    for &n in stroke_counts {
        let doc = scene::scene(1, n, scene::S2K_POINTS_PER_STROKE);
        let mut renderer = Renderer::new(&device, format);
        let fit = size.x / scene::S2K_EXTENT.x;
        for (name, scale) in [("fit (all visible)", fit), ("100%", 1.0)] {
            let (mut cpu, mut wall, mut gpu) = (vec![], vec![], vec![]);
            for f in 0..frames + 10 {
                // Small pan each frame so nothing can be cached by accident.
                let center = scene::S2K_EXTENT * 0.5 + Vec2::splat(f as f32);
                let viewport = Viewport {
                    scale,
                    offset: size * 0.5 - center * scale,
                };
                let t0 = Instant::now();
                let mut enc = device.create_command_encoder(&Default::default());
                if let Some((set, ..)) = &timer {
                    enc.write_timestamp(set, 0);
                }
                renderer.render(
                    &device,
                    &queue,
                    &mut enc,
                    &view,
                    &doc,
                    &viewport,
                    size.to_array(),
                    1.0,
                );
                if let Some((set, resolve, read)) = &timer {
                    enc.write_timestamp(set, 1);
                    enc.resolve_query_set(set, 0..2, resolve, 0);
                    enc.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
                }
                let t_cpu = t0.elapsed();
                queue.submit([enc.finish()]);
                if let Some((_, _, read)) = &timer {
                    read.slice(..)
                        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
                }
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let t_wall = t0.elapsed();
                if let Some((_, _, read)) = &timer {
                    let ticks: [u64; 2] =
                        bytemuck::pod_read_unaligned(&read.slice(..).get_mapped_range().unwrap());
                    read.unmap();
                    if f >= 10 {
                        gpu.push(ticks[1].saturating_sub(ticks[0]) as f64 * period_ns / 1e6);
                    }
                }
                if f >= 10 {
                    cpu.push(t_cpu.as_secs_f64() * 1e3);
                    wall.push(t_wall.as_secs_f64() * 1e3);
                }
            }
            println!(
                "{n:>6} strokes / {:>7} points, {name}:",
                n * scene::S2K_POINTS_PER_STROKE
            );
            println!("    cpu encode+sync  {}", Summary::of(&cpu).unwrap());
            if let Some(s) = Summary::of(&gpu) {
                println!("    gpu              {s}");
            }
            println!("    submit→idle wall {}", Summary::of(&wall).unwrap());
        }
    }
}
