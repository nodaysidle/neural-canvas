//! Save and load cost for S2k against a real database file (docs/PERFORMANCE-BUDGET.md "Save").

use glam::Vec2;
use nc_bench::stats::Summary;
use nc_core::{Command, Element, Stroke, scene};
use nc_store::Db;
use std::path::Path;
use std::time::Instant;

/// `keep`: write `dir/neuralcanvas.db` and leave it, so the app can open S2k
/// (`XDG_DATA_HOME=<parent of dir> neuralcanvas`).
pub fn run(dir: &Path, keep: bool) {
    let path = if keep {
        dir.join("neuralcanvas.db")
    } else {
        dir.join(format!("nc-bench-{}.db", std::process::id()))
    };
    let mut db = Db::open(&path).expect("open database");
    let sketch = db.last_or_new_sketch().expect("create sketch");
    let mut doc = db.load(sketch).expect("load");
    for element in scene::s2k(1).elements() {
        Command::Append {
            element: element.clone(),
        }
        .apply(&mut doc)
        .expect("append");
    }
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    db.save(sketch, doc.revision(), doc.elements())
        .expect("full save");
    println!("full save, S2k (2,000 rows, first write): {:.2} ms", ms(t));

    let mut incremental = Vec::new();
    for i in 0..50 {
        let mut stroke = Stroke::new(Vec2::new(i as f32, 0.0), 2.5, [1.0; 4]);
        for j in 1..100 {
            stroke.push(Vec2::new(i as f32, j as f32));
        }
        Command::append(Element::stroke(stroke))
            .apply(&mut doc)
            .expect("append");
        let t = Instant::now();
        db.save(sketch, doc.revision(), doc.elements())
            .expect("save");
        incremental.push(ms(t));
    }
    println!(
        "incremental save, one 100-point stroke on S2k: {}",
        Summary::of(&incremental).unwrap()
    );

    drop(db);
    let t = Instant::now();
    let mut db = Db::open(&path).expect("reopen");
    let loaded = db.load(sketch).expect("load");
    println!(
        "open + load S2k ({} elements): {:.2} ms",
        loaded.elements().len(),
        ms(t)
    );
    drop(db);
    if keep {
        println!("kept {}", path.display());
        return;
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
