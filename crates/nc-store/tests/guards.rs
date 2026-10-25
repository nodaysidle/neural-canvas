//! Guard tests for the Mac app's persistence bugs (docs/PLAN.md §4), against the real store thread.

use glam::Vec2;
use nc_core::{Command, Element, Session, Stroke};
use nc_store::{DEBOUNCE, Db, Store, StoreError, StoreEvent};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

fn stroke(x: f32) -> Element {
    let mut s = Stroke::new(Vec2::new(x, 0.0), 2.0, [1.0; 4]);
    s.push(Vec2::new(x, 10.0));
    Element::stroke(s)
}

fn quiet(_: StoreEvent) {}

fn save(store: &Store, session: &Session) {
    let doc = session.doc();
    store.save(session.sketch_id, doc.revision(), doc.elements().to_vec());
}

fn open_session(store: &Store, sketch: Uuid) -> Session {
    Session::new(sketch, store.call(move |db| db.load(sketch)).unwrap())
}

/// Mac bug #1: erase → flush → reopen → gone.
#[test]
fn erase_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nc.db");
    let store = Store::open(path.clone(), quiet).unwrap();
    let sketch = store.call(|db| db.last_or_new_sketch()).unwrap();
    let mut session = open_session(&store, sketch);
    let a = stroke(1.0);
    let id = a.id;
    session.execute("Draw", Command::append(a)).unwrap();
    save(&store, &session);
    store.flush().unwrap();
    session.execute("Erase", Command::Remove { id }).unwrap();
    save(&store, &session);
    store.flush().unwrap();
    drop(store);

    let mut db = Db::open(&path).unwrap();
    assert!(db.load(sketch).unwrap().elements().is_empty());
}

/// Mac bug #2: draw in A, switch to B, undo → both unchanged.
#[test]
fn undo_does_not_cross_sketches() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("nc.db"), quiet).unwrap();
    let (a_id, b_id) = store
        .call(|db| {
            let project = db.create_project("P")?;
            Ok((
                db.create_sketch(project, "A")?,
                db.create_sketch(project, "B")?,
            ))
        })
        .unwrap();
    let mut a = open_session(&store, a_id);
    a.execute("Draw", Command::append(stroke(1.0))).unwrap();
    save(&store, &a);

    // Switch: flush, drop A's session (and its history), open B.
    store.flush().unwrap();
    drop(a);
    let mut b = open_session(&store, b_id);
    assert!(!b.undo().unwrap(), "B has no history to undo");
    assert!(b.doc().elements().is_empty());
    save(&store, &b);
    store.flush().unwrap();

    assert_eq!(open_session(&store, a_id).doc().elements().len(), 1);
    assert!(open_session(&store, b_id).doc().elements().is_empty());
}

/// Mac bug #3: a new sketch after a full one is empty; an unreadable sketch is an error, not
/// someone else's content.
#[test]
fn new_sketch_is_empty_and_corrupt_sketch_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nc.db");
    let mut db = Db::open(&path).unwrap();
    let full = db.last_or_new_sketch().unwrap();
    let mut doc = db.load(full).unwrap();
    for i in 0..50 {
        Command::append(stroke(i as f32)).apply(&mut doc).unwrap();
    }
    db.save(full, doc.revision(), doc.elements()).unwrap();
    let project: Uuid = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT project_id FROM sketches WHERE id = ?1",
            [full],
            |r| r.get(0),
        )
        .unwrap();
    let fresh = db.create_sketch(project, "New").unwrap();
    assert!(db.load(fresh).unwrap().elements().is_empty());

    let victim = doc.elements()[7].id;
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE elements SET data = x'ff' WHERE sketch_id = ?1 AND id = ?2",
            rusqlite::params![full, victim],
        )
        .unwrap();
    let mut db = Db::open(&path).unwrap();
    match db.load(full) {
        Err(StoreError::Corrupt {
            sketch, element, ..
        }) => assert_eq!((sketch, element), (full, victim)),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    assert!(
        db.load(fresh).unwrap().elements().is_empty(),
        "other sketches still open"
    );
}

/// Mac bug #4: an unwritable location is an error at open. There is no in-memory fallback.
#[test]
fn unwritable_location_fails_to_open() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::write(locked.join("probe"), b"").is_ok() {
        eprintln!("running with permission overrides (root?); skipping");
        return;
    }
    assert!(Db::open(&locked.join("nc.db")).is_err());
    assert!(Store::open(locked.join("sub").join("nc.db"), quiet).is_err());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Mac bug #5: a save lands within the debounce window, and a burst of edits is one write.
#[test]
fn debounced_save_lands_quickly_and_coalesces() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let store = Store::open(dir.path().join("nc.db"), move |e| {
        sink.lock().unwrap().push((Instant::now(), e))
    })
    .unwrap();
    let sketch = store.call(|db| db.last_or_new_sketch()).unwrap();
    let mut session = open_session(&store, sketch);
    let start = Instant::now();
    for i in 0..20 {
        session
            .execute("Draw", Command::append(stroke(i as f32)))
            .unwrap();
        save(&store, &session);
    }
    while events.lock().unwrap().is_empty() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(DEBOUNCE); // would catch a second, redundant write
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1, "20 edits, one write: {events:?}");
    let (at, event) = &events[0];
    assert_eq!(
        *event,
        StoreEvent::Saved {
            sketch,
            revision: session.doc().revision()
        }
    );
    let waited = at.duration_since(start);
    assert!(
        waited >= DEBOUNCE && waited < DEBOUNCE + Duration::from_millis(200),
        "written after {waited:?}"
    );
}

const CHILD_DB: &str = "NC_KILL9_CHILD_DB";

/// Mac bug #5: kill -9 one second after a stroke is drawn → the stroke is there on reopen.
#[test]
fn stroke_survives_kill_9() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nc.db");
    let mut child = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", "kill_9_child", "--nocapture", "--test-threads=1"])
        .env(CHILD_DB, &path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // libtest prints "test kill_9_child ... " before the child's own output, on the same line.
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let drawn = BufReader::new(stdout)
            .lines()
            .map_while(|l| l.ok())
            .find_map(|l| l.split_once("drawn ").map(|(_, rest)| rest.to_owned()));
        let _ = tx.send(drawn);
    });
    let Ok(Some(drawn)) = rx.recv_timeout(Duration::from_secs(30)) else {
        child.kill().unwrap();
        panic!("child never reported its stroke");
    };
    let (sketch, element) = drawn.split_once(' ').unwrap();
    std::thread::sleep(Duration::from_secs(1));
    child.kill().unwrap(); // SIGKILL: no destructors, no flush
    child.wait().unwrap();

    let mut db = Db::open(&path).unwrap();
    let doc = db.load(sketch.parse().unwrap()).unwrap();
    let ids: Vec<String> = doc.elements().iter().map(|e| e.id.to_string()).collect();
    assert_eq!(ids, [element]);
}

/// The process `stroke_survives_kill_9` kills. Does nothing in a normal test run.
#[test]
fn kill_9_child() {
    let Some(path) = std::env::var_os(CHILD_DB) else {
        return;
    };
    let store = Store::open(Path::new(&path).to_owned(), quiet).unwrap();
    let sketch = store.call(|db| db.last_or_new_sketch()).unwrap();
    let mut session = open_session(&store, sketch);
    let element = stroke(3.0);
    let id = element.id;
    session.execute("Draw", Command::append(element)).unwrap();
    save(&store, &session);
    println!("drawn {sketch} {id}");
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
