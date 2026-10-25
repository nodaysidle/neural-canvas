//! The SQLite side: schema, migrations, and incremental writes (one row per element). Used only by the
//! store thread and by tests; never by the UI thread.

use nc_core::{Document, Element, ElementId, ElementKind};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// One schema step. Code steps rewrite element blobs, since postcard is not self-describing.
enum Migration {
    Sql(&'static str),
    Code(fn(&rusqlite::Transaction<'_>) -> Result<()>),
}

/// `MIGRATIONS[i]` takes the schema from version `i` to `i + 1`; the schema version is
/// `PRAGMA user_version`. Never edit a shipped entry: append a new one.
const MIGRATIONS: &[Migration] = &[
    Migration::Sql(
        "
    CREATE TABLE projects (
        id         BLOB PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sketches (
        id         BLOB PRIMARY KEY,
        project_id BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        revision   INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE elements (
        sketch_id BLOB NOT NULL REFERENCES sketches(id) ON DELETE CASCADE,
        id        BLOB NOT NULL,
        z         INTEGER NOT NULL,
        data      BLOB NOT NULL, -- postcard-encoded nc_core::ElementKind
        PRIMARY KEY (sketch_id, id)
    ) WITHOUT ROWID;
    CREATE INDEX elements_by_z ON elements (sketch_id, z);
",
    ),
    Migration::Code(add_stroke_pressure),
    Migration::Code(add_recognized_label),
];

/// Element encodings as they were stored by older schema versions, frozen. When `ElementKind`'s
/// encoding changes again, copy the current types here as the next version first.
mod format {
    pub mod v1 {
        use super::v2::RecognizedShape;
        use glam::Vec2;
        use nc_core::Shape;
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub enum ElementKind {
            Stroke(Stroke),
            Shape(Shape),
            Recognized(RecognizedShape),
        }

        #[derive(Serialize, Deserialize)]
        pub struct Stroke {
            pub points: Vec<Vec2>,
            pub width: f32,
            pub color: [f32; 4],
        }
    }

    /// v2: strokes with pressure; recognised shapes without a label. `Shape`, `ShapeKind` and
    /// `ShapeType` are shared with the current types: they have not changed and only ever append.
    pub mod v2 {
        use glam::Vec2;
        use nc_core::{ElementId, Shape, ShapeType};
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub enum ElementKind {
            Stroke(Stroke),
            Shape(Shape),
            Recognized(RecognizedShape),
        }

        #[derive(Serialize, Deserialize)]
        pub struct Stroke {
            pub points: Vec<Vec2>,
            pub width: f32,
            pub color: [f32; 4],
            pub pressure: Vec<f32>,
        }

        #[derive(Serialize, Deserialize)]
        pub struct RecognizedShape {
            pub shape_type: ShapeType,
            pub min: Vec2,
            pub max: Vec2,
            pub confidence: f32,
            pub sources: Vec<ElementId>,
        }
    }
}

/// Rewrites every element blob through `upgrade`, decoding the old encoding as `Old`.
fn rewrite_elements<Old: serde::de::DeserializeOwned, New: serde::Serialize>(
    tx: &rusqlite::Transaction<'_>,
    upgrade: impl Fn(Old) -> New,
) -> Result<()> {
    let rows: Vec<(Uuid, Uuid, Vec<u8>)> = tx
        .prepare("SELECT sketch_id, id, data FROM elements")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut update =
        tx.prepare("UPDATE elements SET data = ?3 WHERE sketch_id = ?1 AND id = ?2")?;
    for (sketch, id, data) in rows {
        let old: Old = postcard::from_bytes(&data).map_err(|e| StoreError::Corrupt {
            sketch,
            element: id,
            reason: e.to_string(),
        })?;
        let data = postcard::to_stdvec(&upgrade(old)).expect("element kinds always encode");
        update.execute(params![sketch, id, data])?;
    }
    Ok(())
}

/// v2 → v3: recognised shapes gained a label (empty for every one stored before).
fn add_recognized_label(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    use format::v2;
    rewrite_elements(tx, |old: v2::ElementKind| match old {
        v2::ElementKind::Stroke(s) => ElementKind::Stroke(nc_core::Stroke {
            points: s.points,
            width: s.width,
            color: s.color,
            pressure: s.pressure,
        }),
        v2::ElementKind::Shape(s) => ElementKind::Shape(s),
        v2::ElementKind::Recognized(r) => ElementKind::Recognized(nc_core::RecognizedShape {
            shape_type: r.shape_type,
            min: r.min,
            max: r.max,
            confidence: r.confidence,
            sources: r.sources,
            label: String::new(),
        }),
    })
}

/// v1 → v2: strokes gained a per-point pressure track (empty for every stroke stored before).
fn add_stroke_pressure(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    use format::{v1, v2};
    rewrite_elements(tx, |old: v1::ElementKind| match old {
        v1::ElementKind::Stroke(s) => v2::ElementKind::Stroke(v2::Stroke {
            points: s.points,
            width: s.width,
            color: s.color,
            pressure: Vec::new(),
        }),
        v1::ElementKind::Shape(s) => v2::ElementKind::Shape(s),
        v1::ElementKind::Recognized(r) => v2::ElementKind::Recognized(r),
    })
}

pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInfo {
    pub id: Uuid,
    pub name: String,
    pub sketches: Vec<SketchInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SketchInfo {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    /// The database refused write-ahead logging (e.g. on a network filesystem).
    NotWal(String),
    /// Written by a newer NeuralCanvas; opening it could lose data.
    TooNew {
        found: u32,
        supported: u32,
    },
    UnknownSketch(Uuid),
    /// An element row could not be decoded. The sketch must open read-only.
    Corrupt {
        sketch: Uuid,
        element: ElementId,
        reason: String,
    },
    /// The store thread is gone.
    Closed,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Sqlite(e) => write!(f, "{e}"),
            Self::NotWal(mode) => write!(
                f,
                "the database could not use write-ahead logging (journal mode {mode})"
            ),
            Self::TooNew { found, supported } => write!(
                f,
                "the database was written by a newer NeuralCanvas (format {found}, this version reads up to {supported})"
            ),
            Self::UnknownSketch(id) => write!(f, "sketch {id} does not exist"),
            Self::Corrupt {
                sketch,
                element,
                reason,
            } => write!(
                f,
                "sketch {sketch} has an unreadable element {element}: {reason}"
            ),
            Self::Closed => write!(f, "the store has shut down"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Db {
    conn: Connection,
    /// What is on disk per sketch, as of the last load or save: element id → (z, element). Elements are
    /// immutable behind `Arc`, so a pointer match means the row is current.
    saved: HashMap<Uuid, HashMap<ElementId, (usize, Arc<Element>)>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Db {
    /// Opens (or creates) the database. There is no fallback: any failure is returned (Mac bug #4).
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::NotWal(mode));
        }
        // FULL: a commit is on disk before it is acknowledged; with the 300 ms debounce the fsync is cheap.
        conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")?;
        migrate(&mut conn)?;
        Ok(Self {
            conn,
            saved: HashMap::new(),
        })
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn create_project(&mut self, name: &str) -> Result<Uuid> {
        let id = Uuid::new_v4();
        self.conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, now_ms()],
        )?;
        Ok(id)
    }

    /// Every project with its sketches, for the sidebar: projects oldest first, sketches most
    /// recently edited first.
    pub fn projects(&mut self) -> Result<Vec<ProjectInfo>> {
        let mut projects: Vec<ProjectInfo> = self
            .conn
            .prepare("SELECT id, name FROM projects ORDER BY created_at, name")?
            .query_map([], |r| {
                Ok(ProjectInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    sketches: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, name FROM sketches ORDER BY updated_at DESC, created_at DESC",
        )?;
        let sketches = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Uuid>(1)?,
                SketchInfo {
                    id: r.get(0)?,
                    name: r.get(2)?,
                },
            ))
        })?;
        for row in sketches {
            let (project, sketch) = row?;
            if let Some(p) = projects.iter_mut().find(|p| p.id == project) {
                p.sketches.push(sketch);
            }
        }
        Ok(projects)
    }

    pub fn rename_project(&mut self, id: Uuid, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn rename_sketch(&mut self, id: Uuid, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sketches SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn create_sketch(&mut self, project: Uuid, name: &str) -> Result<Uuid> {
        let id = Uuid::new_v4();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO sketches (id, project_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, project, name, now],
        )?;
        self.saved.insert(id, HashMap::new());
        Ok(id)
    }

    /// The most recently edited sketch, creating a first project and sketch on a fresh database.
    pub fn last_or_new_sketch(&mut self) -> Result<Uuid> {
        let last = self
            .conn
            .query_row(
                "SELECT id FROM sketches ORDER BY updated_at DESC, created_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match last {
            Some(id) => Ok(id),
            None => {
                let project = self.create_project("My Project")?;
                self.create_sketch(project, "Untitled Sketch")
            }
        }
    }

    /// Loads a sketch. An empty sketch is an empty document; an unreadable row is an error, never a
    /// partial or stale document (Mac bug #3).
    pub fn load(&mut self, sketch: Uuid) -> Result<Document> {
        let elements = self.rows(sketch, true)?;
        let saved = elements
            .iter()
            .enumerate()
            .map(|(z, e)| (e.id, (z, e.clone())))
            .collect();
        self.saved.insert(sketch, saved);
        Ok(Document::from_elements(elements))
    }

    /// What can still be read of a damaged sketch, for showing it read-only (Mac bug #3): rows that
    /// don't decode are left out. Nothing is recorded as saved, so this is never a base for saving.
    pub fn load_readable(&mut self, sketch: Uuid) -> Result<Document> {
        Ok(Document::from_elements(self.rows(sketch, false)?))
    }

    /// The sketch's elements in z order. `strict`: an undecodable row is an error, else skipped.
    fn rows(&mut self, sketch: Uuid, strict: bool) -> Result<Vec<Arc<Element>>> {
        let exists = self
            .conn
            .query_row("SELECT 1 FROM sketches WHERE id = ?1", [sketch], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            return Err(StoreError::UnknownSketch(sketch));
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, data FROM elements WHERE sketch_id = ?1 ORDER BY z")?;
        let rows = stmt.query_map([sketch], |r| {
            Ok((r.get::<_, Uuid>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut elements = Vec::new();
        for row in rows {
            let (id, data) = row?;
            match postcard::from_bytes::<ElementKind>(&data) {
                Ok(kind) => elements.push(Arc::new(Element { id, kind })),
                Err(e) if strict => {
                    return Err(StoreError::Corrupt {
                        sketch,
                        element: id,
                        reason: e.to_string(),
                    });
                }
                Err(_) => {}
            }
        }
        Ok(elements)
    }

    /// Writes `elements` as the sketch's content in one transaction, touching only rows that changed.
    pub fn save(&mut self, sketch: Uuid, revision: u64, elements: &[Arc<Element>]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let updated = tx.execute(
            "UPDATE sketches SET revision = ?2, updated_at = ?3 WHERE id = ?1",
            params![sketch, revision as i64, now_ms()],
        )?;
        if updated == 0 {
            return Err(StoreError::UnknownSketch(sketch));
        }
        let previous = match self.saved.get(&sketch) {
            Some(saved) => saved,
            None => {
                // Never loaded by this connection: rewrite the sketch from scratch.
                tx.execute("DELETE FROM elements WHERE sketch_id = ?1", [sketch])?;
                &HashMap::new()
            }
        };
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO elements (sketch_id, id, z, data) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (sketch_id, id) DO UPDATE SET z = excluded.z, data = excluded.data",
            )?;
            let mut move_z =
                tx.prepare_cached("UPDATE elements SET z = ?3 WHERE sketch_id = ?1 AND id = ?2")?;
            for (z, element) in elements.iter().enumerate() {
                match previous.get(&element.id) {
                    Some((old_z, old)) if Arc::ptr_eq(old, element) => {
                        if *old_z != z {
                            move_z.execute(params![sketch, element.id, z as i64])?;
                        }
                    }
                    _ => {
                        let data = postcard::to_stdvec(&element.kind)
                            .expect("element kinds always encode");
                        upsert.execute(params![sketch, element.id, z as i64, data])?;
                    }
                }
            }
            let mut delete =
                tx.prepare_cached("DELETE FROM elements WHERE sketch_id = ?1 AND id = ?2")?;
            let current: HashSet<ElementId> = elements.iter().map(|e| e.id).collect();
            for id in previous.keys().filter(|id| !current.contains(id)) {
                delete.execute(params![sketch, id])?;
            }
        }
        tx.commit()?;
        let saved = elements
            .iter()
            .enumerate()
            .map(|(z, e)| (e.id, (z, e.clone())))
            .collect();
        self.saved.insert(sketch, saved);
        Ok(())
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let found: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::TooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for step in &MIGRATIONS[found as usize..] {
        match step {
            Migration::Sql(sql) => tx.execute_batch(sql)?,
            Migration::Code(f) => f(&tx)?,
        }
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use nc_core::{Command, Stroke};

    fn stroke(x: f32) -> Element {
        let mut s = Stroke::new(Vec2::new(x, 0.0), 2.0, [1.0; 4]);
        s.push(Vec2::new(x, 5.0));
        Element::stroke(s)
    }

    /// Mac bug #16 (whole-document writes per stroke): appending one stroke to 100 writes one element row.
    #[test]
    fn append_writes_one_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(&dir.path().join("nc.db")).unwrap();
        let sketch = db.last_or_new_sketch().unwrap();
        let mut doc = db.load(sketch).unwrap();
        for i in 0..100 {
            Command::append(stroke(i as f32)).apply(&mut doc).unwrap();
        }
        db.save(sketch, doc.revision(), doc.elements()).unwrap();
        Command::append(stroke(-1.0)).apply(&mut doc).unwrap();
        let before = db.conn.total_changes();
        db.save(sketch, doc.revision(), doc.elements()).unwrap();
        // One INSERT plus the sketch row's revision bump.
        assert_eq!(db.conn.total_changes() - before, 2);
    }

    #[test]
    fn migrates_fresh_database_and_refuses_newer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nc.db");
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        db.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(db);
        assert!(matches!(
            Db::open(&path),
            Err(StoreError::TooNew { found, supported }) if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));
    }

    /// A database written by schema v1 (strokes without pressure) opens and loads unchanged.
    #[test]
    fn migrates_v1_strokes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nc.db");
        let (sketch, id) = (Uuid::new_v4(), Uuid::new_v4());
        {
            let conn = Connection::open(&path).unwrap();
            let Migration::Sql(v1) = &MIGRATIONS[0] else {
                unreachable!()
            };
            conn.execute_batch(v1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            let project = Uuid::new_v4();
            conn.execute("INSERT INTO projects VALUES (?1, 'P', 0)", [project])
                .unwrap();
            conn.execute(
                "INSERT INTO sketches (id, project_id, name, created_at, updated_at) VALUES (?1, ?2, 'S', 0, 0)",
                [sketch, project],
            )
            .unwrap();
            let blob = postcard::to_stdvec(&format::v1::ElementKind::Stroke(format::v1::Stroke {
                points: vec![Vec2::ZERO, Vec2::new(3.0, 4.0)],
                width: 2.5,
                color: [0.1, 0.2, 0.3, 1.0],
            }))
            .unwrap();
            conn.execute(
                "INSERT INTO elements VALUES (?1, ?2, 0, ?3)",
                params![sketch, id, blob],
            )
            .unwrap();
        }
        let mut db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let doc = db.load(sketch).unwrap();
        let ElementKind::Stroke(s) = &doc.elements()[0].kind else {
            panic!("expected a stroke");
        };
        assert_eq!(doc.elements()[0].id, id);
        assert_eq!(s.points, [Vec2::ZERO, Vec2::new(3.0, 4.0)]);
        assert_eq!((s.width, s.color), (2.5, [0.1, 0.2, 0.3, 1.0]));
        assert!(s.pressure.is_empty());
    }

    /// A v2 database (recognised shapes without a label) opens with every label empty.
    #[test]
    fn migrates_v2_recognized_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nc.db");
        let (sketch, id, source) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        {
            let mut db = Db::open(&path).unwrap();
            let project = db.create_project("P").unwrap();
            db.conn
                .execute(
                    "INSERT INTO sketches (id, project_id, name, created_at, updated_at) VALUES (?1, ?2, 'S', 0, 0)",
                    params![sketch, project],
                )
                .unwrap();
            let blob = postcard::to_stdvec(&format::v2::ElementKind::Recognized(
                format::v2::RecognizedShape {
                    shape_type: nc_core::ShapeType::Button,
                    min: Vec2::ZERO,
                    max: Vec2::new(120.0, 40.0),
                    confidence: 0.8,
                    sources: vec![source],
                },
            ))
            .unwrap();
            db.conn
                .execute(
                    "INSERT INTO elements VALUES (?1, ?2, 0, ?3)",
                    params![sketch, id, blob],
                )
                .unwrap();
            db.conn.pragma_update(None, "user_version", 2).unwrap();
        }
        let mut db = Db::open(&path).unwrap();
        let doc = db.load(sketch).unwrap();
        let ElementKind::Recognized(r) = &doc.elements()[0].kind else {
            panic!("expected a recognised shape");
        };
        assert_eq!(
            (r.shape_type, r.sources.as_slice()),
            (nc_core::ShapeType::Button, &[source][..])
        );
        assert_eq!(r.label, "");
    }

    #[test]
    fn projects_list_sketches_newest_first_and_rename() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(&dir.path().join("nc.db")).unwrap();
        let first = db.last_or_new_sketch().unwrap();
        let project = db.projects().unwrap()[0].id;
        let second = db.create_sketch(project, "Second").unwrap();
        let other = db.create_project("Other").unwrap();
        db.rename_sketch(first, "Login screen").unwrap();
        db.rename_project(other, "Settings flow").unwrap();
        // Saving makes a sketch the most recently edited (timestamps are ms: step past the creates).
        std::thread::sleep(std::time::Duration::from_millis(5));
        db.save(first, 1, &[]).unwrap();
        let projects = db.projects().unwrap();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["My Project", "Settings flow"]);
        let sketches: Vec<_> = projects[0]
            .sketches
            .iter()
            .map(|s| (s.id, s.name.as_str()))
            .collect();
        assert_eq!(sketches, [(first, "Login screen"), (second, "Second")]);
        assert!(projects[1].sketches.is_empty());
    }

    #[test]
    fn reopen_keeps_data_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nc.db");
        let mut db = Db::open(&path).unwrap();
        let sketch = db.last_or_new_sketch().unwrap();
        let mut doc = db.load(sketch).unwrap();
        let (a, b) = (stroke(1.0), stroke(2.0));
        let ids = [b.id, a.id];
        Command::append(a).apply(&mut doc).unwrap();
        Command::Insert {
            index: 0,
            element: Arc::new(b),
        }
        .apply(&mut doc)
        .unwrap();
        db.save(sketch, doc.revision(), doc.elements()).unwrap();
        drop(db);
        let mut db = Db::open(&path).unwrap();
        assert_eq!(db.last_or_new_sketch().unwrap(), sketch);
        let loaded = db.load(sketch).unwrap();
        assert_eq!(loaded.elements(), doc.elements());
        assert_eq!(
            loaded.elements().iter().map(|e| e.id).collect::<Vec<_>>(),
            ids
        );
    }
}
