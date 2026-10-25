//! NeuralCanvas measurement harness (docs/PERFORMANCE-BUDGET.md). Dev tool: never packaged or shipped.
//!
//! ```text
//! nc-bench offscreen [frames]                    renderer cost vs scene size, headless, 2560x1440
//! nc-bench window <panzoom|draw|input> <fifo|mailbox|immediate> [secs] [latch]
//!                                                latch: `auto` (the app's adaptive schedule) or a fixed
//!                                                delay in ms after the frame callback (default 0)
//! nc-bench tablet [secs]                         log zwp_tablet_v2 vs winit pointer events
//! nc-bench drive <secs> [hz] [strokes]            virtual pointer circling the output; `strokes` holds the
//!                                                left button to draw arcs (end-to-end app checks)
//! nc-bench samples                               per-sample cost of the input hot path
//! nc-bench store [dir]                           S2k save/load cost against a real file; with `dir`, keeps
//!                                                `dir/neuralcanvas.db` for the app to open
//! ```

mod drive;
mod offscreen;
mod samples;
mod store;
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
            });
        }
        Some("samples") => samples::run(1_000_000),
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
        }),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: nc-bench offscreen [frames]\n       nc-bench window <panzoom|draw|input> <fifo|mailbox|immediate> [secs] [auto|late_ms]\n       nc-bench tablet [secs]\n       nc-bench drive <secs> [hz] [strokes]\n       nc-bench samples\n       nc-bench store [dir]"
    );
    std::process::exit(2);
}
