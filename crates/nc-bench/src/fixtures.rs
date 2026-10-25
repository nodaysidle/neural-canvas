//! Recognition fixtures, recorded through the real app: this module generates hand-like drawings of
//! UI elements as `drive script` commands (wobble, rounded or overshot corners, closure gaps, slight
//! rotation, uneven sample spacing), and extracts what the app saved back into labelled fixtures.
//! `record-fixtures.sh` runs the loop. Same seed, same drawings.
//!
//! Pages hold 6 drawings in a 3 × 2 grid of cells; cell coordinates are window-local logical px, which
//! are world units at the app's starting view (100 %, no pan).

use glam::Vec2;
use kurbo::{BezPath, ParamCurveArclen, PathSeg, Point};
use nc_core::ShapeType;
use nc_core::recognize::{Fixture, FixtureStroke};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};
use std::path::Path;

const CELL: Vec2 = Vec2::new(800.0, 640.0);
const CELL_GAP: Vec2 = Vec2::new(20.0, 20.0);
const MARGIN: Vec2 = Vec2::new(30.0, 30.0);
const COLS: usize = 3;
pub const PER_PAGE: usize = 6;

type Strokes = Vec<Vec<Vec2>>;

/// px per 4 ms sample: edges and lines, and handwriting and small loops.
const FAST: (f32, f32) = (2.5, 6.0);
const SLOW: (f32, f32) = (0.6, 1.5);

/// What each fixture draws, how many of it, and the type it should be recognised as.
const KINDS: [(&str, ShapeType, usize); 14] = [
    ("button", ShapeType::Button, 6),
    ("button-4-strokes", ShapeType::Button, 2),
    ("input", ShapeType::InputField, 5),
    ("card", ShapeType::Card, 5),
    ("container", ShapeType::Container, 4),
    ("divider", ShapeType::Divider, 4),
    ("text", ShapeType::Text, 6),
    ("icon", ShapeType::Icon, 4),
    ("image", ShapeType::Image, 4),
    ("circle", ShapeType::Circle, 4),
    ("ellipse", ShapeType::Ellipse, 4),
    ("line", ShapeType::Line, 4),
    ("arrow", ShapeType::Arrow, 4),
    ("rectangle", ShapeType::Rectangle, 4),
];

#[derive(Serialize, Deserialize)]
struct Manifest {
    cells: Vec<Cell>,
}

#[derive(Serialize, Deserialize)]
struct Cell {
    name: String,
    expected: ShapeType,
    /// Window-local logical px: x0, y0, x1, y1.
    rect: [f32; 4],
}

struct Hand {
    rng: ChaCha8Rng,
}

