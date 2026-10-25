//! Style mirror check: extracts a style from real app screenshots and writes, for each, the screenshot
//! side by side with what was read off it (roles, palette, WCAG checks, radius, spacing, and a small
//! mockup drawn in the style), for a human to judge.

use nc_core::style::{Extracted, Note, Pair, Style, extract};
use std::fmt::Write;
use std::path::Path;
use std::time::Instant;

const PANEL: u32 = 560;

pub fn run(out: &Path, shots: &[String]) {
    std::fs::create_dir_all(out).expect("output folder");
    for path in shots {
        let shot = image::open(path).expect("screenshot").to_rgba8();
        let t = Instant::now();
        let e = extract(shot.as_raw(), shot.width(), shot.height()).expect("non-empty screenshot");
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let name = Path::new(path).file_stem().unwrap().to_string_lossy();
        report(&name, &e, shot.width(), shot.height(), ms);

        let height = shot.height().max(780);
        let panel = nc_export::png(&panel_svg(&e, height), 1.0).expect("panel");
        let panel = image::load_from_memory(&panel)
            .expect("panel png")
            .to_rgba8();
        let mut side = image::RgbaImage::from_pixel(
            shot.width() + PANEL,
            height,
            image::Rgba([255, 255, 255, 255]),
        );
        image::imageops::overlay(&mut side, &shot, 0, 0);
        image::imageops::overlay(&mut side, &panel, shot.width() as i64, 0);
        let file = out.join(format!("{name}-style.png"));
        side.save(&file).expect("write comparison");
        println!("  -> {}", file.display());
    }
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

fn report(name: &str, e: &Extracted, w: u32, h: u32, ms: f64) {
    let s = &e.style;
    println!(
        "{name} ({w}x{h}, {ms:.0} ms): background {} surface {} accent {} text {} | radius {} px, spacing {} px, line height {:?}",
        hex(s.background),
        hex(s.surface),
        hex(s.accent),
        hex(s.text),
        s.radius,
        s.spacing,
        s.line_height
    );
    let palette: Vec<String> = e
        .palette
        .iter()
        .map(|(c, share)| format!("{} {:.2}%", hex(*c), share * 100.0))
        .collect();
    println!("  palette: {}", palette.join(", "));
    println!(
        "  radii: {:?}  gaps: {:?}  notes: {:?}",
        e.radii, e.gaps, e.notes
    );
    for p in Pair::ALL {
        let c = s.check(p);
        println!(
            "  {:?}: {:.2}:1 {}",
            p,
            c.ratio,
            if c.passes() { "pass" } else { "FAIL" }
        );
    }
}

fn pair_name(p: Pair) -> &'static str {
    match p {
        Pair::TextOnBackground => "Text on background",
        Pair::TextOnSurface => "Text on surface",
        Pair::LabelOnAccent => "Label on accent",
        Pair::AccentOnBackground => "Accent on background",
        Pair::AccentOnSurface => "Accent on surface",
    }
}

fn note_text(n: Note) -> &'static str {
    match n {
        Note::NoAccent => "No accent colour found",
        Note::NoSurface => "No surface colour found",
        Note::NoBoxes => "No boxes found to measure",
        Note::FewGaps => "Too few gaps to read a rhythm",
        Note::NoText => "No text found",
    }
}

