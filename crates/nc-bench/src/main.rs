//! NeuralCanvas measurement harness (docs/PERFORMANCE-BUDGET.md). Dev tool: never packaged or shipped.
//!
//! ```text
//! nc-bench offscreen [frames]                    renderer cost vs scene size, headless, 2560x1440
//! nc-bench window <panzoom|draw> <fifo|mailbox|immediate> [secs]
//! nc-bench tablet [secs]                         log zwp_tablet_v2 vs winit pointer events
//! nc-bench store [dir]                           S2k save/load cost against a real file (default: temp dir)
//! ```

mod offscreen;
mod store;
mod window;

use window::{Config, Scenario};

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
            });
        }
        Some("store") => store::run(&arg(1).map_or_else(std::env::temp_dir, Into::into)),
        Some("tablet") => window::run(Config {
            scenario: Scenario::Tablet,
            present_mode: wgpu::PresentMode::Fifo,
            secs: secs(1, 15.0),
        }),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: nc-bench offscreen [frames]\n       nc-bench window <panzoom|draw> <fifo|mailbox|immediate> [secs]\n       nc-bench tablet [secs]\n       nc-bench store [dir]"
    );
    std::process::exit(2);
}
