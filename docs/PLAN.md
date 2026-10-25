# NeuralCanvas for Linux: merged plan

Merged from `nodaysidle-knowledge/briefs/neuralcanvas/nc-linux-plan-claude.md` and `…-gemini.md`
(both 2026-10-25). Where they disagree, the choice and the reason are stated inline.

**Target:** Linux only. Arch first, Wayland/Hyprland first; X11 and other distros best effort.
**Reference machine:** Intel Core Ultra (Meteor Lake), Intel Arc iGPU, Mesa 26.2.5, 2560×1440 @ 120 Hz,
Hyprland 0.56.2, Rust 1.98. Every budget is set for this machine.

This is a **new app**, not a recompile of the Swift/macOS one. What carries over: the data model, the
recognition and style-mirror algorithms, the feature set, and the list of bugs to design out (§4).
The PRD still says "macOS only"; Alan amends it before Phase 1 (§9).

---

## 1. Stack

**Rust + winit 0.30 + wgpu 30 (Vulkan on Mesa) for the window, canvas and chrome.** The Phase 0 spike
(below) ruled out iced 0.14 as the host: it pins wgpu 27. The shell (sidebar, inspector, dialogs) is drawn
in our own wgpu layer, unless an iced release on wgpu ≥ 30 matches raw winit in a rerun of `spike-iced`
before Phase 5.

Why: it is the one stack both plans endorse (Gemini primary, Claude fallback). Both rejected Tauri/WebKitGTK
(our VectorStudio measured 12 fps under WebKitGTK on this iGPU). Dropping GTK also removes the hardest
packaging risk (GTK4 AppImage bundling).

Risks taken on, resolved by the Phase 0 spike ([`reports/phase-0.md`](reports/phase-0.md)):

