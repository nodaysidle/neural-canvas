---
paths:
  - "crates/nc-app/**"
---

# `nc-app` Event & Performance Rules

1. **Hot Input Path Invariants**:
   - Zero heap allocations inside mouse move, pen input, or scroll event handlers.
   - Pen events (Wayland tablet / winit pointer) must preserve every hardware point; never downsample before the smoothing pipeline.

2. **Frame Pacing & Idle**:
   - Do not schedule continuous redraws when no input or animation is active. Idle CPU must sit near ~0%.
   - Only call `window.request_redraw()` when state actually changes.

3. **Background Worker Isolation**:
   - Disk writes, autosave (SQLite WAL), vector exports (SVG/PDF), and recognition inference must be dispatched over `crossbeam-channel` or `rayon` threads, never blocking the event loop.
