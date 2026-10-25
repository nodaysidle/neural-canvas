# Phase 1 report: core document, commands, persistence

Date: 2026-10-25. Same machine as Phase 0 (Intel Arc iGPU, Mesa 26.2.5, Hyprland 0.56.2,
2560×1440 @ 120 Hz). Rust 1.98.1. Database benchmarks ran on btrfs.

## Done-criteria

| Criterion | Status | Evidence |
|---|---|---|
| Guard tests for Mac bugs #1–7, 11, 12, 18 pass headless | Pass | Table below; all run in `cargo test --workspace` |
| kill -9 harness: a stroke drawn 1 s before the kill survives | Pass | `nc-store/tests/guards.rs::stroke_survives_kill_9` |
| proptest: any random command sequence, undone fully, returns the original document | Pass | `nc-core/src/command.rs::undo_all_restores_original` (256 cases per run, plus redo-all returns the final document) |
| `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` | Pass | 34 tests, 0 failures. Clippy is also clean with `--features profile` |

### Guard tests

| # | Bug | Test |
|---|---|---|
| 1 | Eraser edits never saved | `erase_is_saved`: draw → flush → erase → flush → reopen → gone |
| 2 | Undo leaks across sketches | `undo_does_not_cross_sketches`: draw in A, switch to B, undo does nothing, A still has its stroke |
| 3 | Empty/corrupt data shows the previous sketch | `new_sketch_is_empty_and_corrupt_sketch_is_an_error`: a new sketch after a 50-stroke one is empty; a damaged row returns `Corrupt` naming the sketch and element, and other sketches still open |
| 4 | Silent in-memory fallback | `unwritable_location_fails_to_open`: `Db::open` and `Store::open` both fail in a read-only directory (the test ran; it skips only under root). The app exits with a message before any window opens; checked with `XDG_DATA_HOME=/proc/nope`, exit code 1 |
| 5 | `try?` saves, 60 s autosave | `stroke_survives_kill_9` and `debounced_save_lands_quickly_and_coalesces`: 20 edits in a burst produce one write, landing 300 to 500 ms after the first edit |
| 6 | Undo off by one | `undo_redo_is_exact`: draw A, B; undo ×2 empty; redo ×2 is A, B in order |
| 7 | Ctrl+Z not wired | `linux_undo_redo_shortcuts`, `undo_enabled_follows_history`: Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y map to actions; enabled state and "Undo Erase" labels follow the history |
| 11 | Wireframe and SVG flipped | `draws_stroke_offscreen` (y-down on the GPU). The SVG/PDF/PNG part arrives with export in Phase 4 |
| 12 | Horizontal pan inverted | `pan_right_moves_content_right` |
| 18 | Fake Zoom to Fit | `fit_uses_content_bounds`, `fit_of_a_dot_is_clamped` |

Also covered:
- **Mac bug #16 (whole-document writes):** `append_writes_one_row`. Adding one stroke to a 100-stroke sketch changes exactly one element row plus the sketch's revision.
- **Proptests for the viewport:** world↔screen round trip, and the zoom anchor holding at any scale (`.claude/rules/core.md`).

## What shipped

**`nc-core`**
- `Element { id: Uuid, kind: Stroke | Shape | Recognized }`.
- `Document` holds `Arc<Element>` in z-order. Fields are private, so the only way to change a document
  is `Command::apply`. It keeps a `revision` (every edit) and a `generation` (bumped on anything but
  an append; the renderer rebuilds on it).
- `Command` has `Append`, `Insert`, `Remove`, `Replace` and an all-or-nothing `Batch`. Applying a
  command returns its exact inverse.
- `History` keeps labelled undo/redo steps. `Session` owns one sketch's document and history; switching
  sketches drops it.
- `Action` / `Shortcut` provide the keyboard map, enabled state and labels.
- `Viewport::fit` frames content bounds with padding, with zoom clamped to 0.1–32×.
- The S2k generator now also derives element ids from its seed, so the whole document is reproducible.

**`nc-store`** (new crate)
- `Db`: bundled SQLite in WAL mode (open fails if WAL is refused), `synchronous = FULL`, foreign keys.
  Schema version lives in `PRAGMA user_version` with an append-only migration list, and a database
  from a newer version is refused (`TooNew`).
- Tables: `projects`, `sketches`, `elements` (one row per element, postcard-encoded `ElementKind`).
- Saves diff against what is on disk by `Arc` pointer: only new or changed rows are written, a z-order
  change is an `UPDATE z`, and deletions are row deletes. Everything happens in one transaction.
- `Store`: the store thread. A save is written at most 300 ms after the first unsaved change (measured
  from the first change, so continuous drawing still saves every 300 ms), and immediately on `flush`.
  A failed write stays pending, is retried after 1 s, and is reported to the UI as
  `StoreEvent::SaveFailed`.

**`neuralcanvas` (app)**
- Opens `$XDG_DATA_HOME/neuralcanvas/neuralcanvas.db` and the most recently edited sketch (a fresh
  install gets "My Project" / "Untitled Sketch").
