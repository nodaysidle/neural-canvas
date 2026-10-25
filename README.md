# NeuralCanvas for Linux

A GPU-accelerated sketch-to-wireframe canvas for Linux. Arch + Wayland/Hyprland first.
This repo is the **scaffold**: a Cargo workspace whose window opens a wgpu canvas with a world-space grid
and lets you draw, pan and zoom. The full app is built on top of it by following
[`AGENT_PROMPT.md`](AGENT_PROMPT.md) and [`docs/PLAN.md`](docs/PLAN.md).

## Stack choice: Rust + winit + wgpu (iced for the shell)

The two port plans (`nodaysidle-knowledge/briefs/neuralcanvas/nc-linux-plan-{claude,gemini}.md`) disagree on
the shell and agree on everything below it:

| | Claude plan | Gemini plan |
|---|---|---|
| Language | Rust | Rust |
| Primary | GTK4 + libadwaita + GtkGLArea (GL) | **winit + wgpu + iced** |
| Fallback | **winit + wgpu + iced** | — |
| Tauri / WebKitGTK | rejected (12 fps measured on this iGPU) | rejected |
| Canvas renderer | instanced SDF capsules, live-stroke fast path, world-space grid | SDF strokes, live-stroke ring buffer, world-space grid |

**winit + wgpu + iced is the only stack both plans endorse**, so it is the pick. It also removes GTK's
AppImage bundling pain (Claude plan risk: "High") and keeps one renderer (wgpu/Vulkan) for the window,
offscreen tests and raster export. The cost, flagged by the Claude plan: Wayland tablet pressure through
winit is unverified, and iced does not look like GTK. Both are Phase 0 spike items in `docs/PLAN.md` with a
documented fallback. The canvas renderer (`nc-render`) draws into any `wgpu::TextureView`, so it can be hosted
by raw winit (today) or an iced `shader` widget (Phase 5) unchanged.

## Layout

```
crates/
  nc-core/     document, strokes, viewport maths. No GPU, no window. Unit-tested.
  nc-render/   wgpu renderer: grid + instanced SDF capsule strokes (canvas.wgsl). Offscreen-tested.
  nc-store/    SQLite (WAL) persistence on its own thread: one row per element, 300 ms debounce.
  nc-export/   SVG writer and PNG (via resvg) for copy and export.
  nc-wayland/  Wayland side channel: presentation feedback and tablet (pen pressure, eraser) input.
  nc-bench/    measurement harness (S2k frame pacing, scanout latency, save cost). Never packaged.
  spike-iced/  Phase 0 iced host spike, kept to rerun the shell decision.
  nc-app/      `neuralcanvas` binary: winit window, wgpu surface, input → core → render.
docs/
  PLAN.md               merged final plan (stack, architecture, bugs to design out, phases)
  UX-PRINCIPLES.md      what "polished" means here
  PERFORMANCE-BUDGET.md frame/latency/memory budgets and how to measure them
  RELEASING.md          release checklist; every artefact ships a .sha256
AGENT_PROMPT.md         self-contained brief for the agent that builds the full app
```

## Build and run

Needs Rust ≥ 1.89 (tested 1.98.1) and a Vulkan driver.

```sh
sudo pacman -S --needed rust vulkan-icd-loader vulkan-intel   # or vulkan-radeon / nvidia-utils
cargo run --release
cargo test            # includes an offscreen GPU render test (skips if no adapter)
cargo clippy --all-targets -- -D warnings
```

Controls: `P` pen, `E` eraser, `H` hand; hold `Space` for a temporary hand. Left-drag uses the tool,
middle-drag pans, wheel / Ctrl+touchpad scroll / pinch zooms at the cursor, touchpad two-finger scroll
pans. `Ctrl+Z` undoes, `Ctrl+Shift+Z` or `Ctrl+Y` redoes, `Ctrl+0` zooms to fit the drawing, `Ctrl+1` shows
it at 100 % (both animate unless the desktop asks for reduced motion). A stylus draws with pressure and
its eraser end erases.

Work saves itself to `$XDG_DATA_HOME/neuralcanvas/neuralcanvas.db` (usually
`~/.local/share/neuralcanvas/`) within 300 ms of each change, and on close or `SIGTERM`.

The Wayland app id is `sh.nodaysidle.NeuralCanvas` (Hyprland `class`), pending confirmation of the domain.

## Releases

Every release artefact ships with a matching `<file>.sha256`. Verify with `sha256sum -c <file>.sha256`.
See [`docs/RELEASING.md`](docs/RELEASING.md).
