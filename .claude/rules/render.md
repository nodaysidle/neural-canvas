---
paths:
  - "crates/nc-render/**"
---

# `nc-render` Architectural Rules

1. **Host Agnostic**:
   - `nc-render` must draw into any arbitrary `wgpu::TextureView`.
   - It must never depend directly on `winit` or windowing handles, so it can run inside offscreen tests, headless benchmarks, or an iced `shader` widget.

2. **GPU Data Layout**:
   - All vertex and instance structures passed to `wgpu` must use `#[repr(C)]` and implement `bytemuck::Pod` and `bytemuck::Zeroable`.
   - Keep WGSL uniform and vertex buffers aligned to 16 bytes.

3. **Offscreen Testability**:
   - Every rendering feature (grid, SDF capsule strokes, wireframe overlays) must include an offscreen headless test verifying texture output without requiring an active display server.