- Strokes are drawn live and committed as one "Draw" command on release.
- Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y undo and redo. Ctrl+0 zooms to fit content, Ctrl+1 goes to 100 %.
- Every edit is queued to the store thread, so the UI thread never touches disk.
- Close and SIGTERM/SIGINT/SIGHUP commit a stroke still under the pen, flush, then exit. Checked live:
  SIGTERM exits 0, the database is in WAL mode at schema version 1, and the `-wal`/`-shm` files are gone
  after the clean close.

**`nc-render`**
- The stroke being drawn has its own instance buffer, rewritten each frame from a reused scratch `Vec`.
  The offscreen test now checks committed and live ink.

**`nc-bench`**
- New `store` subcommand for save/load cost.
- The draw scenario now commits through `Command`, the same path as the app.

## Numbers

### Save and load (`nc-bench store ~/.cache`, btrfs)

| Measurement | Result | Budget |
|---|---|---|
| Full save of S2k (2,000 rows, first write) | 18.08 ms | ≤ 200 ms (stretch: none) |
| Incremental save, one 100-point stroke on S2k, 50 runs | p50 0.92, p99 1.24 ms | ≤ 30 ms (stretch < 10 ms) |
| Open + load S2k (2,050 elements) | 5.25 ms | Sketch switch ≤ 100 ms to first frame (load is part of it) |

### Frame cost before and after this phase

Phase 1 replaced `Document::segments()`, which the renderer walks every frame. The scaffold boxed one
iterator per stroke. My first version boxed two per element, which roughly doubled CPU time (measured:
draw/Fifo CPU p50 2.93 ms, p99 5.15 ms, 11 late frames). The final version allocates nothing. Same runs
as Phase 0, 20 s fullscreen, Fifo:

| Run | Phase 0 CPU p50 / p99 | Phase 1 CPU p50 / p99 | Phase 1 discarded / > 8.33 ms |
|---|---|---|---|
| winit pan/zoom S2k | 1.70 / 3.30 ms | 0.35 / 0.88 ms | 0 / 0 |
| winit draw on S2k | 1.51 / 3.02 ms | 0.36 / 0.94 ms | 0 / 0 |

Headless `nc-bench offscreen 120`, S2k all visible: CPU sync + encode went from p50 1.34 to 0.07 ms.
The main-thread overruns Phase 0 reported (3.30 and 3.02 ms p99) are gone.

GPU time also dropped in the headless sweep, though nothing on the GPU side changed:

| Strokes, all visible | Phase 0 GPU p99 | Phase 1 GPU p99 |
|---|---|---|
| 2,000 | 1.44 ms | 1.40 ms |
| 5,000 | 4.04 ms | 2.49 ms |
| 10,000 | 9.14 ms | 5.22 ms |

My guess is that the iGPU clocks higher when it no longer shares its power budget with a busy CPU. That
is not verified. It does affect the Phase 0 tile-cache threshold (400k visible segments was set where
5k strokes crossed 4 ms, and with these numbers the crossing sits between 5k and 10k). I have **not**
changed the approved threshold. Phase 2 builds the tile cache and should re-measure before fixing the
number.

## Skipped, and why

- **Visible banner and dialog:** store errors currently show in the window title ("NeuralCanvas (not
  saved: …)", "(read-only: …)"), and an unopenable store exits with a message on stderr. Both have
  `shortcut:` comments in the code. The real banner, toast with Retry, and blocking dialog need the
  Phase 5 chrome.
- **Read-only sketches open empty:** a sketch with a corrupt row opens read-only and empty, not
  partially. Showing the readable rows needs a "partial" load path, which needs that UI to explain it.
- **Sketch switching in the app:** the store and `Session` support it, and bug #2 is tested through the
  store. There is no UI to switch until the Phase 5 sidebar.
- **Main thread waits on the store at startup and quit:** loading the first sketch and the final flush
  block the UI thread, before the first frame and after the last. Async switching comes with the
  sidebar.
- **Remaining 7 `ShapeType` cases:** the Swift source is not on this machine. Only the 8 cases the port
  plan names exist. New cases are appended, never inserted, so stored blobs stay valid.
- **Pressure per point:** `Stroke` stores positions only. Pressure arrives in Phase 2 with a schema
  migration.
- **Scripted drawing in the real app:** there is no input synthesis tool here (no `ydotool`). The app was
  launched, closed and signalled on Hyprland, but drawing was exercised only through tests and the
  bench.

## Risks

1. **Tile-cache threshold evidence moved** (see above). Re-measure in Phase 2 before building to 400k.
2. **Startup blocks on disk.** Load is 5.25 ms for S2k, inside the 300 ms cold-start budget, but a huge
   sketch would delay the first frame. Async open belongs with the sidebar work.
3. **History is unbounded.** Steps share `Arc` elements, so memory grows with edits, not with document
   size. A cap is cheap to add if RSS with S2k approaches the 200 MB budget.
4. **The workspace's minimum Rust is now 1.89** (rusqlite 0.41 needs it; wgpu 30 already needed 1.87).
