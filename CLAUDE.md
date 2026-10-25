# CLAUDE.md — NeuralCanvas for Linux

## Canary — read first

- Start every reply with `ALAN`. No exceptions, including one-line answers.
- If `HANDOFF.md` exists, read it before doing anything else.
- When the session is long or you cannot clearly recall earlier work, say so, then write `HANDOFF.md` before continuing.
- Consult `/home/arch/dev/nodaysidle/nodaysidle-knowledge` (`sessions/LATEST.md` and `wiki/`) as the central source of truth for ecosystem decisions.

## Project Overview
- **Product**: GPU-accelerated sketch-to-wireframe canvas for Linux (Arch Linux + Wayland/Hyprland first). Local-first, zero cloud dependencies.
- **Stack**: Rust 2024 (1.89+), Cargo workspace (`nc-core`, `nc-render`, `nc-store`, `nc-export`, `nc-wayland`, `nc-app`, plus dev-only `nc-bench` and `spike-iced`).
  - Window & Canvas: `winit 0.30` + `wgpu 30` (Vulkan on Mesa).
  - Math & Geometry: `glam 0.34`, `bytemuck 1.x`, `kurbo`, `rstar`.
  - App Shell: `iced 0.14` hosting the canvas via `shader` widget (Phase 5).
- **Workspace Crates**:
  - `crates/nc-core`: Document model, stroke geometry, viewport transform math. Strictly NO GPU, NO windowing types.
  - `crates/nc-render`: `wgpu` canvas renderer (grid + instanced SDF capsule strokes via `canvas.wgsl`). Offscreen-tested.
  - `crates/nc-store`: SQLite (WAL) persistence on a dedicated store thread: one row per element, 300 ms debounce, schema migrations.
  - `crates/nc-export`: SVG writer (single source of truth) and PNG via `resvg`; PDF/JPEG in Phase 4.
  - `crates/nc-wayland`: side channel on winit's `wl_display` (presentation feedback, tablet-v2). Never reads the socket itself.
  - `crates/nc-app`: `neuralcanvas` binary (winit window, wgpu surface, event pump, input -> core -> render).

## Common Commands

| Command | Purpose |
|---------|---------|
| `cargo check --workspace` | Fast workspace compilation check |
| `cargo test --workspace` | Run all unit & offscreen tests |
| `cargo run -p neuralcanvas` | Launch the interactive development canvas window |
| `cargo clippy --workspace --all-targets` | Run linter across all crates and tests |
| `cargo fmt --check` | Verify Rust formatting |
| `cargo build --release` | Produce optimized release binary (`target/release/neuralcanvas`) |

## Architecture & Specifications
@AGENT_PROMPT.md
@docs/PLAN.md
@docs/UX-PRINCIPLES.md
@docs/PERFORMANCE-BUDGET.md
@docs/RELEASING.md

## Critical Non-Negotiables & Performance Budgets
1. **Frame & Latency Budgets** (Target: Intel Arc iGPU @ 1440p 120Hz):
   - ≤ 8.3 ms p99 frame time while drawing, panning, or pinch-zooming a 2,000-stroke / 200k-point scene ("S2k").
   - Ink input-to-photon latency ≤ 20 ms p95 (stretch < 12 ms). Every pen sample is preserved.
   - Idle state: 0 scheduled frames, ~0% CPU. Cold start ≤ 300 ms.
2. **Main Thread Purity**:
   - The main/render thread **never** touches disk, **never** runs stroke recognition or export, and **never** allocates on the hot input sample path.
   - Offload persistence, export (PNG/PDF/SVG), and ML recognition to background workers (`rayon` / `crossbeam-channel`).
3. **UX & Canvas Physics**:
   - Content must remain strictly anchored under the cursor during pan and zoom (`exp(k·delta)`).
   - Spacebar triggers a temporary hand tool that restores the previous tool on release.
   - Full-bleed canvas, quiet chrome, no generic icon libraries (text controls preferred), dark-first.
4. **Name the Library**:
   - Do NOT hand-roll math, curves, spatial index, or parsers. Use established crates (`glam`, `kurbo`, `rstar`, `rusqlite`, `resvg`, `image`, `postcard`).
