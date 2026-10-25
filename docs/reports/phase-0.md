# Phase 0 report: spike and measurement harness

Date: 2026-10-25. Machine: Intel Core Ultra 7 155H, Intel Arc iGPU (MTL), Mesa 26.2.5 (Vulkan),
Hyprland 0.56.2, Samsung Odyssey G5 2560×1440 @ 119.998 Hz, scale 1.0. Rust 1.98.1, release builds.

## Decisions

| Question | Decision | Deciding evidence |
|---|---|---|
| Shell host | **Raw winit 0.30 + wgpu 30.** iced 0.14 is not adopted. Chrome is drawn in our own wgpu layer, unless an iced release on wgpu ≥ 30 matches raw winit in a rerun of `spike-iced` before Phase 5. | iced 0.14 requires `wgpu ^27` (crates.io metadata), so it cannot host `nc-render` (wgpu 30) without a downgrade or a second renderer. Host pacing is equal (both 120.0 frames/s, 0 discarded). iced's `frames()` animation path adds one refresh (+8.3 ms) of state-to-scanout lag. |
| Tablet path | **winit 0.30 for windowing, plus a `zwp_tablet_v2` side channel on winit's own `wl_display`** (wayland-client + wayland-protocols). Move to winit's native tablet events once winit 0.31 is stable. | winit 0.30.13's Wayland backend has no tablet code. winit 0.31.0-beta.4 has it, but it is a beta and iced/our stack are on 0.30. The side channel binds on Hyprland. **Pressure values are unverified: no tablet is attached.** |
| Present mode | **`Fifo`** (`AutoVsync` resolves to it), `desired_maximum_frame_latency = 1`. | Fifo, Mailbox and Immediate measured identical: same pacing, 0 discarded, same latency, every frame vsync-flagged. Fifo is the only mode Vulkan guarantees and never renders frames that get thrown away. |
| Tile-cache threshold | **400k visible segments** (≈ 4,000 strokes of 100 points). Below it, draw instanced capsules directly. | GPU time with everything visible at 2560×1440: 2k strokes p99 1.44 ms, 5k strokes p99 4.04 ms (over the 4 ms GPU budget). 400k leaves headroom for the overlay and selection passes. |

## What shipped

- `nc_core::scene::s2k(seed)`: 2,000 cubic-Bézier strokes × 100 points = exactly 200,000 points, scattered
  over 6000×4000 world px. `kurbo` evaluates the curves; `rand_chacha` (ChaCha8) seeds them, so the scene
  is identical on every platform. Tests: exact size, no dropped samples, all finite, same seed same scene.
- `--features profile` on `neuralcanvas` (and `nc-render`): optional `tracy-client 0.19` with spans on
  `window_event`, `Renderer::render` and `Renderer::sync`, plus a frame mark after each present. Without the
  feature, the dependency tree has no tracy crate (`cargo tree` count: 0).
- `crates/nc-bench` (publish = false, never packaged): the measurement harness.
  - `nc-bench offscreen`: headless renderer cost vs scene size at 2560×1440 with GPU timestamp queries.
  - `nc-bench window <panzoom|draw> <fifo|mailbox|immediate> [secs]`: fullscreen S2k, scripted.
  - `nc-bench tablet [secs]`: tablet-v2 side-channel probe.
  - `wl.rs`: a second Wayland event queue on winit's display, with `wp_presentation` feedback for real
    scanout timestamps and compositor-reported discards, and the `zwp_tablet_v2` probe.
- `crates/spike-iced` (publish = false): the same S2k pan/zoom inside an iced 0.14 `shader` widget, using a
  wgpu 27 port of the `nc-render` pipeline (same WGSL, same instancing, same O(points) sync).

## Numbers

### Renderer cost vs scene size (headless, `nc-bench offscreen 120`)

120 frames per row after 10 warm-up frames, a small pan every frame. GPU time comes from timestamps around
the render pass. CPU time is `Renderer::render` (sync + encode). All values are ms.

| Strokes / points | View | CPU p50 / p99 | GPU p50 / p99 |
|---|---|---|---|
| 1,000 / 100k | all visible | 0.67 / 0.75 | 0.82 / 1.28 |
| 1,000 / 100k | 100% | 0.67 / 0.80 | 0.46 / 0.48 |
| **2,000 / 200k (S2k)** | all visible | **1.34 / 1.58** | **1.41 / 1.44** |
| **2,000 / 200k (S2k)** | 100% | **1.34 / 1.53** | **0.75 / 0.80** |
| 5,000 / 500k | all visible | 3.33 / 3.70 | 3.87 / 4.04 |
| 5,000 / 500k | 100% | 3.33 / 3.61 | 1.98 / 2.53 |
| 10,000 / 1M | all visible | 6.70 / 8.41 | 8.51 / 9.14 |
| 10,000 / 1M | 100% | 6.69 / 7.30 | 4.24 / 4.65 |
| 20,000 / 2M | all visible | 13.41 / 15.49 | 9.81 / 10.65 |
| 20,000 / 2M | 100% | 13.42 / 15.03 | 7.24 / 7.90 |

