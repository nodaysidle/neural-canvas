# Phase 3 report: editing tools

Date: 2026-10-25. Reference machine as before (Intel Arc iGPU, Mesa 26.2.5, Hyprland 0.56.2, 2560×1440 @
119.998 Hz, scale 1.0). Release builds, Rust 1.98.1.

## Read first

1. **Moving 500 strokes holds 120 Hz, after a design change.** The first version edited the document
   on every frame of a drag. It ran at 60 fps, with 4.3 ms of CPU per frame. Now a move or resize
   leaves the document alone until release. The GPU draws the selection through the drag's transform,
   so a frame costs the same however much is selected. The edit is committed once, as one undo step.
2. **Predicted ink is in.** The 20 ms budget passes on a derived figure, not a camera one. The raw
   event → scanout figure is unchanged (p95 22.7 ms) because prediction does not shorten that path.
   It draws the ink tip one refresh ahead. On real compositor-delivered input, the tip lands within
   1.3 px (p95) of where the pointer really is one refresh later, against 8.5 px without prediction.
   Camera confirmation is still open.
3. **A scheduler bug surfaced and is fixed.** A few slow one-off frames (committing a big move) taught
   the frame scheduler that its phase was too late. It ratcheted down to drawing right at vblank and
   then missed ordinary frames too. Frames with unusual work no longer teach it.

## Done-criteria

| Criterion | Status | Evidence |
|---|---|---|
| Every editing interaction has a core unit test | Pass | Table below; 87 tests in the workspace |
| Moving 500 selected strokes holds 120 Hz | Pass | 119.1 frames/s over 30 s, 0.70 % of intervals over 8.33 ms, 0 over 16.6 ms, with 15 real commits (below) |
| A rectangle erases by touching any edge | Pass | `hit::a_rectangle_erases_from_any_edge` (all four edge midpoints); the square's top edge in the recording |
| Scripted recording: select → move → resize → copy/paste → shape → segment erase | Pass | Driven by `nc-bench drive script` (virtual pointer + `wtype`); contact sheet `phase-3/e2e-flow.png` |
| `fmt`, `clippy --all-targets -D warnings`, tests | Pass | 87 tests, 0 failures; clippy clean with and without `--features profile` |
| Committed locally | Pass | This commit; nothing pushed |

Interactions and their tests (all in `nc-core` unless noted):

| Interaction | Tests |
|---|---|
| Click select, Shift toggles, click on empty canvas clears | `select::click_selects_shift_click_toggles_empty_click_clears` |
| Pick takes the topmost ink, not the topmost bounding box | `hit::pick_takes_the_topmost_ink_not_the_box` |
| Marquee (fully enclosed), lasso with Alt, Shift adds | `select::marquee_and_lasso_select_on_release`, `hit::marquee_and_lasso_take_only_enclosed_elements` |
| Move: relative to the press, document untouched until release, one undo step | `select::moving_is_relative_to_the_press_and_one_undo_step`, `select::dragging_inside_the_box_moves_even_off_ink` |
| Resize from 8 handles, anchored at the opposite side, never flips | `select::handles_are_found_and_resize_maps_the_box`, `select::resize_gesture_scales_from_the_opposite_corner` |
| Handles 8 px at every zoom | `chrome::handles_stay_8_px_at_any_zoom` (nc-app) |
| Arrow nudge (1, Shift 10), Ctrl+D, Delete / Backspace, Ctrl+A, Esc | `select::delete_duplicate_and_nudge`, `actions::editing_shortcuts` |
| Copy / paste: internal format round trip, paste centred on the pointer | `select::clipboard_round_trip_and_paste_centres_on_the_pointer` |
| SVG / PNG for other apps, y-down ("top stays top") | `nc-export::svg_keeps_y_down_and_native_shapes`, `nc-export::png_renders_the_svg_the_right_way_up` |
| Rect / ellipse / line; Shift squares, circles and 45° lines; Alt from the centre | `shape::*` (4 tests), `element::shapes_trace_their_outline` |
| Splitting eraser: swept path, cut at the disc's edge, ids and z-order kept, exact undo, pressure interpolated | `hit::a_fast_drag_cannot_skip_a_stroke`, `hit::cuts_split_keep_id_order_and_undo_exactly`, `hit::near_misses_do_not_cut_and_full_cover_removes`, `hit::pressure_is_interpolated_at_the_cut`, `hit::a_rectangle_erases_from_any_edge` |
| Eraser ring cursor | `chrome::ring_cursor_is_a_ring` (nc-app) |
| Tool keys V / R / O / L | `tools::tool_keys` |
| Spatial index stays exact under any edits | `document::index_and_tree_follow_every_edit`, proptest `command::indexes_match_brute_force` |
| Shapes, overlay and lifted moves on the GPU | `nc-render::shapes_overlay_and_moves` |
| Predicted ink | `input::prediction_extends_steady_motion_and_stops_with_the_hand`, `input::prediction_error_on_a_curve_is_small` |