/// The right-hand panel as SVG, `height` px tall.
fn panel_svg(e: &Extracted, height: u32) -> String {
    let s: &Style = &e.style;
    let mut o = String::new();
    let _ = writeln!(
        o,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{PANEL}" height="{height}" viewBox="0 0 {PANEL} {height}"><rect width="{PANEL}" height="{height}" fill="#ffffff"/><rect width="1" height="{height}" fill="#c9ccd3"/>"##
    );
    let text = |o: &mut String, x: u32, y: u32, size: u32, weight: u32, fill: &str, t: &str| {
        let _ = writeln!(
            o,
            r#"<text x="{x}" y="{y}" font-family="Manrope" font-size="{size}" font-weight="{weight}" fill="{fill}">{t}</text>"#
        );
    };
    text(&mut o, 32, 44, 20, 600, "#16181e", "Extracted style");
    let mut y = 72;
    for (role, c) in [
        ("Background", s.background),
        ("Surface", s.surface),
        ("Accent", s.accent),
        ("Text", s.text),
    ] {
        let _ = writeln!(
            o,
            r##"<rect x="32" y="{y}" width="48" height="40" rx="6" fill="{}" stroke="#9aa0ab"/>"##,
            hex(c)
        );
        text(&mut o, 96, y + 18, 15, 600, "#16181e", role);
        text(&mut o, 96, y + 36, 14, 400, "#575d6b", &hex(c));
        y += 52;
    }
    // Every cluster, as wide as its share.
    text(&mut o, 32, y + 16, 13, 600, "#575d6b", "Palette by share");
    y += 26;
    let mut x = 32.0;
    for (c, share) in &e.palette {
        let w = share * (PANEL as f32 - 64.0);
        let _ = writeln!(
            o,
            r#"<rect x="{x:.1}" y="{y}" width="{w:.1}" height="20" fill="{}"/>"#,
            hex(*c)
        );
        x += w;
    }
    let _ = writeln!(
        o,
        r##"<rect x="32" y="{y}" width="{}" height="20" fill="none" stroke="#9aa0ab"/>"##,
        PANEL - 64
    );
    y += 48;
    text(&mut o, 32, y, 13, 600, "#575d6b", "WCAG 2.1 AA");
    y += 22;
    for p in Pair::ALL {
        let c = s.check(p);
        let (mark, fill) = if c.passes() {
            ("pass", "#1d7a3a")
        } else {
            ("fails", "#9a5a00")
        };
        text(&mut o, 32, y, 14, 400, "#16181e", pair_name(p));
        text(
            &mut o,
            300,
            y,
            14,
            400,
            "#16181e",
            &format!("{:.2}:1 (needs {})", c.ratio, p.minimum()),
        );
        text(&mut o, 480, y, 14, 600, fill, mark);
        y += 22;
    }
    y += 14;
    let list = |v: &[(u32, u32)]| {
        v.iter()
            .take(3)
            .map(|(v, n)| format!("{v} px ×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let measured = |v: &[(u32, u32)], what: &str| {
        if v.is_empty() {
            String::new()
        } else {
            format!(" ({what}{})", list(v))
        }
    };
    for line in [
        format!("Corner radius {} px{}", s.radius, measured(&e.radii, "")),
        format!(
            "Spacing unit {} px{}",
            s.spacing,
            measured(&e.gaps, "gaps ")
        ),
        match s.line_height {
            Some(l) => format!("Line height {l} px"),
            None => "Line height not measured".into(),
        },
    ] {
        text(&mut o, 32, y, 14, 400, "#16181e", &line);
        y += 22;
    }
    for n in &e.notes {
        text(&mut o, 32, y, 14, 400, "#9a5a00", note_text(*n));
        y += 22;
    }
    // A small screen drawn in the style: the page, a card, two lines of text, a field and a button.
    y += 12;
    let (r, sp) = (s.radius, s.spacing);
    let (bg, sf, ac, tx, on) = (
        hex(s.background),
        hex(s.surface),
        hex(s.accent),
        hex(s.text),
        hex(s.on_accent()),
    );
    let (x0, w, h) = (32.0, PANEL as f32 - 64.0, 220.0);
    let y0 = y as f32;
    let pad = 2.0 * sp;
    let _ = writeln!(
        o,
        r##"<rect x="{x0}" y="{y0}" width="{w}" height="{h}" fill="{bg}" stroke="#9aa0ab"/>
<rect x="{}" y="{}" width="{}" height="{}" rx="{r}" fill="{sf}"/>
<text x="{}" y="{}" font-family="Manrope" font-size="16" font-weight="600" fill="{tx}">Card title</text>
<text x="{}" y="{}" font-family="Manrope" font-size="13" fill="{tx}">Body text in the text colour.</text>
<rect x="{}" y="{}" width="{}" height="36" rx="{r}" fill="{bg}" stroke="{tx}" stroke-opacity="0.35"/>
<rect x="{}" y="{}" width="120" height="36" rx="{r}" fill="{ac}"/>
<text x="{}" y="{}" font-family="Manrope" font-size="14" font-weight="600" fill="{on}" text-anchor="middle">Button</text>"##,
        x0 + pad,
        y0 + pad,
        w - 2.0 * pad,
        h - 2.0 * pad,
        x0 + 2.0 * pad,
        y0 + 2.0 * pad + 18.0,
        x0 + 2.0 * pad,
        y0 + 2.0 * pad + 18.0 + s.line_height.unwrap_or(20.0),
        x0 + 2.0 * pad,
        y0 + h - 2.0 * pad - 36.0,
        w - 6.0 * pad - 120.0,
        x0 + w - 2.0 * pad - 120.0,
        y0 + h - 2.0 * pad - 36.0,
        x0 + w - 2.0 * pad - 60.0,
        y0 + h - 2.0 * pad - 13.0,
    );
    o.push_str("</svg>\n");
    o
}