Two findings for Phase 2:
- CPU grows by 0.67 ms per 100k points even though nothing changed. This is the scaffold's known shortcut:
  `Renderer::sync` walks every segment each frame. Per-stroke GPU caches remove it.
- At 100% zoom most strokes are off screen, yet GPU time still scales with the total (0.75 ms for S2k):
  there is no culling. Tiles or rstar culling fix that.

### Hosts and present modes (windowed, fullscreen 2560×1440, 20 s after 1 s warm-up)

The pan/zoom script circles the scene while zooming 0.25× to 4× (the whole S2k is on screen at 0.25×).
The draw script holds the view at 100% and appends a 1 kHz synthetic pen to a live stroke on top of S2k.
"Scanout" is the `wp_presentation` timestamp (CLOCK_MONOTONIC, clock id 1 confirmed). "Start" is when the
frame's input state was applied.

| Host | Scenario | Present | Frames/s | Discarded | Scanout interval p50 / p99 / max | > 8.33 ms | Start → scanout p50 / p95 / p99 | CPU p50 / p99 |
|---|---|---|---|---|---|---|---|---|
| winit 0.30 | pan/zoom | Fifo | 120.0 | 0 / 2400 | 8.33 / 8.34 / 8.34 | 0 | 16.66 / 16.77 / 16.81 | 1.70 / 3.30 |
| winit 0.30 | pan/zoom | Mailbox | 120.0 | 0 / 2400 | 8.33 / 8.34 / 8.34 | 0 | 16.68 / 16.75 / 16.76 | 1.43 / 2.99 |
| winit 0.30 | pan/zoom | Immediate | 120.0 | 0 / 2400 | 8.33 / 8.34 / 8.34 | 0 | 16.67 / 16.74 / 16.75 | 1.43 / 2.55 |
| winit 0.30 | draw | Fifo | 120.0 | 0 / 2400 | 8.33 / 8.34 / 8.34 | 0 | 16.64 / 16.74 / 16.75 | 1.51 / 3.02 |
| winit 0.30 | draw | Mailbox | 120.0 | 0 / 2400 | 8.33 / 8.34 / 8.34 | 0 | 16.67 / 16.75 / 16.76 | 1.50 / 2.61 |
| iced 0.14 shader | pan/zoom | Fifo | 120.0 | 0 / 2399 | 8.33 / 8.34 / 16.67 | 1 (0.04 %) | 24.95 / 25.05 / 25.06 (from redraw event); 16.57 / 16.68 / 16.70 (from draw start) | prepare 1.30 / 2.77 |
| iced 0.14 shader | pan/zoom | Mailbox | 120.0 | 0 / 2400 | 8.33 / 8.34 / 8.34 | 0 | 24.99 / 25.05 / 25.07 (from redraw event) | prepare 1.25 / 2.76 |

CPU for winit is the whole frame on the main thread: script, swapchain acquire (p99 ≤ 0.11 ms), encode,
submit, present. For iced it is only our primitive's `prepare`, because iced's own update/view/layout time
is not separable from outside. Against the budget: frame pacing (≤ 8.3 ms p99, 0 over 16.6 ms, < 1 % over
8.3 ms) passes in every row. Main-thread CPU ≤ 3 ms p99 passes for winit except the pan/zoom Fifo run
(3.30 ms) and the draw Fifo run (3.02 ms). Both overruns are inside the O(points) sync that Phase 2
replaces. GPU ≤ 4 ms p99 passes for S2k (1.44 ms).

### Tearing

`hyprctl getoption general:allow_tearing` is `false`. Every presented frame in every mode, Immediate
included, carried the compositor's `vsync` flag (2400 / 2400), so none tore. No frame was zero-copy:
`render:direct_scanout` is 0 (off). `misc:vrr` is 2 (fullscreen only), and intervals still held at
8.33 ms because the bench renders every refresh.

### Why Mailbox gains nothing here

All three modes rendered exactly 120.0 frames/s with an acquire wait under 0.2 ms. The loop is throttled
to one frame per refresh before the swapchain ever matters, so Mailbox never has a second frame to swap
in. Start → scanout is two refreshes (16.7 ms) in every mode. That fits the compositor compositing on the
refresh after our commit, then flipping on the one after.

