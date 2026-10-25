# Phase 4 report: recognition, wireframe overlay, export

Date: 2026-10-25. Reference machine as before (Intel Arc iGPU, Mesa 26.2.5, Hyprland 0.56.2, 2560×1440 @
119.998 Hz, scale 1.0). Release builds, Rust 1.98.1.

## Read first

1. **Accuracy: 96.7 % on fixtures the recognizer was never tuned on, against 51.7 % for the Swift
   heuristics on the same drawings.** There are 120 fixtures, all drawn through the real app:
   - Set 1 (seed 4, 60 fixtures): I tuned the recognizer against it.
   - Set 2 (seed 5, 60 fixtures): held out, recorded after tuning, and nothing was changed after
     scoring it.

   Both sets come from the same scripted generator, as decided. The held-out set guards against
   overfitting to particular drawings, not to the generator's style. A set drawn by a person is still
   the real test.
2. **There was no Swift accuracy number to beat.** The Mac app had no recognition fixtures or tests.
   "The Swift baseline" here is a rule-for-rule port of its stroke recognizer (`recognize::swift`),
   scored on the same fixtures. It also documents two Swift bugs:
   - Squarish rectangles came out as circles: images, cards and containers.
   - Ellipses failed its circle test.
3. **The 15 `ShapeType` cases are Swift's** (your decision): the 7 appended after the existing 8 are
   Rectangle, RoundedRectangle, Circle, Ellipse, Line, Arrow and Unknown. Postcard encodes variants by
   index, so appending keeps every stored sketch readable.
