# Changelog

## v0.1.0 (2026-10-25): initial Linux release

Sketch interface layouts by hand, get a clean wireframe, style it from a screenshot, export it.
Local only: no account, no network. Measured on the reference machine: Intel Arc iGPU (Meteor Lake),
2560×1440 at 120 Hz, Hyprland 0.56. Method and raw numbers for each line: `docs/reports/phase-N.md`.

### Core engine
- Rust 2024; raw winit 0.30 + wgpu 30 (Vulkan on Mesa); own wgpu chrome layer.
- S2k scene (2,000 strokes, 200k points) with the full interface drawn: scanout interval p99
  8.34 ms at 120 Hz, 0 frames over 16.6 ms, 0 discarded; 0.08 % (pan/zoom) to 0.53 % (moving 500
  strokes) of frames one refresh late. Main-thread CPU p99 1.19 to 1.60 ms.
- 48 ns per input sample on the hot path; every delivered sample kept.
- Idle: no frames scheduled, 0 CPU ticks over 5 s. Cold start 111 to 119 ms to the first frame.

### Ink
- 1€ filter smoothing: lag under 2 px from 100 to 5,000 px/s once settled.
- Predicted ink one refresh ahead: about 14.4 ms visible lag at p95, a derived figure (23.2 ms
  measured without prediction; a camera run is still to do).
- Pressure for styluses through `zwp_tablet_v2`; Space for a temporary hand tool.

### Recognition and wireframe
- 96.7 % accuracy on 60 held-out stroke fixtures, against 51.7 % for the port of the Swift app's
  heuristics (target above 88 %).
- Runs on a worker: S2k in 55 ms (p50 54.9 ms), with no frames over 16.6 ms while it runs.
- `W` cycles sketch, hybrid and wireframe; `Shift+W` side by side; `U` steps through uncertain
  elements, which are drawn in amber. Wireframe shapes are drawn straightened.

### Editing and saving
- Select, marquee, lasso, move, resize, shapes, a splitting eraser, copy and paste, full undo.
- SQLite (WAL) store on its own thread, one row per element, saved within 300 ms of a change; a
  stroke drawn 1 s before `kill -9` survives.

### Export
- SVG, PDF (svg2pdf), PNG and JPEG, for the sketch and for the wireframe, plain or styled. "Top stays
  top" is checked across canvas, SVG, PDF and PNG.

### Style mirror
- Import a screenshot (file chooser portal or `Ctrl+V`): k-means palette in CIELAB (`kmeans_colors`),
  background, surface, accent and text roles, corner radius and spacing rhythm, WCAG 2.1 AA checks
  with warnings before applying. Presets and each sketch's style saved in the store.
- Decode and extraction on a worker: about 283 ms for a 1440p screenshot; the canvas keeps 0 frames
  over 16.6 ms in two of three runs with extraction looping (one 17.3 ms stall, cause not found).

### Packaging
- AUR-ready `neuralcanvas` and `neuralcanvas-bin` PKGBUILDs (namcap: 0 errors), an AppImage that runs
  on Arch, Ubuntu 24.04 and Fedora 41, desktop entry, AppStream metainfo and icons. The binary needs
  glibc 2.39 or newer.

### Artefacts

| File | Size | SHA-256 |
|---|---|---|
| `NeuralCanvas-0.1.0-x86_64.AppImage` | 9.67 MB | `80fd1df052b91f46692bbba8b78619a14f2195dd5558940c882caf18f49d2247` |
| `neuralcanvas-0.1.0-x86_64.tar.zst` (binary) | 6.97 MB | `fbdb856cd370c7a98183c6918dd2a7cc3c4090f5d8365f763be4438009c85660` |
| `neuralcanvas-0.1.0.tar.gz` (source) | 0.95 MB | `29f21d68ef6cf844db3ba68ee63f6096ad87ae1f6c0ab63373cf17665702830a` |

Built by `packaging/release.sh`, reproducible byte for byte from this tag. Verify a download with
`sha256sum -c <file>.sha256`.

### Known limits
- Spacing and line height from a screenshot are reported, not applied to the wireframe's layout.
- X11 untested. Wayland on Hyprland is the tested path.
- The AppImage uses the host's libwayland and GPU driver; it bundles the Vulkan loader and
  xkbcommon only as a fallback.