### Tablet

- winit 0.30.13: no tablet-v2 code in its Wayland backend (`grep -i tablet` over its sources finds only
  iOS/Windows files). Pen pressure, tilt and eraser cannot arrive through it.
- winit 0.31.0-beta.4: has `zwp_tablet_v2`. It reports pressure normalized to 0..1, tilt cast to whole
  degrees (`as i8`), rotation, and an `Eraser` tool kind.
- smithay-client-toolkit 0.21.1 has no tablet helper, so the fallback uses wayland-client +
  wayland-protocols directly.
- Hyprland advertises `zwp_tablet_manager_v2` v1 (`wayland-info`). `nc-bench tablet 8` bound it and created
  a tablet seat on winit's connection, with no protocol errors and a clean exit. Result: 0 tablets, 0
  tools, because no tablet is plugged in (input devices: Apple Magic Keyboard, Logitech G102, a USB mic).
  A virtual tablet was not possible: `/dev/uinput` is root-only.

## How it was measured

```sh
cargo build --release -p nc-bench -p spike-iced
./target/release/nc-bench offscreen 120
./target/release/nc-bench window panzoom fifo 20      # also mailbox, immediate; draw fifo, draw mailbox
ICED_PRESENT_MODE=fifo ./target/release/spike-iced 20  # also mailbox
./target/release/nc-bench tablet 8
```

Each window run went fullscreen on DP-1 with nothing else animating. Runs were sequential, one at a time.

## Skipped, and why

- **Tracy capture**: the `profile` feature builds and is lint-clean, but no Tracy viewer or `tracy-capture`
  is installed, so no `.tracy` file is attached. All numbers above come from the harness (wall clocks, GPU
  timestamps, `wp_presentation`).
- **Camera input-to-photon**: no 240 fps camera run. The `wp_presentation` number is the software part:
  from the frame's sampled input to scanout start. It excludes the wait for the next frame when real input
  lands mid-frame (0 to 8.3 ms), USB polling (1 ms at 1 kHz) and panel response.
- **Real pen data**: no tablet hardware, see above.
- **iced pointer-input latency**: only `frames()`-driven animation was measured. Whether iced also delays
  real pointer events by one redraw is not measured.
- **150 recognised shapes in S2k**: there is no recognised-shape type before Phase 4, so S2k is strokes only.
- **Fractional scale (1.25/1.5/1.75) and tiled-window runs**: measured fullscreen at scale 1.0 only.

## Risks

1. **Ink latency budget (≤ 20 ms p95 input-to-photon) is at risk.** Sample → scanout is already 16.7 ms.
   Adding the mid-frame input wait, USB polling and panel time likely puts p95 above 20 ms. Phase 2 should
   try rendering later in the refresh: Hyprland advertises `wp_commit_timing_manager_v1` and `wp_fifo_manager_v1`,
   and our frame costs about 3 ms. Confirm with a camera.
2. **Tablet routing**: once our side channel binds the tablet seat, the compositor may stop sending
   emulated pointer events for the pen. Phase 2 must feed side-channel tool events into the same input path
   as the mouse and test with real hardware (open question 4 in `docs/PLAN.md`).
3. **iced divergence**: if the shell later moves to iced, the renderer must be on the same wgpu major as
   iced. `spike-iced` is the rerun harness for that check.
4. **Teardown order**: Wayland proxies created on winit's display must be dropped before winit closes it,
   or exit segfaults in `wl_proxy_destroy`. This happened once in the harness and is fixed there
   (`Bench::finish`). The app's tablet side channel must follow the same rule.

## Done-criteria check (AGENT_PROMPT.md, Phase 0)

- [x] S2k generator, deterministic seed.
- [x] `--features profile` tracy build. Capture not attached (no viewer installed).
- [x] raw winit vs iced `shader`: frame time p50/p99 and dropped frames. Input-to-photon from presentation
  timestamps; no camera.
- [x] Pen on Hyprland via winit 0.30 (none) and 0.31 (beta, supported); fallback prototyped and bound.
  Pressure unverified without hardware.
- [x] Fifo vs Mailbox (plus Immediate) for latency and tearing.
- [x] Decisions recorded in `docs/PLAN.md` §1.

Gate: `cargo fmt --check`, `cargo check --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings` (also with `--features profile` on
`neuralcanvas` and `nc-render`) and `cargo test --workspace` all pass. That is 10 tests, including the
offscreen render test on the Arc GPU.