impl Hand {
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng.random_range(lo..hi)
    }

    fn chance(&mut self, p: f32) -> bool {
        self.range(0.0, 1.0) < p
    }

    /// Low-frequency wobble along arc length: a few sines per axis, `amp` px in total.
    fn wobble(&mut self, amp: f32) -> impl Fn(f32) -> Vec2 + use<> {
        let mut waves = [[0.0f32; 3]; 6];
        for w in &mut waves {
            *w = [
                self.range(0.2, 1.0) * amp / 3.0,
                TAU / self.range(40.0, 180.0),
                self.range(0.0, TAU),
            ];
        }
        move |s: f32| {
            let axis = |ws: &[[f32; 3]]| ws.iter().map(|[a, f, p]| a * (f * s + p).sin()).sum();
            Vec2::new(axis(&waves[..3]), axis(&waves[3..]))
        }
    }

    /// Samples `path` the way a hand-held pointer would at 250 Hz: uneven spacing, wobble, a little
    /// jitter. `speed` is the range of px per sample: long straight edges go at 600–1,500 px/s
    /// (2.5–6), handwriting and small loops at a quarter of that.
    fn trace(&mut self, path: &BezPath, speed: (f32, f32)) -> Vec<Vec2> {
        let amp = self.range(0.6, 2.2);
        let wobble = self.wobble(amp);
        let base_step = self.range(speed.0, speed.1);
        let mut out = Vec::new();
        // Arc length where the current segment starts, and where the next sample falls.
        let (mut seg_start, mut next) = (0.0f32, 0.0f32);
        for seg in path.segments() {
            let len = seg.arclen(0.1) as f32;
            while next <= seg_start + len {
                let p = seg_at(seg, (next - seg_start) / len.max(1e-3));
                let jitter = Vec2::new(self.range(-0.3, 0.3), self.range(-0.3, 0.3));
                out.push(p + wobble(next) + jitter);
                next += base_step * self.range(0.7, 1.3);
            }
            seg_start += len;
        }
        out
    }

    /// A polygon through `corners`, with corners rounded by up to `round` px; closed shapes start at
    /// the first corner and end with an overshoot or a gap of up to `slop` px.
    fn polygon(&mut self, corners: &[Vec2], closed: bool, round: f32, slop: f32) -> Vec<Vec2> {
        let mut pts: Vec<Vec2> = corners.to_vec();
        if closed {
            pts.push(corners[0]);
            // End past the start (overshoot) or short of it (gap).
            let along = (corners[1] - corners[0]).normalize_or_zero();
            let end = corners[0] + along * self.range(-slop, slop);
            pts.push(end);
        }
        let mut path = BezPath::new();
        path.move_to(pt(pts[0]));
        for i in 1..pts.len() {
            let is_corner = i + 1 < pts.len();
            if is_corner && round > 0.0 {
                let r = self.range(0.0, round);
                let (a, c, b) = (pts[i - 1], pts[i], pts[i + 1]);
                let into = c + (a - c).normalize_or_zero() * r.min(a.distance(c) * 0.4);
                let out = c + (b - c).normalize_or_zero() * r.min(b.distance(c) * 0.4);
                path.line_to(pt(into));
                path.quad_to(pt(c), pt(out));
            } else {
                path.line_to(pt(pts[i]));
            }
        }
        self.trace(&path, FAST)
    }

    fn rect(&mut self, w: f32, h: f32) -> Vec<Vec2> {
        let c = [
            Vec2::new(-w / 2.0, -h / 2.0),
            Vec2::new(w / 2.0, -h / 2.0),
            Vec2::new(w / 2.0, h / 2.0),
            Vec2::new(-w / 2.0, h / 2.0),
        ];
        self.polygon(&c, true, 10.0, 12.0)
    }

    /// A rectangle drawn as four separate edges, each running a little past its corners.
    fn rect_4(&mut self, w: f32, h: f32) -> Strokes {
        let c = [
            Vec2::new(-w / 2.0, -h / 2.0),
            Vec2::new(w / 2.0, -h / 2.0),
            Vec2::new(w / 2.0, h / 2.0),
            Vec2::new(-w / 2.0, h / 2.0),
        ];
        (0..4)
            .map(|i| {
                let (a, b) = (c[i], c[(i + 1) % 4]);
                let d = (b - a).normalize_or_zero();
                let (a, b) = (a - d * self.range(-4.0, 8.0), b + d * self.range(-4.0, 8.0));
                self.polygon(&[a, b], false, 0.0, 0.0)
            })
            .collect()
    }

    /// Big loops at full speed, icon-sized ones slower, as a hand slows on tight curves.
    fn ellipse(&mut self, rx: f32, ry: f32) -> Vec<Vec2> {
        let speed = if rx.min(ry) < 40.0 { SLOW } else { FAST };
        let start = self.range(0.0, TAU);
        let sweep = TAU + self.range(-0.25, 0.3);
        let n = ((rx + ry) * sweep / 6.0) as usize;
        let mut path = BezPath::new();
        for i in 0..=n {
            let a = start + sweep * i as f32 / n as f32;
            let p = Vec2::new(rx * a.cos(), ry * a.sin());
            if i == 0 {
                path.move_to(pt(p));
            } else {
                path.line_to(pt(p));
            }
        }
        self.trace(&path, speed)
    }

    /// A handwriting-like scribble: up-and-down strokes with letter-sized loops.
    fn scribble(&mut self, w: f32, h: f32) -> Vec<Vec2> {
        let letters = (w / self.range(9.0, 14.0)).max(3.0) as usize;
        let mut path = BezPath::new();
        path.move_to(pt(Vec2::new(-w / 2.0, 0.0)));
        for i in 0..letters {
            let x = -w / 2.0 + w * (i as f32 + 0.5) / letters as f32;
            let up = h / 2.0 * self.range(0.5, 1.0);
            let x2 = -w / 2.0 + w * (i as f32 + 1.0) / letters as f32;
            path.quad_to(
                pt(Vec2::new(x, -up * 2.0)),
                pt(Vec2::new(x2, h / 2.0 * 0.4)),
            );
        }
        self.trace(&path, SLOW)
    }

    fn line(&mut self, a: Vec2, b: Vec2) -> Vec<Vec2> {
        self.polygon(&[a, b], false, 0.0, 0.0)
    }

    fn draw(&mut self, name: &str) -> Strokes {
        match name {
            "button" | "button-4-strokes" => {
                let (w, h) = (self.range(100.0, 170.0), self.range(34.0, 52.0));
                let mut s = if name == "button" {
                    vec![self.rect(w, h)]
                } else {
                    self.rect_4(w, h)
                };
                if self.chance(0.6) {
                    let (sw, sh) = (w * self.range(0.4, 0.6), self.range(8.0, 12.0));
                    s.push(self.scribble(sw, sh));
                }
                s
            }
            "input" => {
                let (w, h) = (self.range(220.0, 320.0), self.range(34.0, 48.0));
                let mut s = vec![self.rect(w, h)];
                if self.chance(0.6) {
                    let pw = w * self.range(0.25, 0.4);
                    let ph = self.range(7.0, 10.0);
                    let mut placeholder = self.scribble(pw, ph);
                    let dx = -w / 2.0 + 12.0 + pw / 2.0;
                    placeholder.iter_mut().for_each(|p| p.x += dx);
                    s.push(placeholder);
                }
                s
            }
            "card" | "container" => {
                let (w, h) = if name == "card" {
                    (self.range(260.0, 360.0), self.range(200.0, 300.0))
                } else {
                    (self.range(560.0, 700.0), self.range(470.0, 560.0))
                };
                let mut s = vec![self.rect(w, h)];
                let items = if name == "card" {
                    self.rng.random_range(2..=3)
                } else {
                    self.rng.random_range(0..=3)
                };
                for i in 0..items {
                    let y = -h / 2.0 + 30.0 + i as f32 * (h - 50.0) / 3.0;
                    let iw = w * self.range(0.35, 0.7);
                    let mut item = if i == 0 {
                        let ih = self.range(10.0, 14.0);
                        self.scribble(iw, ih)
                    } else {
                        let x0 = -w / 2.0 + 24.0;
                        let dy = self.range(-2.0, 2.0);
                        self.line(Vec2::new(x0, 0.0), Vec2::new(x0 + iw, dy))
                    };
                    let dx = if i == 0 {
                        -w / 2.0 + 24.0 + iw / 2.0
                    } else {
                        0.0
                    };
                    item.iter_mut().for_each(|p| *p += Vec2::new(dx, y));
                    s.push(item);
                }
                s
            }
            "divider" => {
                let w = self.range(160.0, 320.0);
                let tilt = self.range(-0.02, 0.02) * w;
                vec![self.line(Vec2::new(-w / 2.0, -tilt), Vec2::new(w / 2.0, tilt))]
            }
            "text" => {
                let (w, h) = (self.range(100.0, 220.0), self.range(10.0, 16.0));
                vec![self.scribble(w, h)]
            }
            "icon" => {
                let r = self.range(14.0, 24.0);
                let ry = r * self.range(0.9, 1.1);
                vec![self.ellipse(r, ry)]
            }
            "image" => {
                let w = self.range(110.0, 170.0);
                let h = w * self.range(0.85, 1.2);
                let (a, b) = (Vec2::new(-w / 2.0, -h / 2.0), Vec2::new(w / 2.0, h / 2.0));
                let inset = |hand: &mut Self| Vec2::splat(hand.range(2.0, 8.0));
                let d1 = (a + inset(self), b - inset(self));
                let d2 = (
                    Vec2::new(b.x, a.y) + Vec2::new(-1.0, 1.0) * inset(self),
                    Vec2::new(a.x, b.y) + Vec2::new(1.0, -1.0) * inset(self),
                );
                vec![
                    self.rect(w, h),
                    self.line(d1.0, d1.1),
                    self.line(d2.0, d2.1),
                ]
            }
            "circle" => {
                let r = self.range(60.0, 100.0);
                let ry = r * self.range(0.93, 1.07);
                vec![self.ellipse(r, ry)]
            }
            "ellipse" => {
                let rx = self.range(100.0, 150.0);
                let ry = rx * self.range(0.4, 0.6);
                vec![self.ellipse(rx, ry)]
            }
            "line" => {
                let len = self.range(120.0, 250.0);
                let a = self.range(0.5, 1.4) * if self.chance(0.5) { 1.0 } else { -1.0 };
                let d = Vec2::from_angle(a) * len / 2.0;
                vec![self.line(-d, d)]
            }
            "arrow" => {
                let len = self.range(140.0, 260.0);
                let dir = Vec2::from_angle(self.range(-0.6, 0.6));
                let (tail, tip) = (-dir * len / 2.0, dir * len / 2.0);
                let head = self.range(18.0, 30.0);
                let spread = self.range(0.45, 0.65);
                let left = tip - Vec2::from_angle(spread).rotate(dir) * head;
                let right = tip - Vec2::from_angle(-spread).rotate(dir) * head;
                if self.chance(0.5) {
                    vec![self.polygon(&[tail, tip, left, tip, right], false, 0.0, 0.0)]
                } else {
                    vec![
                        self.line(tail, tip),
                        self.polygon(&[left, tip, right], false, 0.0, 0.0),
                    ]
                }
            }
            "rectangle" => {
                let (w, h) = (self.range(200.0, 300.0), self.range(130.0, 180.0));
                if self.chance(0.5) {
                    vec![self.rect(w, h)]
                } else {
                    self.rect_4(w, h)
                }
            }
            other => panic!("no generator for {other}"),
        }
    }
}

