# Phase 2 report: canvas MVP

Date: 2026-10-25. Reference machine as before (Intel Arc iGPU, Mesa 26.2.5, Hyprland 0.56.2, 2560×1440 @
119.998 Hz, scale 1.0). Release builds, Rust 1.98.1.

## Read first

1. **Zoom shortcuts differ from the brief.** The Phase 2 brief said Ctrl+9 fit / Ctrl+0 100 %.
   `docs/UX-PRINCIPLES.md` §4 (the acceptance criteria) says Ctrl+0 fit / Ctrl+1 100 %, which Phase 1
   shipped. I kept the documented mapping. Swapping is a two-line change (`nc-core/src/actions.rs`).
   Open question 8 in `docs/PLAN.md`.
2. **The ink latency budget is missed.** Real pointer event → scanout is p95 23.2 ms against a 20 ms
   budget, after a 3 ms improvement this phase. The rest is the compositor's two-refresh pipeline (see
   Numbers). Open question 7 has the options.
3. **Pen pressure is implemented but unverified.** There is no tablet on the machine, as agreed.

## Done-criteria

| Criterion | Status | Evidence |
|---|---|---|
| S2k frame budget: ≤ 8.3 ms p99, 0 over 16.6 ms, < 1 % over 8.3 ms | Pass | Scanout intervals p99 8.34 ms (one refresh at 119.998 Hz), 0 over 16.6 ms, 0.11 % (pan/zoom) and 0.28 % (draw) over 8.33 ms |
| Main-thread CPU ≤ 3 ms p99 | Pass | 0.95 ms (pan/zoom), 1.03 ms (draw) |
| GPU ≤ 4 ms p99 | Pass | S2k all visible: 1.33 ms (timestamp queries) |
| Input-to-photon ≤ 20 ms p95 | **Fail** | Event → scanout p95 23.24 ms (p50 19.01) |
| Every sample kept | Pass | Every delivered sample produces one point; 48 ns per sample on the hot path |
| Goldens: H/V/diagonal strokes, grid at 1.0 / 1.25 / 2.0 | Pass | `nc-render/tests/golden/*.png`; ±2/255 on ≤ 0.1 % of pixels |
| `fmt`, `clippy -D warnings`, tests | Pass | 59 tests, 0 failures; clippy clean with and without `--features profile` |
| Tracy numbers | Not attached | No Tracy viewer on this machine. All numbers come from the harness: wall clocks, GPU timestamps, `wp_presentation` |

Other budget rows checked this phase:

| Row | Result | Budget |
|---|---|---|
| Idle | 0 CPU ticks over 5 s (empty and S2k) | no frames, ~0 % CPU |
| Cold start to first presented frame (warm cache, 5 runs) | 97–100 ms empty, 126–139 ms S2k | ≤ 300 ms warm |
| RSS | 59 MB empty, 70 MB S2k | ≤ 80 / ≤ 200 MB |
| Stripped binary | 12.6 MB | ≤ 25 MB |
| Input sample cost (Mac bug #16) | 48 ns | < 1 µs |

## What shipped

**Rendering (`nc-render`)**
- **Per-stroke GPU cache.** All committed strokes share one instance arena, with one contiguous range per
  stroke. A range is valid while the document still holds the same `Arc<Element>`. Every edit replaces
  the `Arc`, so pointer identity is the element's revision; that's the "(id, revision)" key from the
  brief.
- **Draws in z-order.** Each frame draws one instanced call per contiguous run in z-order, which is a
  single draw for an untouched sketch.
- **Holes and compaction.** Erasing leaves a hole without a rebuild. Undo restores the stroke into a new
  slot and keeps the z-order right. The arena compacts when holes pass half its size or z-order splits
  into more than 64 runs. An unchanged frame uploads nothing.
- **Live stroke fast path.** The stroke under the pen uploads only its new tail, at most 2 segments a
  frame (tested).
- **Pressure width.** Pressure scales each segment's width.
- **Tests.**
  - Goldens for horizontal, vertical, diagonal (1× and 2×) strokes and the grid at 1×, 1.25× and 2×.
  - A direction-independence check: the vertical stroke equals the transposed horizontal one within
    1/255 (Mac bug #10).
  - Cache-behaviour and live-tail tests.

**Input and tools (`nc-core`)**
- **1€ smoothing** with the `one-euro` crate, in screen px so it feels the same at any zoom. Tested
  bounds:
  - lag under 2 px from 100 to 5,000 px/s once settled
  - under 4 px from 10 ms into a stroke
  - at least a 4× cut in ±1 px tremor energy

  On pen-up the stroke ends exactly where the pen lifted.
- **Pressure** is stored per point (`Stroke::pressure`, empty for mouse strokes) and maps to width with
  a 20 % floor.
- **Tool state machine.** Pen (P), Eraser (E), Hand (H). Space holds a temporary hand and the stylus
  eraser end a temporary eraser; neither overwrites the chosen tool (Mac bug #13 guard).
- **Eraser.** A whole-stroke eraser with swept-path hit testing (Mac bug #14 guard: a fast drag cannot
  skip a stroke). A drag is one undo step (`Session::apply_in_gesture`). Phase 3 replaces it with the
  splitting segment eraser on rstar.
- **View animation.** Zoom to Fit and Actual Size animate over 180 ms with cubic ease-out, zooming
  geometrically. They jump instantly when the desktop requests reduced motion, read live from the
  settings portal through `mundy`. Any pan, zoom or draw cancels a running animation.
- **`FrameScheduler`.** Late frame scheduling (below).

**Store (`nc-store`)**
- **Schema v2.** The first code migration re-encodes v1 stroke blobs (frozen v1 types) with an empty
  pressure track. Tested against a hand-built v1 database.

**Wayland (`nc-wayland`, new crate)**
- **Side channel.** Moved out of the bench and shared by the app and the bench. It provides presentation
  feedback, `zwp_tablet_v2` tool frames (position, pressure, eraser end, proximity) and, for the bench
  only, `wl_pointer` motion timestamps.

**App**
- **Input.** Mouse and tablet feed one input path. While a tablet tool is near, emulated pointer events
  are ignored, so a pen never draws twice.
- **Shortcuts and present mode.** P/E/H/Space, undo/redo, animated fit and 100 %. Present mode is `Fifo`
  explicitly.
- **Scheduler.** Frames are drawn by the scheduler, never on a blocking wait.

**Bench (`nc-bench`)**
- `window input`: event-driven like the app, measures each pointer event's compositor timestamp to the
  scanout of the first frame that contains it.
- `drive`: a `zwlr_virtual_pointer_v1` client that moves, or draws with the left button, through the
  compositor's real input path.
- `samples`: hot-path cost per input sample.
- `store <dir>`: keeps an S2k database for the app to open.

## Numbers

### Late frame scheduling

Hyprland as configured composites about 4 ms after vblank and shows the result two refreshes after the
sample. I first measured the latch edge by delaying the start of each frame after the frame callback
(fullscreen S2k pan/zoom, Fifo, 10–15 s each):

| Delay | Sample → scanout p50 | Frames over 8.33 ms |
|---|---|---|
| 0 ms | 16.66 ms | 0 % |
| 3 ms | 13.65 ms | 0 % |
| 3.5 ms | 13.20 ms | 0 % |
| 4 ms | 12.70 ms | 0.73 % |
| 4.5 ms | 12.20 ms | 2.45 % |
| 5 ms | 11.71 ms (p95 19.92) | 8.90 % |
| 6–7.5 ms | 17.4–18.7 ms | 60–100 % |

A fixed delay is brittle, because the edge depends on compositor load and on the vblank phase. The app
instead uses `FrameScheduler`, which draws at `vblank + phase` on the grid from `wp_presentation` and
waits with a timer, so input keeps being applied while it waits:
- After every 60 on-time frames it moves the phase 0.25 ms later.
- On a miss (a frame shown a refresh late), it backs off 1 ms and caps below the missed phase. The cap
  is re-probed after 30 s without a miss, and the phase never goes past half a refresh.

In simulation it settles inside a 4 ms latch with at most 5 misses in 15,000 frames. On the reference
machine it settled at **3.50 ms** (pan/zoom) and **3.75 ms** (draw).

### Input-to-photon, measured end to end

The `nc-bench drive` virtual pointer moves at 500 Hz. The bench redraws on each pointer event like the
app, and records the compositor's motion timestamp (CLOCK_MONOTONIC; receipt minus timestamp has p50
0 ms) and the scanout time of the first frame that contains it. Runs are 20 s each, about 9,800 events:

| Schedule | p50 | p95 | p99 | max | Frames > 8.33 ms |
|---|---|---|---|---|---|
| Draw at frame callback (Phase 1) | 21.89 ms | 25.62 ms | 26.01 ms | 26.14 ms | 0 |
| **Adaptive (app)** | **19.01 ms** | **23.24 ms** | **25.05 ms** | 33.74 ms | 3 (0.13 %) |
| Fixed 3.5 ms by blocking | 21.88 ms | 25.64 ms | 26.03 ms | 34.40 ms | 2 (0.09 %) |

The blocking row shows why the app waits on a timer: sleeping also delays the input that arrives
meanwhile, which cancels the gain.

This excludes USB polling (1 ms at 1 kHz) and panel response, so the camera figure would be a few ms
higher.

**Why the 20 ms p95 budget can't be met from the app.** An event that arrives just after a frame's
sample waits up to one refresh, then that frame takes two refreshes minus the phase to reach the
screen: about 3 × 8.33 − 3.5 ≈ 21.5 ms before any delivery delay. So p95 below 20 ms would need a
phase near 5 ms, and the latch closes at about 4 ms. The options are in `docs/PLAN.md` §9, question 7.
My recommendation is predicted ink: extrapolate the live tail about one refresh ahead, as platform ink
APIs do. It shortens what the eye sees without touching the measured event path, and it should be
measured with this same harness.

### Frame pacing with the scheduler (S2k, fullscreen, Fifo, 30 s)

| Run | Frames/s | CPU p50 / p99 | Scanout interval p99 / max | > 8.33 ms | > 16.6 ms | Discarded |
|---|---|---|---|---|---|---|
| Pan/zoom | 119.9 | 0.51 / 0.95 ms | 8.34 / 16.67 ms | 4 (0.11 %) | 0 | 0 |
| Draw (1 kHz synthetic pen) | 119.6 | 0.52 / 1.03 ms | 8.34 / 16.67 ms | 10 (0.28 %) | 0 | 0 |

### Renderer cost (headless, 2560×1440, `nc-bench offscreen 120`, ms)

| Strokes / points | View | CPU p50 / p99 | GPU p50 / p99 |
|---|---|---|---|
| 2,000 / 200k (S2k) | all visible | 0.07 / 0.11 | 1.09 / 1.33 |
| 2,000 / 200k (S2k) | 100% | 0.07 / 0.10 | 0.65 / 0.66 |
| 5,000 / 500k | all visible | 0.18 / 0.28 | 2.44 / 2.49 |
| 10,000 / 1M | all visible | 0.37 / 0.60 | 4.69 / 4.78 |
| 20,000 / 2M | all visible | 0.83 / 2.10 | 9.48 / 10.85 |

**Tile-cache threshold, re-evaluated: 600k visible segments** (was 400k). All-visible GPU time is about
0.47 ms per 100k segments, crossing 4 ms near 830k, so 600k keeps 25 % for the overlay and selection
passes. S2k is three times under it, so no tile cache was built. Two notes:
- There is no culling: GPU time at 100 % zoom still scales with the whole document. Per-stroke bounds
  culling is cheap and belongs with the Phase 3 rstar index.
- CPU per frame is now O(elements), with no per-point work: 0.83 ms p50 at 20k strokes.

## End-to-end run in the real app

Driven with `nc-bench drive … strokes` (virtual pointer with the left button), `wtype` for keys, and
`grim` for screenshots, on an empty workspace with a throwaway `XDG_DATA_HOME`. Each step was checked
against the database row count:

| Step | Rows | Screen |
|---|---|---|
| Draw 3 arcs (two overlap) | 3 | arcs visible |
| Ctrl+Z | 2 | last arc removed; it overlapped the first, so the screen looks the same |
| Ctrl+Shift+Z | 3 | back |
| E, one eraser drag through two arcs, P | 1 | both erased in one step |
| Ctrl+0 | 1 | animated fit to the remaining arc |
| SIGTERM (exit 0), relaunch, Ctrl+0 | 1 | same drawing |

Contact sheet: [`phase-2/e2e-flow.png`](phase-2/e2e-flow.png). The screen recording of the whole run is
`phase2-flow.mp4` (5 MB) in the session scratchpad, not committed. Goldens, enlarged for review:
[`phase-2/golden-strokes.png`](phase-2/golden-strokes.png), [`phase-2/golden-grid.png`](phase-2/golden-grid.png).

## Bugs found and fixed this phase

- **The Wayland side channel starved winit.** It read the display socket itself, consuming data
  winit's loop waits on, so winit's own pointer and keyboard events sat undelivered until something
  else woke the loop. Phase 0 missed it because its runs redrew continuously. The event-driven app
  would have stalled on input. It now only dispatches what winit already read.
- **Hyprland 0.56's `cursor.move` warp** sends `wl_pointer.motion` without `wl_pointer.frame`, which
  winit (through smithay-client-toolkit) never delivers. The latency driver therefore uses a virtual
  pointer device. This is a compositor quirk, worth an upstream report if you want one filed.
- **Floating-point end of animation:** `(t − start) / 0.18` lands a hair under 1.0, so the animation
  never reported done. It now compares against the end time directly.
- **Scheduler grid anchor:** a scanout timestamp can be ahead of the next frame request. The grid now
  extends both ways.

## Skipped, and why

- **Tracy capture:** no viewer is installed; the harness numbers above stand in for it.
- **Camera input-to-photon:** no 240 fps camera; the event → scanout measurement is the software part.
- **Pen pressure on hardware:** no tablet attached. The path is complete and the protocol binds; values
  are untested.
- **Pinch in the end-to-end run:** a virtual pointer cannot pinch. Wayland pinch gestures happen at the
  pointer, so the app zooms at the cursor, which is the centroid. Not exercised in this run.
- **Touchpad pan inertia** (UX principle 3) is not in the Phase 2 list and is not built.
- **Eraser ring cursor:** a stock cursor stands in until Phase 3 draws the ring (`shortcut:` comment in
  `app.rs`).
- **Long live strokes:** the live stroke reserves 4,096 points up front, so the input path does not
  reallocate during normal strokes. A longer stroke grows its buffer once per doubling, which is one
  allocation in a move handler (`.claude/rules/app.md` asks for none).

## Risks

1. **Latency budget** (above): it needs your decision.
2. **Scheduler and other compositors.** It relies on `wp_presentation`. Without it (X11, compositors
   without the protocol) it draws immediately, as before. KWin and Mutter pipelines differ and are
   untested.
3. **Tablet input is untested on hardware.** If Hyprland also sends emulated pointer events, the
   proximity guard drops them; if it sends none, nothing depends on them.
4. **Goldens were generated on the Arc GPU.** llvmpipe in CI may differ by more than ±2/255 at the AA
   edges; check when CI exists.