## What shipped

- **Select tool (V).**
  - Click picks the topmost ink within 4 px; Shift+click toggles.
  - A drag on empty canvas is a marquee; with Alt it is a freehand lasso. Both select what they fully
    enclose. Shift adds to the selection.
  - Dragging a selected element, or anywhere inside the selection box, moves the selection.
  - Eight round handles, 8 px across at every zoom, resize from the opposite side. The cursor shows
    the resize direction over each handle.
- **Editing keys.** Arrows nudge by 1 world unit (Shift: 10). Ctrl+D duplicates 16 px down-right and
  selects the copy. Delete or Backspace deletes; Ctrl+A selects all; Esc deselects. Undo labels are
  "Move", "Resize", "Nudge", "Duplicate", "Delete", "Paste", "Draw Rectangle", "Draw Ellipse",
  "Draw Line" and "Erase".
- **Copy and paste.**
  - Ctrl+C offers three formats: `application/x-neuralcanvas` (lossless, for pasting back), SVG, and
    PNG at 2× on the canvas colour.
  - A worker thread builds them and serves them through `wl-clipboard-rs`, so the canvas never waits.
  - Ctrl+V reads only our own format and pastes it centred on the pointer, on a worker thread.
  - I checked with `wl-paste` that other apps get a valid PNG (3110×1628) and SVG. The SVG is not
    offered as plain text, so a text field won't paste markup.
- **Shapes (R, O, L).** Rectangle, ellipse and line, with Shift for squares, circles and 45° lines,
  and Alt to draw from the centre. They are stored as real shapes, so SVG export writes `<rect>`,
  `<ellipse>` and `<line>`, and they draw as the same SDF ink as strokes.
- **Splitting eraser (E).**
  - A disc sweeps between samples, so fast drags can't skip ink.
  - Each touched segment is cut exactly where the disc's edge crosses it, so ink ends where the ring
    was. Pressure is interpolated at the cut.
  - The first surviving piece keeps the element's id and z-position; further pieces go right above
    it. A cut shape becomes stroke pieces.
  - The whole drag is one undo step.
  - The cursor is a hardware ring the eraser's size (light ring, dark halo), so it costs no frames.
- **Predicted ink.**
  - The live stroke's tip extends along its velocity over the last 25 ms, one refresh ahead (the
    refresh comes from presentation feedback). The lead is capped at 24 px, so a mis-prediction
    can't draw a spike.
  - It is drawn in the overlay, never stored, and retracts within 30 ms of the hand stopping.
- **Spatial index.**
  - `rstar` over element bounds, used by pick, marquee, lasso and the eraser.
  - Edits only note what changed, and the next query applies those updates once per element. A
    500-stroke commit costs one batch of updates at the next click instead of 500 per frame.
  - An id → index map makes edits by id O(1).
- **Renderer.**
  - Draws shapes.
  - An edit that keeps an element's segment count (any move or resize) is rewritten in place, so it
    leaves no hole.
  - New overlay layer for chrome and predicted ink.
  - A per-instance `lifted` flag plus an affine in the view uniform draws a dragged selection without
    re-uploading it.
  - `render` now takes a `Scene` (doc, live stroke, lifted selection, overlay).
