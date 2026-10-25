# Build NeuralCanvas for Linux: brief for a coding agent

You are building **NeuralCanvas**, a GPU-accelerated sketch-to-wireframe app for Linux (Arch +
Wayland/Hyprland first), on the scaffold in this repository. This brief stands alone. The detail lives in:

- `docs/PLAN.md`: stack, architecture, the 20 Mac-app bugs to design out, phases, testing, packaging
- `docs/UX-PRINCIPLES.md`: what "polished" means here (acceptance criteria)
- `docs/PERFORMANCE-BUDGET.md`: frame, latency, memory budgets and how to measure them
- `docs/RELEASING.md`: release checklist; every artefact ships a `.sha256`

Read all four before writing code.

## What the product is

Users sketch UI layouts freehand (mouse or pen tablet) on an infinite canvas. NeuralCanvas recognises the
strokes as UI elements (buttons, inputs, cards, text, icons, dividers, images…), shows a clean wireframe
overlay, lets them edit it, styles it from an imported screenshot (palette, spacing, radii), and exports
SVG/PDF/PNG/JPEG. It is local-first: no network, no accounts, no sync. Work is organised as Projects →
Sketches.

## The two things that matter most

**1. Performance is a feature.** On the reference machine (Intel Arc iGPU, 2560×1440 @ 120 Hz, Hyprland):
- Frames ≤ 8.3 ms p99 while drawing, panning and pinch-zooming a 2,000-stroke / 200k-point scene ("S2k").
- Ink input-to-photon ≤ 20 ms p95 (stretch < 12 ms). Every pen sample is kept.
- Idle = no frames scheduled, ~0 % CPU. Cold start ≤ 300 ms.
- The main thread never touches disk, never runs recognition or export, never allocates per input sample on
  the hot path.
- **Measure before optimising.** Build the S2k scene generator and a tracy-instrumented run early (Phase 0)
  and attach before/after numbers to every perf-related change.

**2. UI/UX polish is a feature.** A janky, laggy, or confusing feature is not done:
- Content stays exactly under the cursor during pan and zoom. Zoom is `exp(k·delta)` at the cursor; pinch
  zooms at the centroid; touchpad scroll pans. Zoom-to-fit animates ~180 ms ease-out (off under reduced motion).
- The canvas is the app: full-bleed, quiet chrome, a floating tool palette, collapsible sidebar/inspector.
- Keyboard-first with Linux conventions (`Ctrl`, single-key tools, Space = temporary hand that restores the
  previous tool, full undo/redo wired to shortcuts and menu labels like "Undo Erase").
- Every state designed: hover, pressed, focus, disabled, empty, loading, error. Errors are toasts with an
  action; saving is invisible and never loses data.
- Crisp at fractional scales (1.25/1.5/1.75), works tiled and floating on Hyprland.
- No internal names, debug text, or TODO phrasing in the UI. No debug panels or flags in release builds.

When a trade-off appears between a feature and smoothness, keep the smoothness and say what you cut.

## Stack (decided, see README.md for why)

Rust 2024, Cargo workspace. **winit 0.30 + wgpu 30** for window and canvas; **iced 0.14** for the shell
(Phase 5) hosting the canvas through iced's `shader` widget, *if* the Phase 0 spike shows it holds the budget;
otherwise keep raw winit and draw chrome in our own wgpu layer. Crates to use, not hand-roll: `glam`, `kurbo`,
`rstar`, `bytemuck`, `rusqlite` (bundled, WAL), `serde`, `postcard`, `uuid`, `rayon`, `crossbeam-channel`,
`resvg`/`usvg`, `svg2pdf`, `image`, `kmeans_colors`, `imageproc`, `tracy-client` (non-default feature),
`proptest`, `insta`, `criterion`. Do not add Tauri, Electron, WebKit or GTK.

## What the scaffold already has

- `crates/nc-core`: `Viewport` (y-down world, `screen = world*scale + offset`, `zoom_at`, `pan_by_screen`,
  clamp 0.1–32×), `Stroke` (drops duplicate points), `Document` with a `generation` counter, unit tests.
- `crates/nc-render`: wgpu renderer drawing into any `TextureView`; world-space grid shader with level fade;
  instanced SDF capsule strokes (round caps/joins, analytic AA, hairline floor); append-only instance upload;
  an offscreen GPU test that checks pixels; a layout test guarding attribute offsets.
- `crates/nc-app`: winit window (app id `sh.nodaysidle.NeuralCanvas`, min 640×480), sRGB surface,
  `desired_maximum_frame_latency = 1`, redraw-on-dirty only; left-drag draws, middle/Space-drag pans,
  wheel/pinch/Ctrl-scroll zooms at cursor, touchpad scroll pans, `Ctrl+0` resets.
- Known shortcut to replace in Phase 2: `Renderer::sync` walks all segments to find the new tail
  (O(points) per frame). Replace with per-stroke GPU caches + a tile cache.

## Hard rules

- One coordinate convention: y-down, origin top-left, logical px. The only flip is world→clip in the shader.
- Every document mutation is a `Command` with its own inverse; history is per sketch, dropped on switch.
- Persistence: store thread, 300 ms debounce, flush on sketch switch / window close / SIGTERM. No silent
  fallbacks; a save failure is a visible banner.
- Workers return results tagged with `sketch_id + revision`; stale results are dropped.
- GPU structs are `#[repr(C)]` + `bytemuck::Pod`, offsets from `offset_of!`.
- Each Mac bug in `docs/PLAN.md` §4 gets the guard test listed there.
- Every phase ends green: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
- Commit locally in small, reviewable commits. **Do not add a git remote, push, publish to the AUR, or create
  releases** without the owner's explicit go-ahead.
