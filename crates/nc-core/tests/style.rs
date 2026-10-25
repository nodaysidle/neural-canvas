//! Style mirror on five real app screenshots (Files, Pinta, Xournal++, Papers and NeuralCanvas itself,
//! captured on Hyprland): the roles a person reading the screenshot would name. Side-by-side images
//! for each are in `docs/reports/phase-6/`; known misses are listed there and not asserted here.

use nc_core::style::{Extracted, Note, extract};

fn read(name: &str) -> Extracted {
    let path = format!(
        "{}/tests/style-fixtures/{name}.png",
        env!("CARGO_MANIFEST_DIR")
    );
    let image = image::open(&path).expect("fixture").to_rgba8();
    extract(image.as_raw(), image.width(), image.height()).expect("non-empty")
}

#[test]
fn files_light_libadwaita() {
    let e = read("files");
    let s = e.style;
    assert_eq!(
        [s.background, s.surface, s.accent, s.text],
        [0xebebed, 0xffffff, 0x2686e6, 0x2f2f34],
        "{e:?}"
    );
    assert!(
        e.notes.contains(&Note::NoBoxes),
        "a file list has no cards to measure"
    );
}

#[test]
fn pinta_dark_gtk4() {
    let e = read("pinta");
    let s = e.style;
    assert_eq!((s.background, s.text), (0x222226, 0xffffff), "{e:?}");
    assert_eq!(
        s.radius, 9.0,
        "rounded entries and dropdowns, not the square canvas: {e:?}"
    );
}

#[test]
fn xournalpp_gtk3() {
    let e = read("xournalpp");
    let s = e.style;
    assert_eq!((s.background, s.accent), (0x353535, 0x40a0ff), "{e:?}");
    assert!(
        e.notes.contains(&Note::NoSurface),
        "flat toolbars, no panels: {e:?}"
    );
}

#[test]
fn papers_document_viewer() {
    let e = read("papers");
    let s = e.style;
    assert_eq!(
        (s.background, s.surface, s.text),
        (0x222226, 0x2e2e32, 0xffffff),
        "{e:?}"
    );
    assert!(
        e.notes.contains(&Note::NoAccent),
        "nothing coloured on screen: {e:?}"
    );
    assert_eq!(s.line_height, Some(27.0), "the abstract's lines: {e:?}");
}

#[test]
fn neuralcanvas_own_chrome() {
    let e = read("neuralcanvas");
    let s = e.style;
    assert_eq!(
        [s.background, s.surface, s.accent, s.text],
        [0x1c1e24, 0x2a2c34, 0x5e9dff, 0xe7e9ee],
        "{e:?}"
    );
    assert_eq!(s.radius, 7.0, "panels and buttons are 6 to 8 px: {e:?}");
}
