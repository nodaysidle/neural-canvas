---
paths:
  - "crates/nc-core/**"
---

# `nc-core` Invariants

1. **Zero External Platform Dependencies**:
   - Strictly NO `wgpu`, NO `winit`, NO OS filesystem calls in `nc-core`.
   - Pure document structures, stroke collections, spatial indexing, and viewport mathematics.

2. **Viewport Anchor Invariance**:
   - Pan and zoom mathematics must guarantee exact screen-to-world roundtripping.
   - Zooming in/out must hold the anchor point (mouse cursor / pinch centroid) stationary in world space.

3. **Determinism & Testing**:
   - All stroke processing and math must be 100% deterministic.
   - Use property tests (`proptest`) for geometric transforms and boundary conditions.