4. **Firefox and Inkscape aren't installed here.** I verified exports with:
   - librsvg (GNOME's SVG renderer), ImageMagick and resvg;
   - poppler and the Evince window for the PDFs.

   Headless Brave hung in this environment, even with a throwaway profile. To see the files in
   Firefox and Inkscape: `sudo pacman -S inkscape firefox`.

## Done-criteria

| Criterion | Status | Evidence |
|---|---|---|
| ≥ 50 recorded stroke fixtures | Pass | 120: `nc-core/tests/fixtures` (60) and `fixtures-holdout` (60), drawn through the app by `crates/nc-bench/record-fixtures.sh` |
| Accuracy ≥ the Swift baseline, target > 88 % | Pass | Held-out 96.7 % (Swift port 51.7 %); tuning set 96.7 % (Swift port 60.0 %). Asserted by `nc-core/tests/recognition.rs` |
| Recognition off-thread, no dropped frames on S2k | Pass, see note | Recognizing S2k back to back on a worker for 30 s: 0 intervals over 16.6 ms; the share over 8.33 ms matches runs without recognition (table below) |
| "Top stays top" across canvas, overlay, SVG, PDF, PNG | Pass | Offscreen canvas tests (raw and wireframe layer); `nc-export` tests on PNG, JPEG, wireframe PNG, and the PDF rasterised by poppler |
| Exports open in viewers (screenshots) | Pass, with substitutes | `phase-4/exports-in-viewers.png` (librsvg, poppler, PNG, JPEG), `phase-4/pdf-in-evince.png`. Not Inkscape or Firefox (item 4 above) |
| fmt, clippy `-D warnings`, tests | Pass | 109 tests, 0 failures; clippy clean with and without `--features profile` |

**Note on "no dropped frames":** the strict reading (no interval over 8.33 ms at all) isn't met even
without recognition. A 30 s run always has a handful of single-refresh misses, under the < 1 % budget
line. What I measured is that recognition adds none.

## Accuracy

The **largest** recognised element must have the fixture's type: a card's inner scribbles don't count
against it.

| Type | Fixtures per set | Swift port, tuning / held-out | NeuralCanvas, tuning / held-out |
|---|---|---|---|
| Arrow | 4 | 0 / 0 | 4 / 3 |
| Button (incl. 2 four-stroke) | 8 | 5 / 5 | 6 / 7 |
| Card | 5 | 1 / 0 | 5 / 5 |
| Circle | 4 | 4 / 4 | 4 / 4 |
| Container | 4 | 0 / 0 | 4 / 4 |
| Divider | 4 | 4 / 1 | 4 / 4 |
| Ellipse | 4 | 3 / 1 | 4 / 4 |
| Icon | 4 | 3 / 4 | 4 / 4 |
| Image (box with an X) | 4 | 0 / 0 | 4 / 4 |
| Input field | 5 | 5 / 5 | 5 / 5 |
| Line | 4 | 4 / 4 | 4 / 4 |
| Rectangle (incl. four-stroke) | 4 | 1 / 1 | 4 / 4 |
| Text | 6 | 6 / 6 | 6 / 6 |
| **Total** | **60** | **60.0 % / 51.7 %** | **96.7 % / 96.7 %** |

Remaining misses:
- **Three wide buttons drawn without a label** came out as input fields. A 160×36 box with nothing in
  it is ambiguous. With a label, position decides: centred is a button, left-aligned an input.
- **One level single-stroke arrow** (held out) read as handwriting.

What changed from Swift:
- **Closed strokes:** rectangles are recognised by how much of their minimum-area rotated box they
  fill, so tilt doesn't matter. Ellipses by radius error, tested first, so small coarsely sampled
  circles aren't taken for rounded squares. RDP keeps corners for arrows.
- **Open strokes:** handwriting is told from lines by thickness and path/chord ratio.
- **Strokes joined into one element:**
  - four lines meeting at near-right angles → a rectangle
  - a box with two crossing lines → an image
  - a line plus a chevron at one end → an arrow
- **Rules:** Swift's size rules are kept but applied to rotation-free sizes. Two deliberate changes:
  an empty square is no longer an image (an image is a box with an X, the wireframe convention), and
  a single label inside a field-sized box decides button versus input.

## What shipped

- **`nc-core::recognize`.**
  - Types: `Recognized`, `Wireframe`, and the Swift quality gate (`QualityReport` with the same
    thresholds; Ready / Review recommended / Low confidence).
  - The Swift port, the new recognizer, `geometry::rdp` (the `ramer_douglas_peucker` crate) and the
    fixture format.
  - `draw`: the marks that both the canvas and export draw from, so the two can't disagree. Each
    element type draws its wireframe convention:

    | Type | Draws as |
    |---|---|
    | Button | Rounded box with a centred label bar |
    | Input field | Box with a placeholder bar at the left |
    | Image | Box with an X |
    | Text | A bar |
    | Arrow | Shaft and head |

    Rectangles are drawn straightened. Uncertain elements are drawn in amber.
- **Recognition worker.**
  - Its own thread, on a crossbeam channel, at `SCHED_IDLE` priority.
  - It always works on the newest request, and results carry `(sketch_id, revision)`. A result for an
    older revision is dropped; the overlay keeps the last current one.
  - It runs only while the wireframe is visible.
  - The wireframe is derived data: it isn't stored and isn't in undo, so recognition never adds
    "Undo Recognize" steps.
- **Wireframe overlay.**
  - **W** cycles raw → hybrid → wireframe.
  - **`[` / `]`** step opacity by 10 %: the raw sketch fades out as the wireframe fades in, and both
    are fully shown at 50 %.
  - **Shift+W** shows side by side: raw left, wireframe right, the same view, with a divider. Drawing
    works in either pane.
  - The renderer keeps the wireframe in its own GPU buffer, uploaded only when it changes; opacity is
    two numbers in the view uniform.
- **Review uncertain items (U).**
  - The window title shows the quality status and the uncertain count, e.g. "Review recommended ·
    Review 1 uncertain (U)".
  - U zooms (animated, at most 2×) to the next flagged element in reading order and marks it with an
    amber box. It switches to hybrid so you see what you drew next to what was recognised.
- **Export.**
  - Ctrl+E exports the raw sketch; Ctrl+Shift+E the wireframe.
  - Each export writes four files (SVG, PDF via `svg2pdf`, PNG at 2×, JPEG at 2×) to
    `~/Pictures/NeuralCanvas/` (the XDG pictures directory).
  - Files are numbered so nothing is overwritten, on a worker thread. The title reports the path or
    the error.
  - `resvg` is pinned to 0.45 so it shares `usvg` with `svg2pdf` (one SVG parser in the binary).
- **Bench and tools.**
  - `nc-bench fixtures` generates hand-like drawings as drive scripts and extracts what the app saved
    into fixtures.
  - `record-fixtures.sh` runs the loop, one fresh app and database per page.
  - `nc-bench recognize` times S2k.
  - `nc-bench window … recognize` keeps recognition running during a frame-pacing run.

## Numbers

| Measure | Result | Budget |
|---|---|---|
| Recognition of S2k (2,000 strokes), one worker | p50 54.9 ms, max 55.2 ms (10 runs) | ≤ 300 ms |
| Same, back to back during a 30 s frame run | p50 56.2–56.7 ms, max 104 ms | — |

Frame pacing with S2k, fullscreen, Fifo, adaptive schedule, 30 s each:

| Scenario | Recognition | CPU per frame p99 | Intervals > 8.33 ms | > 16.6 ms |
|---|---|---|---|---|
| Pan/zoom | off | 1.14 ms | 7 (0.19 %) | 0 |
| Pan/zoom | on, back to back | 1.43 ms | 8 (0.22 %) | 0 |
| Draw | off | 1.18 ms | 17 (0.47 %) | 0 |
| Draw | on, back to back | 1.19 ms | 7 (0.19 %) | 0 |

**First runs were contaminated.** Another process (a Java daemon not started by me) was using about
4.6 cores. During it:
- pan/zoom with recognition dropped to 80 fps (48 % of intervals late);
- pan/zoom without recognition also degraded (2.2 % late);
- recognition itself spiked to 516 ms.

The table above was measured after that load ended. The app's worker threads now run at idle priority,
so on a busy machine recognition and export slow down rather than the canvas.

## End-to-end run in the real app

Release build on workspace 9, with a throwaway data folder and pictures folder. I drew one fixture page
(an input and five cards) plus an open spiral through the virtual pointer, then pressed W, W, Shift+W,
U, Ctrl+E and Ctrl+Shift+E.

- `phase-4/modes.png`: raw, hybrid (the spiral flagged amber), wireframe only.
- `phase-4/side-by-side-and-review.png`: side by side with the divider, then review zoomed to the
  flagged spiral in hybrid.
- The title read "Wireframe 50 % · Review recommended · Review 1 uncertain (U) · Uncertain item 1 of 1".
- The two exports produced 8 files (Sketch 1 and Wireframe 1, in four formats). The verification sheet
  shows them through librsvg, poppler, and the PNG and JPEG as written. Evince opened both PDFs
  (`pdf-in-evince.png`).

## Bugs found and fixed this phase

- **Shape-tool rectangles** were recognised from their two drag corners. A box around two points has no
  height, so they came out as text. The wireframe-export test caught it; the box now comes from the
  shape's outline.
- **Fixture generator, sampling:** it restarted its step at every path segment, so circles built from
  short segments came out empty. Caught by looking at the recorded fixtures.
- **Fixture generator, speed:** it "wrote" handwriting at 600–1,500 px/s, about 2.5 samples per
  letter, which the ink smoothing flattened. Scribbles and small loops now go at realistic speed.
  Both recognizers were re-scored on the re-recorded set.

## Skipped, and why

- **Inkscape and Firefox screenshots:** not installed (Read first, item 4).
- **Editing the wireframe** (change a recognised element's type, nudge it): that is the Phase 5
  inspector. `ElementKind::Recognized` stays in the document model for that.
- **An export dialog, light export theme and preferences:** Phase 5 shell. Exports use the canvas's
  dark background and colours, as Copy does.
- **RoundedRectangle as an output:** the new recognizer records a corner radius but reports these as
  Rectangle (or the UI element they become). The type exists for compatibility and for Phase 5
  editing.
- **The Swift Vision (raster) path:** dropped with the Mac app, as planned (PLAN §3).

## Risks

- **The fixtures aren't human drawings.** A person's sketches will be messier in ways the generator
  doesn't model (lifted pen mid-edge, overdrawn corners, crossed-out strokes). A fixture-recording mode
  for real drawings would give the number that matters.
- **Status and notices live in the window title** until Phase 5 gives them a proper place.
- **Large sketches:** recognition is whole-sketch on every change while the wireframe is visible.
  S2k takes 55 ms; at about 10× S2k it would be about half a second behind the pen, still off the main
  thread. Per-stroke caching would fix that if it shows up.