- **`nc-export`, pulled forward from Phase 4.** The SVG writer (single source of truth) and PNG
  rendered from it with `resvg` (no font or raster features). Copy needed both now. Phase 4 adds PDF
  and the wireframe.
- **Bench.**
  - `nc-bench window move` puts S2k fully on screen and drags 500 strokes along a circle. It commits
    and presses again every 2 s, so commit frames are measured too.
  - `nc-bench drive script` reads pointer and key commands from stdin.
  - The `input` scenario now replays the real pointer stream through the predictor.

## Numbers

### Moving 500 strokes (S2k all visible, fullscreen, Fifo, 30 s, `nc-bench window move fifo 30 auto`)

| Version | Frames/s | CPU per frame p50 / p99 | Intervals > 8.33 ms | > 16.6 ms |
|---|---|---|---|---|
| Edit the document every frame (first version) | 60.3 | 4.25 / 9.14 ms | 96.6 % | 45 |
| Same, R-tree updates deferred (10 s run) | 87.3 | 3.15 / 8.55 ms | 33.9 % | 30 |
| GPU-lifted selection, one commit per release | 113.9 | 0.47 / 1.57 ms | 5.27 % | 3 |
| + scheduler ignores one-off heavy frames | **119.1** | **0.52 / 1.39 ms** | **0.70 %** | **0** |

How each step was found:

- **Core edit.** Timing the core edit alone showed 1.16 ms per frame for 500 replaces, almost all
  R-tree remove and insert. Deferring the updates brought it to 0.20 ms. After an edit, the first
  query costs 0.006 ms; rebuilding the tree on query would cost 0.9 ms.
- **Render path.** Re-uploading 49,500 segments every frame cost 2.2 ms p50 and 6 ms p99 inside
  `Renderer::render`. Lifting the selection removed that cost. On commit frames the render max is
  4.6 ms.
- **Scheduler.** With commits disabled the same run had 0.33 % slow intervals, so the commit frames
  were the cause. With commits on, the late frames were ordinary 0.5 ms frames that followed them, and
  the adaptive phase ended at 0. The cause was in the scheduler (Bugs, below).

### Phase 2 budgets re-checked after this phase's changes

| Row | Phase 2 | Now |
|---|---|---|
| S2k pan/zoom, 15 s: intervals > 8.33 ms | 0.11 % | 0.78 % (CPU p99 1.19 ms) |
| S2k draw, 15 s: intervals > 8.33 ms | 0.28 % | 0.33 % (CPU p99 1.16 ms) |
| GPU, S2k all visible, p99 | 1.33 ms | 1.15 ms (instances are now 40 bytes, not 36) |
| Idle | 0 ticks | 0 CPU ticks over 3 s |
| RSS, empty | 59 MB | 59.6 MB |
| Stripped binary | 12.6 MB | 14.2 MB (resvg, wl-clipboard-rs) |

Pan/zoom rose from 0.11 % to 0.78 %. That is still under the 1 % line, but it's worth watching. The
pan/zoom code path didn't change this phase, apart from the scheduler's heavy-frame rule and the wider
instances. I have one run of each, so this could be noise.

### Ink latency (`nc-bench window input fifo 20 auto` + `nc-bench drive 21 500`, 9,540 events)

| | p50 | p95 | p99 |
|---|---|---|---|
| Event → scanout, Phase 2 | 19.01 ms | 23.24 ms | 25.05 ms |
| Event → scanout, now | 18.16 ms | 22.73 ms | 24.33 ms |
| Tip vs pointer one refresh later, last sample only | 7.91 px | 8.47 px | 8.74 px |
| Tip vs pointer one refresh later, **predicted** | **0.74 px** | **1.34 px** | 1.61 px |

The px rows replay the pointer stream the bench received (a 300 px circle at 942 px/s, sampled at
500 Hz) through the app's `Predictor`. They compare the drawn tip with where the pointer really was
8.33 ms later. Prediction removes about 85 % of the visible gap. Unpredicted, the tip trails by one
refresh of motion. Predicted, it sits within a pixel and a half.