| Risk (source) | Spike result | Decision |
|---|---|---|
| Wayland tablet-v2 pressure/tilt/eraser via winit is unverified (Claude) | winit 0.30.13 has no tablet code; 0.31.0-beta.4 has it but is a beta. A `zwp_tablet_v2` side channel (wayland-client + wayland-protocols on winit's `wl_display`) binds on Hyprland. No pen attached, so pressure values are unverified | winit 0.30 + our side channel feeding the same input path; switch to winit's tablet events when 0.31 is stable. Hardware check open (§9 q4) |
| iced shell may add latency or frame-pacing jitter to the canvas | Same pacing as raw winit (120.0 frames/s, 0 discarded). iced 0.14 requires wgpu 27 (we are on 30); `frames()`-driven animation lands one refresh later (25.0 vs 16.7 ms to scanout) | Raw winit host; own wgpu chrome layer |
| iced widgets don't look like GTK (Claude) | n/a: accepted. We design our own restrained dark-first chrome (docs/UX-PRINCIPLES.md) | — |

### Phase 0 decisions (2026-10-25, measured on the reference machine)

- **Shell host:** raw winit 0.30 + wgpu 30. Re-check iced only if a release targets our wgpu major.
- **Tablet path:** winit 0.30 plus a `zwp_tablet_v2` side channel; winit 0.31 native once stable.
- **Present mode:** `Fifo` (`AutoVsync`), `desired_maximum_frame_latency = 1`. Mailbox and Immediate
  measured identical on Hyprland (0 discarded, every frame vsynced, 16.7 ms sample → scanout).
- **Tile-cache threshold:** 400k visible segments (≈ 4k strokes). S2k all-visible GPU p99 is 1.44 ms;
  5k strokes reaches 4.04 ms.

**Crates (name the library, don't hand-roll):** `glam` (math), `kurbo` (curves, flattening),
`rstar` (R-tree hit-test/culling), `bytemuck` (`#[repr(C)]` + `Pod` GPU structs), `rusqlite` (bundled,
WAL) + `serde` + `postcard` + `uuid` (persistence), `rayon` (workers), `crossbeam-channel`,
`resvg`/`usvg` (PNG export), `svg2pdf` (PDF export), `image` (JPEG, screenshot import),
`kmeans_colors` + `imageproc` (style mirror), `tracy-client` behind a non-default cargo feature (profiling),
`proptest`, `insta`, `criterion` (tests/benches).

Export choice: Claude used cairo (a GTK dependency we no longer have); Gemini used usvg/resvg/printpdf.
Merged: **write SVG ourselves (single source of truth), PDF via `svg2pdf`, PNG via `resvg` or our own
offscreen wgpu renderer, JPEG via `image`.**

## 2. Architecture

```
crates/
  nc-core/     document, element (Stroke | Shape | RecognizedShape), commands + history, viewport,
               hit-test (rstar), input (one-euro filter, pressure curve), tools state machine,
               recognize/, style/. No GPU, no window. 100% headless-testable.
  nc-render/   wgpu renderer drawing into any TextureView: grid, committed strokes (per-stroke GPU caches,
               tile cache above 400k visible segments), live stroke fast path, wireframe overlay, selection chrome.
  nc-store/    rusqlite WAL; store thread, 300 ms debounce, flush on switch/close/SIGTERM.
  nc-export/   SVG writer, svg2pdf, PNG, JPEG; raw sketch AND wireframe export.
  nc-app/      `neuralcanvas` binary: window, input routing, actions/shortcuts, shell (own wgpu chrome).
  nc-bench/    measurement harness (S2k runs, `wp_presentation` timing, tablet probe). Never packaged.
  spike-iced/  Phase 0 iced 0.14 host spike, kept as the rerun harness for the shell decision.
packaging/     PKGBUILD, PKGBUILD-bin, .desktop, metainfo.xml, icons, AppImage recipe.
```
The scaffold has `nc-core`, `nc-render`, `nc-app`. Add the others when their phase starts.

**Coordinates (one convention, everywhere):** world space is y-down, origin top-left, 1 unit = 1 logical px
at 100%. `screen = world * scale + offset`. Matches Wayland, SVG, PDF and PNG; the only flip is world→clip in
the shader. A test asserts "top stays top" across canvas, overlay, SVG, PDF, PNG.

**Threads:**
- *Main thread:* input → tools → `Command` → document mutation → `request_redraw()`. Never blocks: no disk
  I/O, no recognition, no export.
- *Render:* on `RedrawRequested` only (no continuous loop; idle = no frames). Live stroke appends only the new
  tail to the GPU buffer.
- *Store thread:* receives `(sketch_id, revision, Arc<snapshot>)`, debounces, writes one transaction.
  Save failure → persistent in-app banner, never just a log line.
- *Worker pool (rayon):* recognition, style extraction, export, thumbnails. Results carry
  `sketch_id + revision`; stale results are dropped.
- *One-way intents:* shortcuts, menus and toolbar fire actions on the focused canvas controller. No mirrored
  state flags.

**Render pipeline per frame:** clear → grid (full-screen triangle, world position from the inverse view) →
committed strokes (instanced SDF capsules; tiles above 400k visible segments) → live stroke →
recognised-shape / wireframe overlay at user opacity → selection, handles, eraser ring. SDF gives analytic AA
at every angle, so no MSAA. Vertex attribute offsets come from `offset_of!`, never typed by hand.

## 3. What carries over from the Swift app

Port as logic (re-typed in Rust, names kept so docs/copy carry over): `StrokePoint`, `VectorStroke`,
`ShapeType` (15 cases), `RecognizedShape`, `RecognitionQualityStatus/Report`; `SketchRecognitionActor`
heuristics (`isApproximatelyLinear`, `isApproximatelyCircular`, `hasCorners` 0.7 threshold,
`detectCornerRadius`, `detectCompositeShapes`, `promoteToUIElement`) plus Ramer–Douglas–Peucker before corner
detection; the recognition quality gate; `StyleMirrorActor` maths (palette clustering, accent/background,
spacing base unit, radii, contrasting text) plus a WCAG 2.1 AA contrast check (Gemini); `StyleTypes`;
export layout + `ExportOptions`; Project → Sketches → Wireframe + style presets.

Redesigned: full-bleed canvas with collapsible sidebar/inspector; wireframe as an overlay with opacity slider
(plus optional side-by-side); pressure only for real styluses; shape auto-straightening opt-in ("Snap
shapes", off by default, own undo step).

Dropped: Vision raster rectangle/text detection (recognition runs on vectors; OCR post-v1), CloudKit,
MenuBarExtra, dead services. Mac data import (`CanvasData` JSON, flip y) only if Alan wants it.

## 4. Bugs from the Swift app to design out (each gets a guard test)

| # | Bug | Prevention | Guard test |
|---|---|---|---|
| 1 | Eraser edits never saved | All mutations are `Command`s; store watches document revision | erase → flush → reopen → gone |
| 2 | Undo leaks across sketches | `History` owned by the sketch session, dropped on switch | draw A, switch B, undo → both unchanged |
| 3 | Empty/corrupt data shows previous sketch | `load()` → `Result`; empty = default doc; corrupt = banner + read-only | new sketch after a full one is empty |
| 4 | Silent in-memory store fallback | No fallback; blocking dialog Retry / Choose location / Quit | unwritable DB path → dialog, no drawing |
| 5 | `try?` saves, 60 s autosave | 300 ms debounce, flush on switch/close/SIGTERM, errors surfaced | kill -9 1 s after stroke → stroke present |
| 6 | Undo off by one | Commands store their inverse | draw A,B; undo×2 empty; redo×2 A+B |
| 7 | Ctrl+Z not wired | Action with `Ctrl+Z`, `Ctrl+Shift+Z`, `Ctrl+Y`; labels "Undo Erase" | action enabled follows history |
| 8 | Wrong vertex attribute offset | `#[repr(C)]` + `Pod`, `offset_of!` | **in scaffold:** `segment_layout_is_tight` |
| 9 | Grid not world-space / wrong on HiDPI | Grid from inverse view + fractional scale | golden at 1.0 / 1.25 / 2.0 |
| 10 | Direction-dependent AA, NaN on duplicate points | SDF capsules; duplicates dropped at input | **in scaffold:** `duplicate_points_dropped…`; golden H/V/diag |
| 11 | Wireframe and SVG flipped | One y-down space (§2) | **in scaffold:** `draws_stroke_offscreen`; "top stays top" across outputs |
| 12 | Horizontal pan inverted | `pan_by_screen` single sign convention | **in scaffold:** `pan_right_moves_content_right` |
| 13 | Space release → always pen; key repeat | Tool machine holds `previous_tool`; repeats ignored | Space while eraser → release → eraser |
| 14 | Eraser tests vertices only; fast drags skip | Segment distance along the swept path | erase rect by touching mid-edge |
| 15 | Every stroke auto-straightened | Opt-in, undoable | default stroke stays raw |
| 16 | O(n²) copies, per-frame allocs, whole-doc JSON per stroke | Append-only live buffer, per-stroke GPU cache, row-per-element writes | bench: append < 1 µs/sample |
| 17 | Thread-unsafe state; focus stealing | Document on main thread, workers get `Arc` snapshots; canvas takes focus only on click | compile-time; manual |
| 18 | Fake Zoom to Fit | Fit uses content bounds | fit test |
| 19 | Sidebar can't browse sketches | Two-level sidebar from day one | manual |
| 20 | Read-only inspector | Editable inspector (Phase 5) | manual |

## 5. Performance and UX

Budgets: [`PERFORMANCE-BUDGET.md`](PERFORMANCE-BUDGET.md). UX rules: [`UX-PRINCIPLES.md`](UX-PRINCIPLES.md).
Both are acceptance criteria, not aspirations.

## 6. Phases

Estimates are focused developer-days. Every phase ends with
`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green plus its proof.
Order follows the Claude plan (core and persistence before features: data loss was the Mac app's worst bug);
the scaffold already covers the start of Gemini's "canvas first" Phase 1.

| Phase | Scope | Est. | Done when |
|---|---|---|---|
| **0 Spike** | S2k scene (2k strokes/200k points) in raw winit vs iced `shader`; tablet pressure on Hyprland via winit, else sctk; present-mode and latency measurement | 2–3 d | One-page spike report with numbers in the vault; shell + tablet path chosen |
| **1 Core + store** | Document/element/commands/history/viewport, `nc-store` (WAL, debounce, flush, error surfacing) | 4–5 d | Bugs #1–7, 11, 12, 18 tests pass; kill -9 durability harness; proptest undo-all = original |
| **2 Canvas MVP** | Per-stroke GPU caches, tile cache, live stroke ring, one-euro smoothing, pressure, temporary hand, undo/redo, fit/100%, pinch | 5–7 d | S2k meets every frame/latency budget (tracy + camera numbers attached); goldens H/V/diag + grid at 1/1.25/2; recording draw→undo→redo→pan→pinch→fit |
| **3 Editing tools** | Select (click/marquee/lasso), move, nudge, delete, duplicate, copy/paste, rect/ellipse/line, swept eraser with ring cursor, rstar | 4–6 d | Each interaction unit-tested and in a recording; rect erases from any edge; moving 500 strokes holds 120 Hz |
| **4 Recognition + export** | Port heuristics + `promoteToUIElement`, quality report, overlay + opacity, side-by-side, SVG/PDF/PNG/JPEG raw + wireframe | 5–6 d | ≥ 50 recorded fixtures; accuracy ≥ Swift baseline (target > 88%); recognition off-thread with no dropped frames; "top stays top" across outputs; files open in Inkscape, Firefox, a PDF viewer |
| **5 Shell + polish** | Shell in our own wgpu chrome layer (iced only if the §1 re-check passes): two-level sidebar with thumbnails, editable inspector, floating tool palette, zoom HUD, empty-canvas hint, toasts, preferences, shortcuts overlay, light/dark, a11y, copy review | 5–7 d | Every UX principle checked with screenshots/recordings on Hyprland tiled + floating; no internal strings in UI |
| **6 Style mirror** | Screenshot import (file + paste), palette/spacing/radius/line-height, WCAG check, apply with warnings, presets | 3–4 d | 5 real screenshots give palettes a human agrees with; extraction off-thread at 120 Hz |
| **7 Packaging** | AUR `neuralcanvas` + `neuralcanvas-bin`, AppImage, `.desktop`, metainfo, icons, `.sha256` per artefact | 3–4 d | Clean Arch VM installs from AUR; AppImage runs on Arch + Fedora/Ubuntu; scripted flow loses nothing; `sha256sum -c` passes |

**Total ≈ 31–42 days.** Minimum lovable build: Phases 0–3. Phase 4 is where it becomes NeuralCanvas.

**Out of scope for v1:** collaboration, sync, code generation, OCR, raster sketch import, plugins,
Windows/macOS, Flatpak (good later candidate), rotation, smart guides, minimap, extra themes beyond
light/dark (Gemini's Catppuccin/Nord/OLED are post-v1 unless Alan pulls them in).

## 7. Testing

- **Unit (nc-core):** commands/history, viewport, hit-test, tools, smoothing, recognition, style.
  `proptest`: no NaN, world↔screen round-trip, undo-all restores the document.
- **Persistence:** temp-dir SQLite, kill -9 harness, schema migrations, corrupt-blob handling.
- **Golden rendering:** `nc-render` offscreen (already in scaffold) on llvmpipe in CI (±2/255, ≤ 0.1 %
  pixels) and the Arc GPU locally.
- **Input replay:** recorded pointer/stylus fixtures fed to the controller in tests only. No replay flag or
  debug panel ships in the binary.
- **UI smoke:** headless Sway/Weston in CI, Hyprland locally; ~5 flows.
- **Perf:** `criterion` benches in CI (alert > 10 % regression); GPU/latency numbers from the reference
  machine attached to perf PRs.

## 8. Packaging

App id `sh.nodaysidle.NeuralCanvas` (Wayland app_id, `.desktop`, icons, AppStream). Data in
`$XDG_DATA_HOME/neuralcanvas/neuralcanvas.db`, config in `$XDG_CONFIG_HOME/neuralcanvas/`.
AUR source + `-bin` packages (`depends=(vulkan-icd-loader wayland libxkbcommon)`), lint with `namcap`,
build in a clean chroot. AppImage: no GTK to bundle, so binary + desktop file + icon with the host's Vulkan
loader; build in an Ubuntu 24.04 container for glibc reach. Every artefact gets a `.sha256`
([`RELEASING.md`](RELEASING.md)). Release CI drafts the GitHub Release; a human publishes.

## 9. Open questions for Alan

1. Amend the PRD: Linux-only, or Linux plus the Mac app on life support?
2. Repo name/location: this folder as `nodaysidle-neuralcanvas-linux`?
3. Import Mac `CanvasData` JSON, or start fresh?
4. Which pen tablet is on the reference machine (Wacom/Huion/XP-Pen)?
5. App id domain: `sh.nodaysidle` or other?
6. AppImage required at launch, or AUR first?