fn pt(v: Vec2) -> Point {
    Point::new(v.x as f64, v.y as f64)
}

fn seg_at(seg: PathSeg, t: f32) -> Vec2 {
    let p = kurbo::ParamCurve::eval(&seg, t.clamp(0.0, 1.0) as f64);
    Vec2::new(p.x as f32, p.y as f32)
}

/// Every fixture for `seed`: (name, expected type, strokes centred on the origin). Deterministic.
/// Seed 4 is the tuning set, seed 5 the held-out set nothing was tuned on.
fn all(seed: u64) -> Vec<(String, ShapeType, Strokes)> {
    let mut hand = Hand {
        rng: ChaCha8Rng::seed_from_u64(seed),
    };
    let mut out = Vec::new();
    for (name, expected, count) in KINDS {
        for i in 0..count {
            let mut strokes = hand.draw(name);
            // Nobody draws level: tilt the whole drawing a little.
            let tilt = Vec2::from_angle(hand.range(-3.0, 3.0) * PI / 180.0);
            strokes
                .iter_mut()
                .flatten()
                .for_each(|p| *p = tilt.rotate(*p));
            out.push((format!("{name}-{}", i + 1), expected, strokes));
        }
    }
    out
}

pub fn pages() -> usize {
    KINDS.iter().map(|k| k.2).sum::<usize>().div_ceil(PER_PAGE)
}

