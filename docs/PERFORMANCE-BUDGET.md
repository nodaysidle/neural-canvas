# Performance budget

**Reference machine:** Intel Arc iGPU (Meteor Lake), Mesa 26.2.5, 2560×1440 @ 120 Hz, Hyprland. If it's smooth
here, it's smooth on most Linux desktops.

**Stress scene S2k:** 2,000 strokes, 200k points, 150 recognised shapes, zoom 0.1×–32×.

"Budget" is the pass line. "Stretch" is the Gemini plan's tighter target: aim for it, don't block on it.

| Metric | Budget (must) | Stretch | How measured |
|---|---|---|---|
| Frame time drawing / panning / pinch-zooming S2k | ≤ 8.3 ms p99, ≤ 16.6 ms worst | ≤ 8.3 ms worst | tracy frame marks + wgpu timestamp queries; 60 s scripted pan/zoom |
| Main-thread CPU per frame | ≤ 3 ms p99 | ≤ 2 ms | tracy |
| GPU per frame at 2560×1440 | ≤ 4 ms p99 | ≤ 2 ms | timestamp queries |
| Input-to-photon, live stroke | ≤ 20 ms p95 | < 12 ms | 240 fps camera, 20 samples; `wp_presentation` timestamps (dev-only feature) |
| Pen/mouse sample retention | 100 % of delivered samples | — | compare with `libinput debug-events` |
| Dropped frames, 60 s pan of S2k | 0 over 16.6 ms; < 1 % over 8.3 ms | 0 over 8.3 ms | tracy |
| Idle | no frames scheduled, ~0 % CPU | — | frame counter, `/proc/<pid>/stat` |
| Cold start to interactive canvas | ≤ 300 ms warm / ≤ 800 ms cold | < 250 ms | exec → first presented frame |
| Sketch switch (S2k) | ≤ 100 ms to first frame | — | tracy span |
| Save | off main thread; ≤ 30 ms incremental, ≤ 200 ms full | < 10 ms incremental | tracy on store thread |
| Recognition of S2k | ≤ 300 ms on worker, UI stays 120 Hz | wireframe preview < 30 ms | tracy |
| RSS idle / with S2k | ≤ 80 MB / ≤ 200 MB | < 50 MB / < 180 MB | `/proc/self/status` (release build) |
| Binary (stripped) / AppImage | ≤ 25 MB / ≤ 40 MB | — | `ls -l` |

## Rules

- **Measure before optimising.** Every perf PR attaches before/after numbers from the S2k run.
- Nothing on the main thread touches disk, runs recognition, or allocates per sample on the hot path.
- Redraw only when dirty. The live stroke appends its new tail to the GPU buffer; committed strokes are never
  re-tessellated per frame.
- Present with `desired_maximum_frame_latency = 1`; choose present mode (Fifo vs Mailbox) from Phase 0
  latency numbers.
- Profiling (`tracy-client`) is behind a non-default cargo feature that release builds never enable.
- CI runs `criterion` benches (tessellation, hit-test, save, recognition) and fails on > 10 % regression.

## Scaffold baseline (2026-10-25, empty scene)

Window opens on Hyprland; offscreen render test passes on the Arc GPU. Release build: 0 CPU ticks over 3 s
idle, RSS ≈ 60 MB, stripped binary 8.0 MB. Not yet measured: frame time, latency, S2k.
