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
- **Tile-cache threshold:** ~~400k~~ **600k visible segments** (≈ 6k strokes), re-measured in Phase 2 on
  the per-stroke renderer: all-visible GPU p99 is 1.33 ms at 2k strokes, 2.49 ms at 5k, 4.78 ms at 10k
  (~0.47 ms per 100k segments), so 4 ms is crossed near 830k; 600k keeps 25 % for overlay and selection.
  S2k (200k) is 3× under it, so no tile cache is built yet.

### Phase 2 decisions (2026-10-25, [`reports/phase-2.md`](reports/phase-2.md))

- **Frame scheduling:** draw at `vblank + phase`, with `phase` learnt from `wp_presentation` feedback
  (`nc_core::FrameScheduler`: creep 0.25 ms per 60 on-time frames, back off 1 ms and cap below any
  miss). Settles at 3.5 ms on the reference machine and cuts real pointer event → scanout by ~3 ms.
- **Input-to-photon:** event → scanout measured at p50 19.0 / p95 23.2 ms with that schedule. Hyprland's
  default pipeline shows a frame two refreshes after its sample, so ≤ 20 ms p95 is out of reach from the
  app alone at 120 Hz; open question 7 (§9).
- **Wayland side channel:** `crates/nc-wayland` (presentation feedback + `zwp_tablet_v2`) is shared by the
  app and the bench. It must never read the socket itself (that starves winit's own events).

### Decisions after Phase 2 review (2026-10-25)

- **Ink latency (was q7):** predicted ink, not a budget change. The live tip is extrapolated one refresh
  ahead; the unpredicted 23.2 ms p95 stays documented in `reports/phase-2.md` as the raw baseline.
- **Zoom shortcuts (was q8):** `Ctrl+0` fit, `Ctrl+1` 100 % (UX-PRINCIPLES §4), as shipped.

### Phase 3 decisions (2026-10-25, [`reports/phase-3.md`](reports/phase-3.md))

- **Moves and resizes are lifted onto the GPU:** the document is untouched while dragging; the renderer
  draws the selection through the drag's affine (per-instance flag + view uniform) and the edit is
  committed once on release. Per-frame cost no longer depends on selection size (500 strokes: 119 fps).
- **Spatial index:** `rstar` over element bounds with deferred, per-element-coalesced updates applied
  at the next query; an id → index map makes edits by id O(1).
- **Selection semantics:** click picks the topmost ink (4 px); marquee and lasso select what they fully
  enclose; Shift adds / toggles; Alt+drag is the lasso.
- **Eraser:** cuts at the disc's edge along the swept path; the first piece keeps the id and z-position;
  a cut shape becomes stroke pieces; one drag is one undo step.
- **Predicted ink:** linear velocity over the last 25 ms, one refresh ahead, capped at 24 px, drawn in the
  overlay only. Measured tip error p95 1.3 px (8.5 px unpredicted); camera confirmation still open.
- **Clipboard:** `wl-clipboard-rs` on a worker thread; offers `application/x-neuralcanvas`, SVG and PNG
  (2×, canvas background); pastes only its own format.
- **`nc-export` arrived in Phase 3** (SVG writer + PNG via `resvg`), because copy needs it; Phase 4 adds
  PDF and the wireframe.
- **Frame scheduler:** frames whose work exceeds twice the running average plus 1 ms do not tune the
  phase (a slow one-off frame is not a late phase).

### Phase 4 decisions (2026-10-25, [`reports/phase-4.md`](reports/phase-4.md))

- **`ShapeType`:** the Swift app's 15 cases; Rectangle, RoundedRectangle, Circle, Ellipse, Line, Arrow,
  Unknown appended after the original 8 (owner decision; append-only keeps stored blobs readable).
- **Fixtures:** scripted hand-like drawings recorded through the real app (owner decision): a tuning
  set (seed 4) and a held-out set (seed 5), 60 each. Accuracy is asserted on both. The Swift heuristics
  are kept as a port (`recognize::swift`) and scored as the baseline (51.7 % held out, ours 96.7 %).
- **The wireframe is derived data:** recomputed off-thread for the current revision while visible,
  never stored and never in undo; stale results are dropped. Editing it is Phase 5.
- **One drawing source:** canvas overlay and wireframe export both draw from `recognize::draw` marks.
- **Semantics changed from Swift:** an image is a box with an X (an empty square is a rectangle); a
  single label's position decides button (centred) vs input (left).
- **Export:** SVG is the source; PDF via `svg2pdf`, PNG/JPEG via `resvg` (pinned to 0.45 to share
  `usvg`). Ctrl+E / Ctrl+Shift+E write all four to `~/Pictures/NeuralCanvas/` until the Phase 5 dialog.
- **Worker threads run at `SCHED_IDLE`:** recognition, export and clipboard never compete with the
  canvas.

**Crates (name the library, don't hand-roll):** `glam` (math), `kurbo` (curves, flattening),
`rstar` (R-tree hit-test/culling), `ramer_douglas_peucker` (RDP), `bytemuck` (`#[repr(C)]` + `Pod` GPU structs), `rusqlite` (bundled,
WAL) + `serde` + `postcard` + `uuid` (persistence), `rayon` (workers), `crossbeam-channel`,
`resvg`/`usvg` (PNG export), `svg2pdf` (PDF export), `image` (JPEG, screenshot import),
`kmeans_colors` + `imageproc` (style mirror), `tracy-client` behind a non-default cargo feature (profiling),
`proptest`, `insta`, `criterion` (tests/benches).

Export choice: Claude used cairo (a GTK dependency we no longer have); Gemini used usvg/resvg/printpdf.
Merged: **write SVG ourselves (single source of truth), PDF via `svg2pdf`, PNG via `resvg` or our own
offscreen wgpu renderer, JPEG via `image`.**

### Phase 5 decisions (2026-10-25, [`reports/phase-5.md`](reports/phase-5.md))

- **Shell:** own wgpu chrome layer on raw winit (iced 0.14 still pins wgpu 27). `nc-shell` is pure
  layout: it emits a `UiScene` (rects, images, text) and `Region`s, which drive hit-testing, Tab order
  and the AccessKit tree. Text is glyphon 0.12. Fonts are Manrope (static instances cut from the
  variable font) and DM Mono, bundled, SIL OFL. The visual rules are in `DESIGN.md`: no shadows (1 px
  border), one accent, amber and red for state only.
- **Light theme:** ink lightness is inverted in the shader with hue kept (`c + 1 − 2Y`, its own
  inverse). Wireframe and chrome are not inverted; they use their own light colours. Exports follow
  the canvas theme.
- **Wireframe edits:** a type or label edited in the inspector is stored as a pinned
  `RecognizedShape` element that overrides recognition for its source strokes (confidence 1).
  Store schema v3 adds labels.
- **Accessibility:** AccessKit (`accesskit_winit` 0.34) to AT-SPI; the window is created hidden so the
  adapter exists before it is first shown.
- **Store failure (bug #4):** no exit and no fallback. The app opens on an empty, read-only canvas
  under a blocking Retry / Choose location / Quit dialog. Retries run on a worker. A chosen folder
  (XDG file chooser portal via `ashpd`) is remembered in `settings.json`.
- **Damaged sketch (bug #3):** opens read-only with every row that still decodes (`Db::load_readable`).
- **Preferences:** `$XDG_CONFIG_HOME/neuralcanvas/settings.json`, written atomically on a worker:
  theme, grid, smoothing, pressure curve, sidebar and inspector open state, store folder.
- **Raster export cap:** 8,192 px on the long side.

### Phase 6 decisions (2026-10-25, [`reports/phase-6.md`](reports/phase-6.md))

- **Palette:** `kmeans_colors` (no default features; `palette_color`) with k = 16 in CIELAB on up to
  120k sampled pixels; each cluster snaps to its most frequent exact pixel, so flat UI colours come
  back as painted. k = 10 lost low-share text colours on real screenshots.
- **Roles:** background is the colour most common along the image's edges (not the largest share:
  cards can outweigh the page); text is the colour of short thin strokes that contrasts most with it;
  accent the largest-share colour of Lab chroma ≥ 20; surface the largest neutral within 20 L of the
  background. A role the screenshot doesn't show is derived and the inspector says so.
- **Geometry:** boxes are single-colour regions that fill their bounding box, sit inside the window and
  aren't strips; each box size votes once (a canvas grid is one vote). Radius comes from the uncovered
  corner area, `r = √(A / (1 − π/4))`; sharp boxes win only if fewer than a quarter are rounded.
  Spacing is 8 or 4 from gaps and insets between boxes; 8 is assumed with fewer than four.
- **Applying:** the style paints the wireframe as a filled mockup (`draw::paint`, shared by canvas and
  SVG export): backdrop, surfaces, accent buttons, the style's radius; uncertain elements keep their
  amber outline. Fills are drawn by the canvas shader as rounded boxes and ellipses (`FILL`, `ELLIPSE`
  instance flags) and are half strength while the raw sketch shows too. Spacing only pads the
  backdrop and line height is reported, not applied.
- **Storage:** schema v4: `style_presets` table and `sketches.style`, postcard-encoded `Style`.
- **Threads:** decoding (`image`, PNG and JPEG only, 16384 px and 1 GiB limits) and extraction run on
  a `style` worker at `SCHED_IDLE`; the portal picker runs there too.

### Phase 7 decisions (2026-10-25, [`reports/phase-7.md`](reports/phase-7.md))

- **One binary for every artefact:** built from the source tarball in an ubuntu:24.04 container
  (glibc 2.39), shared by the binary tarball, `neuralcanvas-bin` and the AppImage. The source
  `PKGBUILD` builds its own with `options=('!lto' '!debug')` (makepkg's GCC LTO breaks the bundled
  SQLite link; Cargo already strips).
- **AppImage libraries:** the Vulkan loader and xkbcommon are bundled as a fallback that `AppRun`
  uses only when the host lacks them. libwayland is not bundled, against the Phase 7 brief: the
  host's Vulkan driver needs the host's libwayland (Ubuntu's 1.22 made Mesa on Arch fail to load:
  `undefined symbol: wl_display_create_queue_with_name`).
- **Dependencies declared by hand:** wayland, libxkbcommon and the Vulkan loader are loaded at run
  time, so namcap can't see them and warns that they "may not be needed"; they are.
- **Checks in containers:** `packaging/check-arch.sh` (makepkg, namcap, clean pacman install and run)
  and `packaging/check-appimage.sh` (Ubuntu 24.04, Fedora 41), against the host's Wayland session.
  `packaging/smoke.sh` drives the release flow and compares the store before and after a relaunch.
- **Homepage:** `https://github.com/nodaysidle` until the repository's public name is settled (§9 q2).
- **Licence:** `LICENSE` (MIT, as `Cargo.toml` declared); the bundled fonts' OFL texts ship next to it.

## 2. Architecture

```
crates/
  nc-core/     document, element (Stroke | Shape | RecognizedShape), commands + history, viewport,
               hit-test (rstar), input (one-euro filter, pressure curve), tools state machine,
               recognize/, style/. No GPU, no window. 100% headless-testable.
  nc-render/   wgpu renderer drawing into any TextureView: grid, committed strokes (per-stroke GPU caches,
               tile cache above 600k visible segments), live stroke fast path, wireframe overlay, selection chrome.
  nc-store/    rusqlite WAL; store thread, 300 ms debounce, flush on switch/close/SIGTERM.
  nc-export/   SVG writer, svg2pdf, PNG, JPEG; raw sketch AND wireframe export.
  nc-shell/    shell layout (no GPU, no window): bar, palette, sidebar, inspector, toasts, dialogs, copy,
               palette + WCAG check; emits a UiScene and hit/focus/accessibility regions.
  nc-app/      `neuralcanvas` binary: window, input routing, actions/shortcuts, shell state, AccessKit.
  nc-wayland/  side channel on winit's wl_display: presentation feedback, tablet-v2 tool input.
  nc-bench/    measurement harness (S2k runs, `wp_presentation` timing, virtual-pointer input latency,
               tablet probe, save cost). Never packaged.
  spike-iced/  Phase 0 iced 0.14 host spike, kept as the rerun harness for the shell decision.
packaging/     PKGBUILD, PKGBUILD-bin, .desktop, metainfo.xml, icons, AppImage recipe.
```
Present after Phase 7: `packaging/` (desktop file, metainfo, icons, PKGBUILDs, AppImage recipe,
release and check scripts). Present after Phase 6: `nc-core` (with `recognize` and `style`), `nc-render` (canvas + UI pass), `nc-store` (with style presets), `nc-export` (SVG, PDF, PNG, JPEG; raw and wireframe, plain or styled), `nc-shell`, `nc-wayland`, `nc-app`, `nc-bench`, `spike-iced`.

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
committed strokes (instanced SDF capsules from a per-stroke arena; tiles above 600k visible segments) → live stroke →
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
AUR source + `-bin` packages (`depends=(vulkan-icd-loader wayland libxkbcommon hicolor-icon-theme)`),
lint with `namcap`, build in a clean container. AppImage: no GTK to bundle, so binary + desktop file +
icons, the Vulkan loader and xkbcommon only as a fallback, libwayland and the driver from the host;
built in an Ubuntu 24.04 container for glibc reach (`packaging/release.sh`). Every artefact gets a `.sha256`
([`RELEASING.md`](RELEASING.md)). Release CI drafts the GitHub Release; a human publishes.

## 9. Open questions for Alan

1. Amend the PRD: Linux-only, or Linux plus the Mac app on life support?
2. Repo name/location: this folder as `nodaysidle-neuralcanvas-linux`?
3. Import Mac `CanvasData` JSON, or start fresh?
4. Which pen tablet is on the reference machine (Wacom/Huion/XP-Pen)?
5. App id domain: `sh.nodaysidle` or other?
6. AppImage required at launch, or AUR first?
7. ~~Input-to-photon vs Hyprland's pipeline~~: resolved 2026-10-25, predicted ink (§1, decisions after
   Phase 2). Open part: a 240 fps camera run to confirm the derived ~14 ms p95.
8. ~~Zoom shortcuts~~: resolved 2026-10-25, `Ctrl+0` fit / `Ctrl+1` 100 %.
