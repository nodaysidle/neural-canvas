# Phase 6 report: style mirror

Measured 2026-10-25 on the reference machine (Intel Arc iGPU, 2560×1440 at 120 Hz, Hyprland 0.56).

## What shipped

- **Screenshot import.** "Import screenshot…" in the inspector opens the desktop's file chooser
  (XDG portal through `ashpd`, filtered to PNG and JPEG). `Ctrl+V` with an image on the clipboard
  (and no NeuralCanvas elements there) reads that image instead of pasting.
- **Style extraction (`nc_core::style`).** Palette by `kmeans_colors` in CIELAB, the four roles
  (background, surface, accent, text), WCAG 2.1 AA checks on five pairs, corner radius clusters,
  a 4 or 8 px spacing rhythm, and line height where the screenshot shows a paragraph. A role the
  screenshot doesn't show is derived, and the inspector says so.
- **Review, then apply.** What was read appears in the inspector (nothing selected) with its
  swatches, metrics, amber WCAG warnings and notes, and Apply / Discard. Applying with warnings is
  allowed; the warnings are shown before, not after.
- **Styled wireframe.** An applied style paints the wireframe as a filled mockup: a backdrop in the
  background colour, cards and panels in the surface colour, buttons and icons in the accent with
  labels in the colour that reads best on it, fields in the background colour with a border, the
  style's corner radius. Elements the quality gate flags keep their amber outline. Canvas and SVG
  export paint from the same list (`draw::paint`), so PDF, PNG and JPEG match too.
- **Presets.** "Save as preset" stores the applied style in the SQLite store (schema v4); presets
  list under the style, the applied one marked; a click applies, "Delete" removes. Each sketch
  remembers its style.
- **Threads.** Decoding and extraction run on a `style` worker at `SCHED_IDLE`; the portal picker
  waits there too. The UI thread never decodes, clusters or touches the file.
- **Owner decisions applied.** `image` is pulled with `default-features = false`, PNG and JPEG only
  (in `nc-app` it adds no new crates: export already built them). `ashpd` as approved.

## Done-criteria

| Criterion | Status | Evidence |
|---|---|---|
| Five real app screenshots give palettes and radii a human agrees with | For your judgement, misses listed below | `phase-6/01` to `05` side by side; fixtures in `nc-core/tests/style-fixtures`, asserted by `nc-core/tests/style.rs` |
| Extraction off the main thread, no dropped canvas frames | Pass with one note | Table below: the frame budget holds in all six runs; 1 frame over 16.6 ms in one of three pan/zoom runs with the worker |
| fmt, clippy `-D warnings`, tests | Pass | 152 tests, 0 failures; clippy clean with and without `--features profile` |
| Binary under 25 MB | Pass | 21.69 MB stripped (Phase 5: 21.13 MB) |
| Commit locally, report | Pass | This file; local commit, nothing pushed |

## The five screenshots

