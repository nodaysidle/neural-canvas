//! NeuralCanvas measurement harness (docs/PERFORMANCE-BUDGET.md). Dev tool: never packaged or shipped.
//!
//! ```text
//! nc-bench offscreen [frames]                    renderer cost vs scene size, headless, 2560x1440
//! nc-bench window <panzoom|draw|move|input> <fifo|mailbox|immediate> [secs] [latch] [recognize] [style=PNG] [shell]
//!                                                latch: `auto` (the app's adaptive schedule) or a fixed
//!                                                delay in ms after the frame callback (default 0)
//! nc-bench tablet [secs]                         log zwp_tablet_v2 vs winit pointer events
//! nc-bench drive <secs> [hz] [strokes] | script < commands            virtual pointer circling the output; `strokes` holds the
//!                                                left button to draw arcs (end-to-end app checks)
//! nc-bench samples                               per-sample cost of the input hot path
//! nc-bench style <out-dir> <screenshot...>       style mirror on real screenshots, side-by-side images
//! nc-bench store [dir]                           S2k save/load cost against a real file; with `dir`, keeps
//!                                                `dir/neuralcanvas.db` for the app to open
//! ```

mod drive;
mod fixtures;
mod offscreen;
mod samples;
mod store;
mod style;
mod window;

use window::{Config, Latch, Scenario};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    let secs = |i: usize, default: f64| arg(i).map_or(default, |s| s.parse().expect("seconds"));
    match arg(0) {
        Some("offscreen") => offscreen::run(
            &[1_000, 2_000, 5_000, 10_000, 20_000],
            arg(1).map_or(120, |s| s.parse().expect("frames")),
        ),
        Some("window") => {
            let scenario = match arg(1) {
                Some("panzoom") => Scenario::PanZoom,
                Some("draw") => Scenario::Draw,
                Some("move") => Scenario::Move,
                Some("input") => Scenario::Input,
                _ => usage(),
            };
            let present_mode = match arg(2) {
                Some("fifo") => wgpu::PresentMode::Fifo,
                Some("mailbox") => wgpu::PresentMode::Mailbox,
                Some("immediate") => wgpu::PresentMode::Immediate,
                _ => usage(),
            };
            window::run(Config {
                scenario,
                present_mode,
                secs: secs(3, 20.0),
                latch: match arg(4) {
                    Some("auto") => Latch::Adaptive,
                    _ => Latch::Fixed(std::time::Duration::from_secs_f64(secs(4, 0.0) / 1e3)),
                },
                recognize: args[5.min(args.len())..].iter().any(|a| a == "recognize"),
                style: args[5.min(args.len())..]
                    .iter()
                    .find_map(|a| a.strip_prefix("style="))
                    .map(Into::into),
                shell: args[5.min(args.len())..].iter().any(|a| a == "shell"),
            });
        }
        Some("recognize") => {
            // Recognition of S2k (budget: ≤ 300 ms on the worker), 10 runs.
            let doc = nc_core::scene::s2k(1);
            let mut ms = Vec::new();
            let mut wire = None;
            for _ in 0..10 {
                let t = std::time::Instant::now();
                wire = Some(nc_core::recognize::recognize(doc.elements()));
                ms.push(t.elapsed().as_secs_f64() * 1e3);
            }
            let wire = wire.expect("ran");
            println!(
                "S2k ({} elements): {}",
                doc.elements().len(),
                nc_bench::stats::Summary::of(&ms).expect("10 runs")
            );
            println!(
                "{} shapes, status {:?}, {:?}",
                wire.shapes.len(),
                wire.report.status(),
                wire.report
            );
        }
        Some("samples") => samples::run(1_000_000),
        Some("fixtures") => match arg(1) {
            Some("pages") => println!("{}", fixtures::pages()),
            Some("script") => fixtures::script(
                arg(2)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or_else(|| usage()),
                arg(3)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or_else(|| usage()),
                glam::Vec2::new(secs(4, 0.0) as f32, secs(5, 0.0) as f32),
                arg(6).unwrap_or_else(|| usage()).as_ref(),
            ),
            Some("extract") => fixtures::extract(
                arg(2).unwrap_or_else(|| usage()).as_ref(),
                arg(3).unwrap_or_else(|| usage()).as_ref(),
                arg(4).unwrap_or_else(|| usage()).as_ref(),
            ),
            _ => usage(),
        },
        Some("drive") if arg(1) == Some("script") => drive::script(std::io::stdin().lock()),
        Some("drive") => drive::run(secs(1, 10.0), secs(2, 500.0), arg(3) == Some("strokes")),
        Some("store") => match arg(1) {
            Some(dir) => store::run(dir.as_ref(), true),
            None => store::run(&std::env::temp_dir(), false),
        },
        Some("tablet") => window::run(Config {
            scenario: Scenario::Tablet,
            present_mode: wgpu::PresentMode::Fifo,
            secs: secs(1, 15.0),
            latch: Latch::Fixed(std::time::Duration::ZERO),
            recognize: false,
            style: None,
            shell: false,
        }),
        Some("style") if args.len() > 2 => style::run(args[1].as_ref(), &args[2..]),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: nc-bench offscreen [frames]\n       nc-bench window <panzoom|draw|move|input> <fifo|mailbox|immediate> [secs] [auto|late_ms] [recognize] [style=PNG] [shell]\n       nc-bench tablet [secs]\n       nc-bench drive <secs> [hz] [strokes] | script < commands\n       nc-bench fixtures <pages | script SEED PAGE X Y MANIFEST | extract MANIFEST DB DIR>\n       nc-bench recognize\n       nc-bench samples\n       nc-bench store [dir]\n       nc-bench style <out-dir> <screenshot...>"
    );
    std::process::exit(2);
}
