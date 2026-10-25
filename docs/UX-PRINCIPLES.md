# UX principles

UI/UX and performance are the product. A feature that is janky, laggy, or unclear is not done.
Each principle below is checked with a screenshot or recording before Phase 5 closes.

1. **The canvas is the app.** Full-bleed by default. Chrome is a slim top bar and a floating tool palette that
   fades while drawing. Sidebar (`Ctrl+B`) and inspector (`Ctrl+I`) collapse and remember their state.
2. **Ink never lags.** The live stroke appears in the next frame from raw samples. Smoothing (one-euro) never
   adds visible tail lag; refit once on pen-up. No sample is dropped.
3. **Motion follows the hand.** The point under the cursor stays under the cursor during pan and zoom.
   Touchpad two-finger scroll pans; pinch zooms around the centroid; wheel / `Ctrl`+scroll zooms at the
   cursor with `exp(k·delta)` so speed is device-independent. Pan has gentle inertia on touchpads only.
   Animated zoom-to-fit ≈ 180 ms ease-out; no animation when reduced motion is requested.
4. **Keyboard-first, Linux conventions.** `Ctrl`, not Cmd. Single-key tools: `P` pen, `E` eraser, `V` select,
   `H` hand, `R` rect, `O` ellipse, `L` line. Hold `Space` for a temporary hand that restores the previous
   tool. `Ctrl+Z` / `Ctrl+Shift+Z` / `Ctrl+Y`, `Ctrl+0` fit, `Ctrl+1` 100 %, `Ctrl+2` zoom to selection,
   `Ctrl+±` zoom, `Ctrl+?` shortcuts overlay. Every tooltip shows its shortcut.
5. **Fit Hyprland.** Works tiled and floating, any size ≥ 640×480, fractional scales (1.25/1.5/1.75) with
   no blur. Stable app id `sh.nodaysidle.NeuralCanvas` for window rules. Dark-first, follows the portal
   `color-scheme` for light/dark.
6. **Never lose work, never block.** Saving is invisible. Errors are toasts with an action ("Retry"); only an
   unopenable store gets a dialog. Long work (recognition, style, export) shows inline progress and can be
   cancelled. The main thread never waits on disk or workers.
7. **Honest output.** Low-confidence recognition is visibly marked, with a "Review 3 uncertain items" action.
8. **Direct manipulation.** Select, move, arrow-nudge (Shift = 10 px), `Delete`, `Ctrl+D`, `Ctrl+A`, `Esc`.
   Handles are 8 px on screen at every zoom. Per-tool cursors (crosshair, grab/grabbing, eraser ring sized to
   the eraser).
9. **Accessible.** Every control has a label and a keyboard path. Chrome text contrast ≥ 4.5:1. Focus is
   always visible. The canvas exposes a summary ("Sketch: 12 strokes, 5 recognised elements").
10. **Strings are product copy.** No internal names, debug text, or TODO phrasing in the UI. No debug panels or
    flags in release builds.

## Visual polish rules

- One accent colour; neutrals carry the rest. Grid is quiet: major lines readable, minor lines fade with zoom.
- 4 px spacing grid in chrome; consistent 8 px radius on floating panels; one shadow style.
- Motion: 120–200 ms, ease-out, never blocks input. Nothing animates while the user is drawing.
- Empty states teach: the first canvas shows one line of hint ("Draw with P, pan with Space") that disappears
  on first stroke.
- Every interactive state exists: hover, pressed, focused, disabled, selected.