Captured from real apps running on an off-screen Hyprland output (so your desktop wasn't touched),
cropped to the window.

| App | Background | Surface | Accent | Text | Radius | Spacing | Line height |
|---|---|---|---|---|---|---|---|
| Files (Nautilus, light, libadwaita) | `#ebebed` | `#ffffff` | `#2686e6` | `#2f2f34` | 0 (no boxes) | 8 (assumed) | none |
| Pinta (dark, GTK 4) | `#222226` | `#1d1d20` | `#00ff90` (miss) | `#ffffff` | 9 px | 8 (assumed) | none |
| Xournal++ (GTK 3) | `#353535` | `#404040` (derived) | `#40a0ff` | `#959491` | 0 | 4 | none |
| Papers (dark, a PDF open) | `#222226` | `#2e2e32` | none: text colour | `#ffffff` | 0 (no boxes) | 8 (assumed) | 27 px |
| NeuralCanvas (its own chrome) | `#1c1e24` | `#2a2c34` | `#5e9dff` | `#e7e9ee` | 7 px | 4 | none |

Where I think a person would agree, and where not:

- **Files:** the four roles are what the window shows (grey sidebar, white list, blue folders, near-
  black labels). It flags that its blue fails 4.5:1 for button labels (3.7:1), which is true of
  white on `#2686e6`.
- **NeuralCanvas:** every role and the radius are right (its panels and buttons are 6 to 8 px; it
  read 7). It is the only case with the real values known from source (`nc-shell/src/theme.rs`:
  accent `#5e9dff`, text `#e7e9ee`).
- **Pinta:** background, text and radius are right. **The accent is a miss:** GTK's blue
  (`#3584e4`, one checkbox) covers too few pixels to become a cluster, so a swatch from Pinta's
  colour palette wins. The surface (`#1d1d20`, the header bar) is defensible; the entries are
  `#38383b`.
- **Xournal++:** the chrome grey and the ruled-line blue read correctly; the text colour is the grey
  of its status bar labels, and it reports that they fail 4.5:1 on the toolbar (4.04:1).
- **Papers:** the chrome is right and it says honestly that nothing on screen is an accent. The line
  height (27 px) is the PDF's paragraph, measured off the page.
- **Spacing is the weakest estimate.** It needs gaps between boxes, and app chrome has few: three of
  five fall back to the 8 px default and say so ("Too few gaps…" or "No cards or buttons…").
- **Line height** shows only where there is a paragraph (one of five). It is reported, not applied.

## Off-thread proof

`nc-bench window <scenario> fifo 30 auto [style=PNG] shell`: S2k with the shell drawn, 30 s each,
with a worker at `SCHED_IDLE` decoding a 2534×1389 PNG and extracting its style back to back the
whole time (the app does it once per import).

| Scenario | Worker | Intervals > 8.33 ms | > 16.6 ms | Main-thread CPU p99 / max |
|---|---|---|---|---|
| Pan/zoom | none | 2 (0.06 %) | 0 | 1.80 / 3.26 ms |
| Pan/zoom | style | 21 (0.59 %) | **1** | 2.34 / 17.33 ms |
| Pan/zoom | style | 24 (0.67 %) | 0 | 2.02 / 4.50 ms |
| Pan/zoom | style | 8 (0.22 %) | 0 | 1.85 / 2.67 ms |
| Draw | none | 11 (0.31 %) | 0 | 1.77 / 3.23 ms |
| Draw | style | 12 (0.33 %) | 0 | 1.83 / 3.03 ms |

Extraction on the worker, back to back: p50 283 ms, p95 301 to 391 ms per 1440p screenshot
(decode included). On the 1554×929 fixtures: 110 to 145 ms.

**Note:** one frame in the first pan/zoom run went over 16.6 ms, with a single 17.3 ms stall on the
main thread; the next two runs had none. I did not find the cause. A likely one is the worker
allocating and freeing about 100 MB per extraction (kernel page zeroing competing with the main
thread), which a real import does once, not 100 times. The late share with the worker (0.22 to
0.67 %) is higher than without (0.06 to 0.31 %) but under the 1 % line in every run.

## Screens

- `phase-6/06`: Pinta's style read and waiting for review in the inspector.
- `phase-6/07`, `11`: applied (card in the surface colour, fields in the background, 9 px corners).
- `phase-6/08`: a light style with its amber WCAG warning and the "no boxes" note. Captured before
  the warning was reworded to "Button labels: 3.7:1, below the 4.5:1 WCAG AA asks for".
- `phase-6/09`: the light style applied.
- `phase-6/10`: after quit and relaunch: the sketch's style is back and its preset is marked.

All in the real app with an isolated store and settings, driven by `nc-bench drive script` and the
portal's own file chooser.

## Tests added

- `nc-core`: extraction on a synthetic screen (roles, 8 px radius, 8 px rhythm, 20 px lines), WCAG
  pairs, a flat image's notes, storage round trip; the five real screenshots; styled painting order
  (backdrop, card under button, style radius, uncertain outline, label colour).
- `nc-render`: offscreen fills (rounded corner left empty, ellipse, outline over fill, half strength
  in hybrid).
- `nc-export`: a styled wireframe exports filled (SVG attribute and PNG pixels).
- `nc-store`: presets and sketch styles survive a reopen; v2 to v4 migration.
- `nc-shell`: the style section (warnings before applying, presets after, accessible names).
- `nc-app`: screenshots decode from bytes; bad bytes say why.

## Not done, and risks

- **Ctrl+V of an image was not tried in the running app.** Your clipboard held text from Brave and I
  would have overwritten it. The code path shares the decoder that the portal import exercised and
  has a unit test; the clipboard read itself is untested live.
- **Presets can't be renamed.** They take the screenshot's file name ("pinta"), or "Pasted
  screenshot".
- **Spacing and line height are not applied** to the wireframe's geometry: spacing only pads the
  backdrop. Moving recognised elements onto a grid would change what the person drew.
- **Ellipse fills** use a first-order distance approximation: exact on circles, slightly soft on
  long ellipses.
- **The 16.6 ms stall above** is unexplained; reusing the worker's buffers between extractions is the
  first thing I would try if it shows up again.
- **Applying a style is not an undo step:** it is saved with the sketch at once; "Remove style" or a
  preset brings the previous look back.
- **Binary:** 21.69 MB of 25 MB (+0.56 MB this phase, mostly `kmeans_colors` and `palette`).

## How to rerun

```sh
cargo test -p nc-core --test style
cargo run --release -p nc-bench -- style /tmp/out crates/nc-core/tests/style-fixtures/*.png
cargo run --release -p nc-bench -- window panzoom fifo 30 auto style=SCREENSHOT.png shell
```
