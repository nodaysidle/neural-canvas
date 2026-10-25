//! SQLite persistence for sketches: WAL, one row per element, written by a dedicated store thread.
//! The database lives at `$XDG_DATA_HOME/neuralcanvas/neuralcanvas.db` (docs/PLAN.md §8).

mod db;
mod thread;

pub use db::{Db, Preset, ProjectInfo, Result, SCHEMA_VERSION, SketchInfo, StoreError};
pub use thread::{DEBOUNCE, RETRY, Store, StoreEvent};
