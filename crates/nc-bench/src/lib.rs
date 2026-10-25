//! Shared measurement pieces for the harness binary and the iced spike.

pub mod stats;

use nc_wayland::Presented;
use stats::{Summary, over};
use std::collections::HashMap;

/// Prints pacing and latency for frames `0..starts.len()`; `starts[i]` is frame i's start in
/// CLOCK_MONOTONIC ns (the moment its input state was sampled).
pub fn print_scanout(clock_id: Option<u32>, presented: &HashMap<u64, Presented>, starts: &[u64]) {
    println!("wp_presentation clock id: {clock_id:?} (1 = CLOCK_MONOTONIC)");
    let (mut shown, mut latency, mut discarded, mut missing, mut zero_copy, mut vsync) =
        (vec![], vec![], 0, 0, 0, 0);
    for (i, start) in starts.iter().enumerate() {
        match presented.get(&(i as u64)) {
            Some(Presented::Shown {
                at_ns,
                vsync: v,
                zero_copy: z,
                ..
            }) => {
                shown.push(*at_ns);
                latency.push(at_ns.saturating_sub(*start) as f64 / 1e6);
                zero_copy += *z as usize;
                vsync += *v as usize;
            }
            Some(Presented::Discarded) => discarded += 1,
            None => missing += 1,
        }
    }
    let intervals: Vec<f64> = shown
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]) as f64 / 1e6)
        .collect();
    println!(
        "presented {} / discarded {discarded} (never reached the screen) / no feedback {missing}; vsync flag {vsync}, zero-copy {zero_copy}",
        shown.len()
    );
    if let Some(s) = Summary::of(&intervals) {
        println!("scanout interval: {s}");
        println!(
            "  intervals > 8.33 ms: {} ({:.2} %), > 16.6 ms: {}",
            over(&intervals, 8.34),
            over(&intervals, 8.34) as f64 * 100.0 / intervals.len() as f64,
            over(&intervals, 16.7)
        );
    }
    if let Some(s) = Summary::of(&latency) {
        println!("frame start (input applied) → scanout: {s}");
    }
}
