//! Recognition accuracy on recorded fixtures, drawn through the app by
//! `crates/nc-bench/record-fixtures.sh`: `tests/fixtures` (seed 4, the set the recognizer was tuned
//! on) and `tests/fixtures-holdout` (seed 5, recorded after tuning and never tuned on). A fixture is
//! right when the largest recognised shape has the fixture's type. Run with `--nocapture` for the
//! per-type tables.

use nc_core::ShapeType;
use nc_core::recognize::{self, Fixture, primary, strokes, swift};
use std::collections::BTreeMap;

fn fixtures(set: &str) -> Vec<Fixture> {
    let dir = format!("{}/tests/{set}", env!("CARGO_MANIFEST_DIR"));
    let mut out: Vec<Fixture> = std::fs::read_dir(dir)
        .expect("fixture directory")
        .map(|e| {
            let path = e.expect("dir entry").path();
            serde_json::from_slice(&std::fs::read(&path).expect("read fixture"))
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Fraction right, printing a per-type table and every miss.
fn score(
    name: &str,
    fixtures: &[Fixture],
    recognize: impl Fn(&Fixture) -> Option<ShapeType>,
) -> f64 {
    let mut by_type: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut right = 0;
    println!("\n{name}");
    for f in fixtures {
        let got = recognize(f);
        let ok = got == Some(f.expected);
        right += ok as usize;
        let entry = by_type.entry(format!("{:?}", f.expected)).or_default();
        entry.0 += ok as usize;
        entry.1 += 1;
        if !ok {
            println!(
                "  miss {:<20} expected {:<11?} got {got:?}",
                f.name, f.expected
            );
        }
    }
    for (t, (ok, n)) in &by_type {
        println!("  {t:<12} {ok}/{n}");
    }
    let accuracy = right as f64 / fixtures.len() as f64;
    println!(
        "  total {right}/{} = {:.1} %",
        fixtures.len(),
        accuracy * 100.0
    );
    accuracy
}

fn check(set: &str) {
    let fixtures = fixtures(set);
    assert!(fixtures.len() >= 50, "{} fixtures", fixtures.len());
    let baseline = score(&format!("{set}: Swift baseline (port)"), &fixtures, |f| {
        let elements = f.elements();
        primary(&swift::recognize(&strokes(&elements))).map(|r| r.shape_type)
    });
    let ours = score(&format!("{set}: NeuralCanvas recognizer"), &fixtures, |f| {
        primary(&recognize::recognize(&f.elements()).shapes).map(|r| r.shape_type)
    });
    assert!(
        ours >= baseline,
        "{ours} below the Swift baseline {baseline}"
    );
    assert!(ours > 0.88, "accuracy {ours} is not above 88 %");
}

#[test]
fn accuracy_on_the_tuning_set() {
    check("fixtures");
}

#[test]
fn accuracy_on_the_held_out_set() {
    check("fixtures-holdout");
}
