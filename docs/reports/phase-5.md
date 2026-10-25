# Phase 5 report: shell and polish

Date: 2026-10-25. Reference machine: Intel Arc iGPU (Meteor Lake), Mesa 26.2.5, 2560×1440 @ 120 Hz,
Hyprland 0.56.2. Screenshots and the accessibility dump are in [`phase-5/`](phase-5/).

## Read first

1. **The shell is our own wgpu layer on raw winit**, as decided in Phase 0. iced 0.14 still pins
   wgpu 27, so the §1 re-check did not run. Text is glyphon 0.12 (cosmic-text) on wgpu 30. The fonts
   are Manrope and DM Mono, bundled, both SIL OFL.
2. **The direction is in `DESIGN.md`:**
   - dark first, with one accent; amber and red mark state only;
   - a 4 px grid, panel radius 8 and control radius 6;
   - no shadows (a 1 px border instead) and text controls.

   It is a transcription of the Phase 5 brief and UX-PRINCIPLES, not a new style.
3. **Two Phase 1 shortcuts are closed.** An unopenable store used to exit with a line on stderr. It now
   gets the blocking Retry / Choose location / Quit dialog (bug #4). A damaged sketch used to open
   empty. It now opens read-only with every row that can still be read (bug #3).
4. **A new dependency: `ashpd`**, for the XDG file chooser portal behind "Choose location…". It runs
   on zbus with async-io, which the tree already had through mundy and AccessKit. Phase 6 needs the
   same portal for screenshot import.
5. **The stripped binary grew from 14.2 MB to 21.1 MB.** The budget is 25 MB. The new dependencies
   this phase are text shaping, the fonts, AccessKit with its AT-SPI stack, and the portal client;
   I did not break the growth down by crate. Phase 6 adds image decoding, so the binary budget needs
   watching.

## Done-criteria

| Criterion | Status | Evidence |
|---|---|---|
| Each of the 10 UX principles checked with a screenshot or recording on Hyprland | Pass, with the gaps listed under each principle | Checklist below, `phase-5/` |
| Tiled and floating, at scale 1.0 and 1.5 | Pass | `18-hyprland-tiled-floating.png`: DP-1 at 1.0, plus a headless output at 1.5 |
| No internal strings in UI text | Pass | Every string lives in `nc-shell/src/copy.rs`; `copy_has_no_internal_or_debug_text` audits it, including action labels, tooltips and dialog copy |
| Frame budget holds with the shell visible | Pass | S2k with sidebar, inspector and palette drawn every frame (table below) |
| fmt, clippy `-D warnings`, tests | Pass | 137 tests, 0 failures; clippy clean with and without `--features profile` |

## UX principles

**1. The canvas is the app.**
- Full-bleed canvas with a 40 px top bar and a floating palette (`01-empty-canvas.png`).
- The palette steps back to 30 % opacity while a stroke or drag is in progress
  (`14-palette-fade-while-drawing.png`: mid-stroke on the left, after on the right).
- Sidebar (`Ctrl+B`) and inspector (`Ctrl+I`) collapse. Their state is saved to `settings.json` and
  restored on launch (`08-panels-restored-after-relaunch.png`, taken after a quit and relaunch).

**2. Ink never lags.** No change to the ink path this phase. The live stroke, the 1€ smoothing
(still switchable in Preferences) and predicted ink are as measured in Phases 2 and 3. Draw with the
shell visible is in the frame table below.

**3. Motion follows the hand.** Pan, pinch and cursor-anchored zoom are unchanged from Phase 2.
- New: touchpad pan coasts after the fingers lift. Velocity decays with a 0.25 s time constant and
  stops below 20 px/s; it is touchpad only, and a mouse never coasts.
- Fit animates for 180 ms and is skipped when the portal asks for reduced motion.
- **Not verified:** inertia was not tried on a physical touchpad (the reference machine was driven
  through the virtual pointer, which has no touchpad phases).

**4. Keyboard-first, Linux conventions.**
- The new shortcuts work: `Ctrl+2` zoom to selection, `Ctrl+=`/`Ctrl+-`, `Ctrl+B`, `Ctrl+I`, `Ctrl+,`
  and `?`.
- The shortcuts sheet lists every binding (`03-shortcuts.png`).
- Tab and Shift+Tab move focus through every control, and Enter or Space activates it. In a dialog,
  focus stays inside and wraps.
- Every tooltip names its key (`13-tooltip.png`). Palette tools show their key inline instead.

**5. Fit Hyprland.**
- Fractional scales were checked on a headless output (`hyprctl output create headless`). At 1.0,
  1.25 and 1.5, text and 1 px borders stay crisp (`17-fractional-scales.png`). Window logical sizes
  were 1510×813 at 1.25 and 1254×669 at 1.5.
- Tiled and floating both lay out correctly (`18-…`).
- Light and dark follow the portal's `color-scheme`. "Match system" is the default, with a manual
  override in Preferences (`05-light-theme.png`).
- In light mode, ink lightness is inverted with hue kept. The wireframe and chrome use their own
  light colours instead of being inverted.

**6. Never lose work, never block.**
- Saving is invisible; the top bar says Saved, Saving… or Not saved.
- A save failure is a red banner with Retry.
- A damaged sketch shows what could be read, under an amber read-only banner
  (`09-read-only-banner.png`).
- An unopenable store gets the only blocking dialog, with Retry, Choose location… and Quit
  (`10-store-dialog.png`). It can't be dismissed, and keys and clicks don't reach the canvas behind it.
- The three dialog paths were checked in the real app:
  - **Retry:** the folder was made writable, then Retry opened the store.
  - **Choose location…:** opens the portal (`16-portal-folder-chooser-title.png`). The chosen folder
    opened (`11-opened-in-chosen-folder.png`) and is remembered in `settings.json`.
  - **Quit:** exits.
- Export shows "Exporting…" until the result toast replaces it ("Exported … as SVG, PDF, PNG and
  JPEG", with Open folder; `07-export-toast.png`). Raster exports are capped at 8,192 px on the long
  side, so a huge sketch at 2× can't need gigabytes.
- **Gaps:**
  - Recognition and export can't be cancelled. Both run on idle-priority workers and never block the
    UI.
  - A 20-stroke export finished before the next frame, so the "Exporting…" toast is covered by
    `progress_toasts_stay_and_cannot_be_dismissed` rather than by a screenshot.

**7. Honest output.** Uncertain wireframe items are amber, and the top bar offers "Review N
uncertain items" (`U`). The inspector shows confidence and marks uncertain items
(`12-wire-edit.png`).

**8. Direct manipulation.**
- Selection and handles are as in Phase 3.
- New: the inspector edits position, size and stroke width of strokes and shapes. It also edits a
  wireframe element's type, from a menu of the 15 types (`06-inspector-type-menu.png`), and its
  label, which is drawn on the canvas and in exports (`12-label.png`).
- An edited element is pinned: recognition keeps the user's type and label.
- **Gap:** the custom eraser cursor is still drawn at scale 1, so it is soft at 1.5 (winit 0.30 has
  no buffer scale for custom cursors).

**9. Accessible.**
- Every control has a label and a role. The AT-SPI tree, read with Atspi over the session bus, is in
  `19-atspi-tree.txt`. It holds:
  - the canvas, labelled "Sketch: 20 strokes, 0 shapes, 0 recognised elements";
  - toggle buttons with their pressed state;
  - text fields with their value;
  - the selected sketch.
- Chrome text contrast is at least 4.5:1 in both themes (`chrome_meets_wcag_aa_in_both_themes`).
- Keyboard focus draws an accent ring (`15-keyboard-focus.png`).
- Two defects found by reading that tree are fixed: the shortcuts button was announced as "?" and the
  save status as an empty label.

**10. Strings are product copy.**
- All UI text is in `copy.rs`, and the audit test rejects internal names, debug wording and TODO
  phrasing.
- No debug panel or flag exists in the binary; profiling stays behind the non-default `profile`
  feature.
- Exports are now named after the sketch ("Login flow 1.svg", "Login flow wireframe 1.svg") instead of
  "Sketch 1". Path separators in a name become "-".

## Numbers

S2k with the shell drawn every frame (sidebar with a sample project, inspector, palette, top bar).
Each run is `nc-bench window <scenario> fifo 30 auto shell`, 30 s at 120 Hz:

| Scenario | Frames/s | Intervals > 8.33 ms | > 16.6 ms | Main-thread CPU p99 |
|---|---|---|---|---|
| Pan/zoom | 119.9 | 0.08 % | 0 | 1.46 ms |
| Draw | 119.8 | 0.11 % | 0 | 1.19 ms |
| Move 500 strokes | 119.3 | 0.53 % | 0 | 1.60 ms |

All three meet "0 over 16.6 ms, < 1 % over 8.3 ms", with main-thread CPU p99 under the 3 ms budget.

| Measure | Result | Budget |
|---|---|---|
| Cold start to first presented frame (`--features profile`, 5 runs) | 111.5–117.4 ms empty store, 113.1–119.2 ms with a 20-stroke sketch | ≤ 300 ms warm |
| Idle, sidebar and inspector open | 0 CPU ticks over 5 s | ~0 % |
| RSS idle, sidebar and inspector open (release) | 68.2 MB | ≤ 80 MB |
| Stripped binary | 21.1 MB | ≤ 25 MB |

Cold start rose by about 15 ms from 97–100 ms (Phase 2). The new startup work is font loading, the
text atlas and the AccessKit adapter; I did not profile which of them costs what.

## What shipped

- **`nc-shell` (new crate):** pure layout, no GPU and no window.
  - It turns app state into a `UiScene` (rects, images, text in three layers) plus `Region`s.
  - The regions drive hit-testing, Tab order and the accessibility tree.
  - It holds the palette, the WCAG check, all copy, the toasts, the shortcuts and preferences
    dialogs, and the store dialog. 16 tests.
- **`nc-render`:**
  - A UI pass with SDF rounded rects, thumbnails and glyphon text, with text buffers cached across
    frames.
  - Light and dark canvas themes, and a grid toggle.
- **`nc-store`:**
  - Schema v3 adds wireframe labels; its migration is tested.
  - New calls: the project and sketch list, renames, and `Store::request` (non-blocking, results
    come back as events).
  - `load_readable` reads a damaged sketch for read-only display.
- **`nc-core`:** pinned wireframe edits override recognition; new actions; the ink-on-light
  transform.
- **`nc-export`:** wireframe labels as SVG `<text>`, rendered with the bundled Manrope; the light
  theme; the raster cap.
- **`nc-app`:** split into `app/{mod,shell,library,access,settings}.rs`.
  - Sidebar with live thumbnails (re-rendered at most once a second while editing).
  - Inspector editing; toasts and banners; the store dialog with portal folder choice.
  - Settings written atomically on a worker.
  - The AccessKit tree; touchpad inertia; exports named after the sketch.
- **`nc-bench`:** `shell` argument for the frame runs above.

## Bugs found and fixed this phase

- The layout used 1280×800 for the first frame: the GPU was taken out before the window size was read.
- Thumbnails were zoomed in, because `Viewport::fit` clamps at 0.1×. Thumbnails now use their own
  unclamped fit.
- Light theme turned the wireframe and chrome navy (inverted). They now carry no-inversion flags.
- The variable Manrope font was named "ExtraLight", so text fell back to mono. It is now cut into
  static Regular and SemiBold instances (`nc-render/fonts/README.md`).
- Tab focus could reach controls behind a dialog's scrim. Focus is now trapped in the dialog.
- The AT-SPI names of the "?" button and the save status (above).

## Skipped, and why

- **Cancelling recognition or export.** Both finish in well under a second on S2k (Phase 4: 55 ms
  recognition) and never block the UI. A cancel control would be noise at those times.
- **Disabled states.** No chrome control is ever unavailable, so there is no disabled state to
  show. Undo and redo live in the keyboard and shortcuts sheet, not in the bar.
- **Touchpad inertia on hardware**, as noted under principle 3.
- **A screen-reader walkthrough with Orca.** The tree was read programmatically; no one has listened
  to it.

## Risks

- **Binary size headroom is 3.9 MB** before the 25 MB budget, and Phase 6 adds image decoding.
- **AccessKit on AT-SPI** is only exported while session accessibility is on. That is the platform's
  rule, so the tree was checked by switching it on for the run and back off afterwards.
- **Portal dependence:** "Choose location…" needs a FileChooser portal backend (gtk here). Without
  one, the dialog shows the portal's error in place of the store's, and Retry and Quit still work.