Read as time, the tip shown at scanout is about 8.3 ms newer than its sample, so what the eye sees is
about 22.7 − 8.3 ≈ 14.4 ms behind at p95, inside the 20 ms budget. That figure is derived: the
prediction error is measured, the subtraction is not. A camera run (budget table, 240 fps, 20
samples) is still needed to confirm it. The raw baseline stays documented in `phase-2.md`.

## End-to-end run in the real app

Release build on workspace 9, with a throwaway `XDG_DATA_HOME` (the owner's sketches untouched),
driven by a 115-line script (virtual pointer at 250 Hz, keys and held modifiers through `wtype`):

1. Draw three strokes; select one on its ink and move it.
2. Resize it from the south-east handle.
3. Ctrl+C, then Ctrl+V at the pointer.
4. Marquee one stroke; Alt-lasso another.
5. Shift+Right ×3, Ctrl+D, Delete.
6. A Shift square, an Alt ellipse and a Shift-snapped line.
7. Erase through the square's top edge and across the resized stroke.
8. Ctrl+Z, Ctrl+Shift+Z, Ctrl+0.

The database held 9 elements, which is the count the script predicts. After SIGTERM and a relaunch,
the same 9 were on screen. Then I ran Ctrl+A, Ctrl+C and read the clipboard back the way another app
would.

- `docs/reports/phase-3/e2e-flow.png`: 10 frames from the recording, in order: strokes, moved,
  resized, pasted, marquee, lasso, after nudge/duplicate/delete, shapes, erasing (ring visible),
  fitted.
- `docs/reports/phase-3/eraser-ring.png`: the ring next to the gap it cut.
- The 24 s recording (`phase3-flow.mp4`, 3 MB) stayed in the session scratchpad, as in Phase 2. Not
  committed.

## Bugs found and fixed this phase

- **Scheduler ratchet.** Every miss set the phase ceiling below the current phase, and the ceiling
  re-probes only every 3,600 frames. A few slow one-off frames therefore pinned the phase at 0, where
  frames compete with the compositor's own work and miss more. Frames whose work is more than twice
  the running average plus 1 ms now don't tune the phase either way. The new test
  `heavy_one_off_frames_do_not_teach_the_phase` fails without the fix.
- **SVG pasted as text.** `wl-clipboard-rs` adds `text/plain` aliases when an offer looks like text,
  so the SVG source would paste into text fields. They are turned off.

## Skipped, and why

- **Pasting from other apps** (their SVG or PNG): out of scope. The brief asks for SVG/PNG going out;
  raster import is post-v1. Ctrl+V ignores other content without an error.
- **Cut (Ctrl+X):** not in the Phase 3 list. Copy then Delete does the same in two steps.
- **Aspect lock while resizing (Shift on a handle):** not in the list.
- **Square handles:** handles are round. The capsule renderer has no squares, and the 8 px size rule
  is met.
- **Tracy capture and camera latency:** still no viewer and no camera. All numbers come from the
  harness.
- **Pen pressure on a real tablet:** none attached, as agreed. Cuts interpolate pressure (tested
  headless).

## Risks

- **The eraser ring will be blurry on HiDPI outputs.** winit 0.30 gives custom cursors no buffer
  scale. Scale is 1.0 here; this fixes itself with winit 0.31.
- **Lasso is Alt+drag.** UX-PRINCIPLES has no lasso key, and on some desktops Alt+drag moves windows.
  This Hyprland binds window drags to Super, so it works here. A palette toggle in Phase 5 would remove
  the dependency.
- **Hyprland binds middle click globally** here (`mouse:274`, no modifier). I didn't check whether it
  intercepts middle-drag pan in the app. Space and the hand tool are unaffected.
- **Ellipses are flattened at a fixed tolerance** (0.05 world units). Large ellipses show facets above
  about 16× zoom.
- **A commit frame still misses its refresh:** one frame per 500-stroke commit, about 4.6 ms of
  re-upload. That's within budget, and visible only as one repeated frame at release.