fn cell_rect(slot: usize) -> [f32; 4] {
    let at = MARGIN + Vec2::new((slot % COLS) as f32, (slot / COLS) as f32) * (CELL + CELL_GAP);
    [at.x, at.y, at.x + CELL.x, at.y + CELL.y]
}

/// Prints the drive script for `page`, for a window whose top-left is at `origin` (output px), and
/// writes the page's manifest.
pub fn script(seed: u64, page: usize, origin: Vec2, manifest: &Path) {
    let fixtures = all(seed);
    let mut cells = Vec::new();
    println!("key p");
    for (slot, (name, expected, strokes)) in fixtures
        .into_iter()
        .skip(page * PER_PAGE)
        .take(PER_PAGE)
        .enumerate()
    {
        let rect = cell_rect(slot);
        let centre = Vec2::new(rect[0] + rect[2], rect[1] + rect[3]) / 2.0 + origin;
        for stroke in &strokes {
            let at = |p: &Vec2| (*p + centre).round();
            let first = at(&stroke[0]);
            println!("move {} {} 120", first.x, first.y);
            println!("down");
            for p in &stroke[1..] {
                let q = at(p);
                println!("move {} {} 4", q.x, q.y);
            }
            println!("up");
            println!("wait 60");
        }
        cells.push(Cell {
            name,
            expected,
            rect,
        });
    }
    println!("wait 500");
    let json = serde_json::to_string_pretty(&Manifest { cells }).expect("manifest serializes");
    std::fs::write(manifest, json).expect("write manifest");
}

/// Reads the strokes the app saved for one page and writes one fixture per cell into `out`.
pub fn extract(manifest: &Path, db: &Path, out: &Path) {
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(manifest).expect("read manifest")).expect("manifest");
    let mut store = nc_store::Db::open(db).expect("open the page's database");
    let sketch = store.last_or_new_sketch().expect("sketch");
    let doc = store.load(sketch).expect("load sketch");
    std::fs::create_dir_all(out).expect("create fixture dir");
    for cell in manifest.cells {
        let [x0, y0, x1, y1] = cell.rect;
        let strokes: Vec<FixtureStroke> = doc
            .elements()
            .iter()
            .filter_map(|e| match &e.kind {
                nc_core::ElementKind::Stroke(s) => Some(s),
                _ => None,
            })
            .filter(|s| {
                s.bounds().is_some_and(|b| {
                    let c = b.center();
                    (x0 as f64..x1 as f64).contains(&c.x) && (y0 as f64..y1 as f64).contains(&c.y)
                })
            })
            .map(|s| FixtureStroke {
                width: s.width,
                points: s.points.iter().map(|p| [p.x - x0, p.y - y0]).collect(),
            })
            .collect();
        assert!(
            !strokes.is_empty(),
            "nothing was drawn in cell {}",
            cell.name
        );
        let fixture = Fixture {
            name: cell.name.clone(),
            expected: cell.expected,
            strokes,
        };
        let path = out.join(format!("{}.json", cell.name));
        std::fs::write(
            &path,
            serde_json::to_string(&fixture).expect("fixture serializes"),
        )
        .expect("write fixture");
        println!("{} ({} strokes)", path.display(), fixture.strokes.len());
    }
}