- Never open or print `.env`, `*.key`, `~/.ssh`, `~/.grokbot`, or any credentials.
- No placeholder code, no `todo!()`, no mock implementations left behind. If something is out of scope, leave
  it out and say so.

## Phases and done-criteria

Do them in order. Do not start a phase until the previous one meets its done-criteria. At the end of each
phase, write a short report (what shipped, numbers, what's left, risks) to `docs/reports/phase-N.md`.

### Phase 0: Spike and measurement harness (2–3 d)
- S2k scene generator (deterministic seed) and a `--features profile` tracy build.
- Measure S2k in raw winit vs iced `shader` widget: frame time p50/p99, dropped frames, input-to-photon
  (presentation timestamps + camera if possible).
- Verify pen pressure/tilt/eraser on Hyprland through winit 0.30 (and 0.31 if released); else prototype
  reading `zwp_tablet_v2` via `smithay-client-toolkit` on the same display.
- Compare `PresentMode::Fifo` vs `Mailbox` for latency and tearing.
- **Done when:** `docs/reports/phase-0.md` has the numbers and the decisions (shell host, tablet path,
  present mode, tile-cache threshold); the decisions are recorded in `docs/PLAN.md`.

### Phase 1: Core document, commands, persistence (4–5 d)
- `nc-core`: `Element` (Stroke | Shape | RecognizedShape) with stable `Uuid`, commands + history, viewport
  fit-to-bounds. `nc-store` crate: rusqlite WAL, one row per element, store thread, debounce, flushes,
  schema version + migrations.
- **Done when:** guard tests for bugs #1–7, 11, 12, 18 pass headless; kill -9 harness proves a stroke drawn
  1 s before the kill survives; proptest shows any random command sequence undone fully returns the original
  document.

### Phase 2: Canvas MVP that feels great (5–7 d)
- Per-stroke GPU buffers keyed by (id, revision); tile cache above the Phase 0 threshold; live-stroke ring
  buffer; one-euro smoothing tuned for no visible lag; pressure for styluses only; tool state machine with
  temporary hand; undo/redo; fit/100 %; pinch; smooth animated fit.
- **Done when:** S2k meets every row of the frame and latency budgets on the reference machine (tracy capture
  attached); golden images for H/V/diagonal strokes and the grid at scales 1/1.25/2 pass; a screen recording
  shows draw → undo → redo → pan → pinch → fit with no visible hitch.

### Phase 3: Editing tools (4–6 d)
- Select (click, marquee, lasso), move, arrow nudge, delete, duplicate, copy/paste (internal format plus
  SVG/PNG for other apps), rect/ellipse/line with Shift/Alt modifiers, swept-path segment eraser with a ring
  cursor, rstar index; 8 px screen-constant handles.
- **Done when:** every interaction has a core unit test and appears in one recording; a rectangle erases by
  touching any edge; moving 500 selected strokes holds 120 Hz.

### Phase 4: Recognition, wireframe overlay, export (5–6 d)
- Port the Swift heuristics (`docs/PLAN.md` §3) with RDP; quality report and "Review uncertain items";
  overlay with opacity slider and side-by-side; `nc-export` crate: SVG writer, PDF via `svg2pdf`, PNG, JPEG,
  for raw sketch and wireframe.
- **Done when:** ≥ 50 recorded stroke fixtures with accuracy ≥ the Swift baseline (target > 88 %);
  recognition runs off-thread with zero dropped frames; "top stays top" passes across canvas, overlay, SVG,
  PDF, PNG; exports open correctly in Inkscape, Firefox and a PDF viewer (screenshots attached).

### Phase 5: Shell and polish (5–7 d)
- Two-level sidebar (projects → sketches with thumbnails), editable inspector (type, geometry, label,
  colours), floating tool palette, zoom HUD, empty-canvas hint, toasts, preferences (pressure curve,
  smoothing, snap shapes, grid), shortcuts overlay, light/dark from the portal, accessibility, copy review.
- **Done when:** each of the 10 principles in `docs/UX-PRINCIPLES.md` is checked off with a screenshot or
  recording on Hyprland (tiled and floating) at scale 1.0 and 1.5; grep finds no internal strings in UI text;
  the frame budget still holds with the shell visible.

### Phase 6: Style mirror (3–4 d)
- Screenshot import (file chooser portal + paste), palette via `kmeans_colors`, spacing/radius/line-height
  estimates, WCAG 2.1 AA check, apply with warnings, presets library.
- **Done when:** five real app screenshots give palettes a human agrees with (side-by-side attached);
  extraction runs off the main thread with the UI at 120 Hz.

### Phase 7: Packaging (3–4 d)
- `packaging/`: `.desktop`, AppStream metainfo, hicolor icons, PKGBUILD (source) + PKGBUILD-bin, AppImage
  recipe (Ubuntu 24.04 container), release script that writes a `.sha256` per artefact (`docs/RELEASING.md`).
- **Done when:** `namcap` clean; clean Arch VM installs and runs; AppImage runs on Arch plus Fedora or
  Ubuntu; scripted flow (create project → draw → erase → switch → quit → relaunch) loses nothing;
  `sha256sum -c` passes for every artefact. Stop there; publishing is the owner's call.

## How to report

At the end of each phase: what changed, test and clippy output, measured numbers against the budget (with
how they were measured), screenshots/recordings paths, anything skipped and why, open risks. If a budget is
missed, say so plainly with the numbers; do not round them into a pass.
