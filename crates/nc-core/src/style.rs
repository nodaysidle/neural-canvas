//! Style mirror: a palette, corner radius, spacing rhythm and line height read off a screenshot of a
//! real app, checked against WCAG 2.1 AA, for the wireframe to be drawn in. Pure maths over RGBA8
//! pixels; decoding the file is the caller's (worker's) job.
//!
//! Colours are clustered with k-means in CIELAB, then each cluster is snapped to its most frequent
//! exact pixel, so a flat UI colour comes back as the hex it was painted with, not an average of it
//! and its anti-aliased edges. Geometry comes from the connected regions of each cluster: regions that
//! fill their bounding box are boxes (cards, buttons, fields), whose corners give the radius and whose
//! gaps and insets give the spacing.

use crate::theme::{contrast, hex};
use kmeans_colors::{Sort, get_kmeans_hamerly};
use palette::{IntoColor, Lab, Srgb};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What the wireframe is drawn in. Colours are sRGB hex (0xRRGGBB), as the screenshot held them.
/// Stored postcard-encoded (style presets, a sketch's style): append fields only with a migration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub background: u32,
    /// Cards and panels.
    pub surface: u32,
    /// Buttons and other primary controls.
    pub accent: u32,
    pub text: u32,
    /// Corner radius of cards, buttons and fields, px.
    pub radius: f32,
    /// Spacing base unit, px (4 or 8).
    pub spacing: f32,
    /// Distance between lines of body text, px.
    pub line_height: Option<f32>,
}

/// A WCAG 2.1 AA pair the style is checked on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pair {
    TextOnBackground,
    TextOnSurface,
    /// A button's label ([`Style::on_accent`]) on the accent.
    LabelOnAccent,
    AccentOnBackground,
    AccentOnSurface,
}

impl Pair {
    pub const ALL: [Pair; 5] = [
        Pair::TextOnBackground,
        Pair::TextOnSurface,
        Pair::LabelOnAccent,
        Pair::AccentOnBackground,
        Pair::AccentOnSurface,
    ];

    /// The AA minimum: 4.5:1 for text (1.4.3), 3:1 for a control against what it sits on (1.4.11).
    pub fn minimum(self) -> f32 {
        match self {
            Pair::TextOnBackground | Pair::TextOnSurface | Pair::LabelOnAccent => 4.5,
            Pair::AccentOnBackground | Pair::AccentOnSurface => 3.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Check {
    pub pair: Pair,
    pub ratio: f32,
}

impl Check {
    pub fn passes(&self) -> bool {
        self.ratio >= self.pair.minimum()
    }
}

impl Style {
    /// Text on an accent fill: the text, background or surface colour, whichever contrasts most.
    pub fn on_accent(&self) -> u32 {
        let accent = hex(self.accent);
        [self.text, self.background, self.surface]
            .into_iter()
            .max_by(|a, b| contrast(hex(*a), accent).total_cmp(&contrast(hex(*b), accent)))
            .unwrap_or(self.text)
    }

    pub fn check(&self, pair: Pair) -> Check {
        let (a, b) = match pair {
            Pair::TextOnBackground => (self.text, self.background),
            Pair::TextOnSurface => (self.text, self.surface),
            Pair::LabelOnAccent => (self.on_accent(), self.accent),
            Pair::AccentOnBackground => (self.accent, self.background),
            Pair::AccentOnSurface => (self.accent, self.surface),
        };
        Check {
            pair,
            ratio: contrast(hex(a), hex(b)),
        }
    }

    /// The pairs below their AA minimum.
    pub fn failing(&self) -> impl Iterator<Item = Check> + '_ {
        Pair::ALL
            .into_iter()
            .map(|p| self.check(p))
            .filter(|c| !c.passes())
    }
}

/// What the screenshot didn't show, so a value had to be assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// No saturated colour: the accent is the text colour.
    NoAccent,
    /// Nothing between background and text: the surface is the background, a step lighter or darker.
    NoSurface,
    /// No boxes found: radius 0 and an 8 px rhythm are assumed.
    NoBoxes,
    /// Too few gaps between boxes to read a rhythm: 8 px is assumed.
    FewGaps,
    /// No text found: the text colour is black or white, whichever reads on the background.
    NoText,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Extracted {
    pub style: Style,
    /// The screenshot's colours, largest share first, with their share of the image (0..=1).
    pub palette: Vec<(u32, f32)>,
    /// Corner radii found on boxes, most common first, with how many boxes had each (px).
    pub radii: Vec<(u32, u32)>,
    /// The most common gaps and insets between boxes, most common first (px).
    pub gaps: Vec<(u32, u32)>,
    pub notes: Vec<Note>,
}

/// Clusters k-means looks for. A UI shows a handful of flat colours plus anti-aliasing between them.
const K: usize = 16;
/// Pixels k-means sees, sampled evenly across the image.
const SAMPLE: usize = 120_000;
/// Clusters closer than this (CIEDE76) are one colour.
const SAME: f32 = 1.5;
/// Lab chroma from which a colour counts as an accent, not a neutral.
const CHROMATIC: f32 = 20.0;
/// A pixel this far (squared sRGB distance) from every palette colour belongs to none: an
/// anti-aliased edge or a photo.
const NEAR: u32 = 3 * 20 * 20;
const NONE: u8 = u8::MAX;

#[derive(Clone, Copy, Debug)]
struct Cluster {
    rgb: u32,
    lab: Lab,
    share: f32,
}

fn lab(rgb: u32) -> Lab {
    let [r, g, b] = [16, 8, 0].map(|s| ((rgb >> s) & 0xff) as f32 / 255.0);
    Srgb::new(r, g, b).into_color()
}

fn from_lab(lab: Lab) -> u32 {
    let c: Srgb = lab.into_color();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    byte(c.red) << 16 | byte(c.green) << 8 | byte(c.blue)
}

fn delta_e(a: Lab, b: Lab) -> f32 {
    ((a.l - b.l).powi(2) + (a.a - b.a).powi(2) + (a.b - b.b).powi(2)).sqrt()
}

fn chroma(c: Lab) -> f32 {
    c.a.hypot(c.b)
}

fn dist2(a: u32, b: u32) -> u32 {
    [16, 8, 0]
        .map(|s| ((a >> s) & 0xff).abs_diff((b >> s) & 0xff).pow(2))
        .iter()
        .sum()
}

/// Reads a style off an RGBA8 screenshot (`width * height * 4` bytes). `None` for an empty image or
/// a short buffer.
pub fn extract(rgba: &[u8], width: u32, height: u32) -> Option<Extracted> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return None;
    }
    // Transparent pixels are seen over white, as an image viewer would show them.
    let pixels: Vec<u32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .take(w * h)
        .map(|p| {
            let a = p[3] as u32;
            let c = |v: u8| (v as u32 * a + 255 * (255 - a)) / 255;
            c(p[0]) << 16 | c(p[1]) << 8 | c(p[2])
        })
        .collect();
    let clusters = palette_of(&pixels);
    let mut notes = Vec::new();

    let labels = label(&pixels, &clusters);
    let (region, regions) = regions(&labels, w, h);
    // The background is what the window's edges show, which is not always the colour with the
    // largest share: cards can cover more of the screen than the page around them.
    let page = edge_colour(&labels, w, h).unwrap_or(0);
    let background = clusters[page as usize];
    let others = || clusters.iter().filter(|c| c.rgb != background.rgb);
    // Text is drawn in short thin strokes. Of the colours seen that way, text is the one that contrasts
    // most with the page, weighted by how much of it there is (a white canvas in a dark app contrasts
    // as much as its labels, but has no glyph-like strokes).
    let glyphs = glyph_pixels(&labels, w, h);
    let mut glyph_count = vec![0usize; clusters.len()];
    for (i, &g) in glyphs.iter().enumerate() {
        if g {
            glyph_count[labels[i] as usize] += 1;
        }
    }
    let bg = hex(background.rgb);
    let text_label = (0..clusters.len())
        .filter(|&i| {
            i != page as usize && glyph_count[i] >= 200 && contrast(hex(clusters[i].rgb), bg) >= 3.0
        })
        .max_by(|&a, &b| {
            let score = |i: usize| contrast(hex(clusters[i].rgb), bg) * glyph_count[i] as f32;
            score(a).total_cmp(&score(b))
        });
    let text = match text_label {
        Some(i) => clusters[i].rgb,
        None => {
            notes.push(Note::NoText);
            if background.lab.l > 50.0 { 0 } else { 0xffffff }
        }
    };
    let accent = others()
        .filter(|c| c.rgb != text && c.share >= 0.0005 && chroma(c.lab) >= CHROMATIC)
        .max_by(|a, b| a.share.total_cmp(&b.share))
        .map_or_else(
            || {
                notes.push(Note::NoAccent);
                text
            },
            |c| c.rgb,
        );
    let surface = others()
        .filter(|c| {
            c.rgb != text
                && c.rgb != accent
                && c.share >= 0.01
                && chroma(c.lab) < CHROMATIC
                && (c.lab.l - background.lab.l).abs() <= 20.0
        })
        .max_by(|a, b| a.share.total_cmp(&b.share))
        .map_or_else(
            || {
                notes.push(Note::NoSurface);
                let step = if background.lab.l < 50.0 { 5.0 } else { -4.0 };
                from_lab(Lab::new(
                    background.lab.l + step,
                    background.lab.a,
                    background.lab.b,
                ))
            },
            |c| c.rgb,
        );

    let boxes = boxes(&region, &regions, w, h);
    let radii = radii(&region, w, &boxes);
    let gaps = gaps(&boxes);
    if boxes.is_empty() {
        notes.push(Note::NoBoxes);
    } else if gaps.iter().map(|g| g.1).sum::<u32>() < MIN_GAPS {
        notes.push(Note::FewGaps);
    }
    let line_height = line_height(&labels, &glyphs, w, h, page);

    Some(Extracted {
        style: Style {
            background: background.rgb,
            surface,
            accent,
            text,
            radius: corner_radius(&radii),
            spacing: rhythm(&gaps),
            line_height,
        },
        palette: clusters.iter().map(|c| (c.rgb, c.share)).collect(),
        radii,
        gaps: gaps.into_iter().take(4).collect(),
        notes,
    })
}

/// The image's colours by share, largest first, near-duplicates merged.
fn palette_of(pixels: &[u32]) -> Vec<Cluster> {
    let stride = pixels.len().div_ceil(SAMPLE).max(1);
    let sample: Vec<u32> = pixels.iter().step_by(stride).copied().collect();
    // k-means needs at least k distinct colours.
    let mut seen = std::collections::HashSet::new();
    for p in &sample {
        seen.insert(*p);
        if seen.len() >= K {
            break;
        }
    }
    let distinct = seen.len();
    let labs: Vec<Lab> = sample.iter().map(|p| lab(*p)).collect();
    let k = K.min(distinct).max(1);
    let result = get_kmeans_hamerly(k, 30, 1.0, false, &labs, 0);
    // Each cluster becomes its most frequent exact colour.
    let mut counts: Vec<HashMap<u32, u32>> = vec![HashMap::new(); k];
    for (p, &i) in sample.iter().zip(&result.indices) {
        *counts[i as usize].entry(*p).or_default() += 1;
    }
    let mut clusters: Vec<Cluster> = Lab::sort_indexed_colors(&result.centroids, &result.indices)
        .into_iter()
        .filter_map(|c| {
            let (&rgb, _) = counts[c.index as usize]
                .iter()
                .max_by_key(|(rgb, n)| (**n, std::cmp::Reverse(**rgb)))?;
            Some(Cluster {
                rgb,
                lab: lab(rgb),
                share: c.percentage,
            })
        })
        .collect();
    clusters.sort_by(|a, b| b.share.total_cmp(&a.share));
    let mut merged: Vec<Cluster> = Vec::new();
    for c in clusters {
        match merged.iter_mut().find(|m| delta_e(m.lab, c.lab) < SAME) {
            Some(m) => m.share += c.share,
            None => merged.push(c),
        }
    }
    merged
}

/// Each pixel's palette index, or [`NONE`].
fn label(pixels: &[u32], clusters: &[Cluster]) -> Vec<u8> {
    let mut last = (u32::MAX, NONE);
    pixels
        .iter()
        .map(|&p| {
            if p != last.0 {
                let best = clusters
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (dist2(p, c.rgb), i))
                    .min()
                    .filter(|(d, _)| *d <= NEAR)
                    .map_or(NONE, |(_, i)| i as u8);
                last = (p, best);
            }
            last.1
        })
        .collect()
}

/// A connected region (4-connected) of one palette colour. `x1`, `y1` exclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Region {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    /// Its index in the region list, which is its value in the region map.
    id: u32,
}

impl Region {
    fn w(&self) -> usize {
        self.x1 - self.x0
    }
    fn h(&self) -> usize {
        self.y1 - self.y0
    }
    fn contains(&self, o: &Region) -> bool {
        self.id != o.id && self.x0 <= o.x0 && self.y0 <= o.y0 && self.x1 >= o.x1 && self.y1 >= o.y1
    }
}

/// Labels every pixel with its region (`u32::MAX` for pixels of no palette colour) and lists the
/// regions.
fn regions(labels: &[u8], w: usize, h: usize) -> (Vec<u32>, Vec<Region>) {
    let mut map = vec![u32::MAX; labels.len()];
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for seed in 0..labels.len() {
        if map[seed] != u32::MAX || labels[seed] == NONE {
            continue;
        }
        let id = regions.len() as u32;
        let l = labels[seed];
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        map[seed] = id;
        stack.push(seed);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            let mut visit = |j: usize| {
                if map[j] == u32::MAX && labels[j] == l {
                    map[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        regions.push(Region { x0, y0, x1, y1, id });
    }
    (map, regions)
}

/// Longest run of one colour a glyph stroke makes, px: across (a bar of an `m`) and down (an `l`).
const GLYPH_RUN: (usize, usize) = (16, 24);

/// Pixels that look like glyph strokes: a palette colour next to another colour (or to none), in a
/// short run both across and down. Rules, outlines and long lines are thin but long one way; fills
/// are long both ways.
fn glyph_pixels(labels: &[u8], w: usize, h: usize) -> Vec<bool> {
    // Length of the run of equal labels each pixel sits in, along rows and along columns.
    let runs = |len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![0u16; labels.len()];
        for line in 0..count {
            let mut start = 0;
            for k in 1..=len {
                if k == len || labels[at(line, k)] != labels[at(line, start)] {
                    let n = (k - start).min(u16::MAX as usize) as u16;
                    (start..k).for_each(|j| out[at(line, j)] = n);
                    start = k;
                }
            }
        }
        out
    };
    let across = runs(w, h, &|y, x| y * w + x);
    let down = runs(h, w, &|x, y| y * w + x);
    (0..labels.len())
        .map(|i| {
            let (x, y, l) = (i % w, i / w, labels[i]);
            let edge = (x > 0 && labels[i - 1] != l)
                || (x + 1 < w && labels[i + 1] != l)
                || (y > 0 && labels[i - w] != l)
                || (y + 1 < h && labels[i + w] != l);
            l != NONE
                && edge
                && across[i] as usize <= GLYPH_RUN.0
                && down[i] as usize <= GLYPH_RUN.1
        })
        .collect()
}

/// The palette index most common along the image's edges.
fn edge_colour(labels: &[u8], w: usize, h: usize) -> Option<u8> {
    let mut counts = [0usize; 256];
    let edges = (0..w)
        .flat_map(|x| [x, (h - 1) * w + x])
        .chain((0..h).flat_map(|y| [y * w, y * w + w - 1]));
    for i in edges {
        counts[labels[i] as usize] += 1;
    }
    counts[NONE as usize] = 0;
    let (best, n) = counts
        .iter()
        .enumerate()
        .max_by_key(|&(i, n)| (*n, std::cmp::Reverse(i)))?;
    (*n > 0).then_some(best as u8)
}

/// Smallest box side worth measuring, px: controls and cards, not icons or colour swatches.
const MIN_BOX: usize = 24;

/// Regions that are boxes (cards, buttons, fields): they fill their bounding box, sit inside the
/// window and are not long strips. Largest first, at most 400.
fn boxes(map: &[u32], regions: &[Region], w: usize, h: usize) -> Vec<Region> {
    let mut boxes: Vec<Region> = regions
        .iter()
        .filter(|b| {
            // Regions cut off by the window's edge (the page, a sidebar) and strips between rules
            // are not components.
            let inside = b.x0 > 0 && b.y0 > 0 && b.x1 < w && b.y1 < h;
            let (short, long) = (b.w().min(b.h()), b.w().max(b.h()));
            inside && short >= MIN_BOX && long <= 10 * short && outer_fill(map, w, b) >= 0.92
        })
        .copied()
        .collect();
    boxes.sort_by_key(|b| std::cmp::Reverse(b.w() * b.h()));
    boxes.truncate(400);
    boxes
}

/// The leftmost and rightmost column of `b` in row `y`.
fn row_extent(map: &[u32], w: usize, b: &Region, y: usize) -> Option<(usize, usize)> {
    let row = &map[y * w + b.x0..y * w + b.x1];
    let first = row.iter().position(|r| *r == b.id)?;
    let last = row.iter().rposition(|r| *r == b.id)?;
    Some((b.x0 + first, b.x0 + last))
}

/// How much of `b`'s bounding box its outline (holes filled) covers.
fn outer_fill(map: &[u32], w: usize, b: &Region) -> f32 {
    let covered: usize = (b.y0..b.y1)
        .filter_map(|y| row_extent(map, w, b, y))
        .map(|(l, r)| r + 1 - l)
        .sum();
    covered as f32 / (b.w() * b.h()) as f32
}

/// Corner radius of each box, clustered: most common first, each box size counted once. Pills and circles (radius about half
/// the short side) are left out: they say nothing about the app's corner radius.
fn radii(map: &[u32], w: usize, boxes: &[Region]) -> Vec<(u32, u32)> {
    let mut counts: HashMap<u32, u32> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for b in boxes {
        let depth = (b.w().min(b.h()) / 2).min(64);
        let rows_top: Vec<_> = (0..depth)
            .map(|k| row_extent(map, w, b, b.y0 + k))
            .collect();
        let rows_bottom: Vec<_> = (0..depth)
            .map(|k| row_extent(map, w, b, b.y1 - 1 - k))
            .collect();
        // How far each row's edge sits inside the box; the straight middle of the edge is the baseline
        // (anti-aliasing can move it a pixel).
        let mid = row_extent(map, w, b, b.y0 + b.h() / 2);
        let Some((mid_l, mid_r)) = mid else { continue };
        let (base_l, base_r) = (mid_l - b.x0, b.x1 - 1 - mid_r);
        let missing = |rows: &[Option<(usize, usize)>], left: bool| -> f32 {
            rows.iter()
                .map(|e| match e {
                    Some((l, r)) => {
                        let inset = if left { l - b.x0 } else { b.x1 - 1 - r };
                        inset.saturating_sub(if left { base_l } else { base_r }) as f32
                    }
                    None => depth as f32,
                })
                .sum()
        };
        let mut corners = [
            missing(&rows_top, true),
            missing(&rows_top, false),
            missing(&rows_bottom, true),
            missing(&rows_bottom, false),
        ];
        corners.sort_by(f32::total_cmp);
        // A quarter disc of radius r leaves (1 − π/4)·r² of its square uncovered.
        let area = (corners[1] + corners[2]) / 2.0;
        // Pixels count as covered by their centre, which loses about half a pixel per row of the arc.
        let r = if area <= 1.5 {
            0.0
        } else {
            (area / (1.0 - std::f32::consts::FRAC_PI_4)).sqrt() + 0.5
        };
        if r >= 0.4 * b.w().min(b.h()) as f32 {
            continue;
        }
        // Boxes of one size vote once: a canvas grid or a pattern is many boxes but one design choice.
        let r = r.round() as u32;
        if seen.insert((b.w(), b.h(), r)) {
            *counts.entry(r).or_default() += 1;
        }
    }
    let mut out: Vec<_> = counts.into_iter().collect();
    out.sort_by_key(|&(r, n)| (std::cmp::Reverse(n), r));
    out
}

/// The app's corner radius: the most common rounded corner when at least a quarter of the boxes are
/// rounded, else 0. Sharp boxes are often content (a canvas, a page, an image), not the app's own
/// controls.
fn corner_radius(radii: &[(u32, u32)]) -> f32 {
    let total: u32 = radii.iter().map(|r| r.1).sum();
    radii
        .iter()
        .find(|r| r.0 > 0)
        .filter(|_| radii.iter().filter(|o| o.0 > 0).map(|o| o.1).sum::<u32>() * 4 >= total)
        .map_or(0.0, |r| r.0 as f32)
}

/// Gaps between neighbouring boxes and insets of boxes in their container, most common first.
fn gaps(boxes: &[Region]) -> Vec<(u32, u32)> {
    let mut counts: HashMap<u32, u32> = HashMap::new();
    // As with radii, boxes of one size vote once per gap.
    let mut seen = std::collections::HashSet::new();
    for b in boxes {
        let mut add = |v: usize| {
            if (2..=128).contains(&v) && seen.insert((v, b.w(), b.h())) {
                *counts.entry(v as u32).or_default() += 1;
            }
        };
        if let Some(parent) = boxes
            .iter()
            .filter(|p| p.contains(b))
            .min_by_key(|p| p.w() * p.h())
        {
            add(b.x0 - parent.x0);
            add(b.y0 - parent.y0);
            add(parent.x1 - b.x1);
            add(parent.y1 - b.y1);
        }
        let overlap = |a0: usize, a1: usize, b0: usize, b1: usize| {
            a1.min(b1).saturating_sub(a0.max(b0)) * 2 >= (a1 - a0).min(b1 - b0)
        };
        let unrelated = |o: &&Region| !o.contains(b) && !b.contains(o);
        let right = boxes
            .iter()
            .filter(unrelated)
            .filter(|o| o.x0 >= b.x1 && overlap(b.y0, b.y1, o.y0, o.y1))
            .map(|o| o.x0 - b.x1)
            .min();
        let below = boxes
            .iter()
            .filter(unrelated)
            .filter(|o| o.y0 >= b.y1 && overlap(b.x0, b.x1, o.x0, o.x1))
            .map(|o| o.y0 - b.y1)
            .min();
        right.into_iter().chain(below).for_each(&mut add);
    }
    let mut out: Vec<_> = counts.into_iter().collect();
    out.sort_by_key(|&(v, n)| (std::cmp::Reverse(n), v));
    out
}

/// 8 px when the gaps keep to multiples of 8 nearly as well as to multiples of 4, else 4. 8 when
/// too little was measured.
/// Fewest gaps to read a rhythm from.
const MIN_GAPS: u32 = 4;

fn rhythm(gaps: &[(u32, u32)]) -> f32 {
    let total: u32 = gaps.iter().map(|g| g.1).sum();
    if total < MIN_GAPS {
        return 8.0;
    }
    let on = |unit: u32| -> f32 {
        let n: u32 = gaps
            .iter()
            .filter(|(v, _)| {
                let r = v % unit;
                r <= 1 || unit - r <= 1
            })
            .map(|g| g.1)
            .sum();
        n as f32 / total as f32
    };
    if on(8) >= 0.7 * on(4) { 8.0 } else { 4.0 }
}

/// Median distance between consecutive lines of text of similar height, from the rows that hold
/// glyph strokes (any colour but the page's: a document's text counts) in 160 px wide columns, keeping
/// runs at least three times as wide as tall. `None` unless three pairs agree with it to a
/// pixel.
fn line_height(labels: &[u8], glyphs: &[bool], w: usize, h: usize, page: u8) -> Option<f32> {
    const STRIP: usize = 160;
    let mut pitches = Vec::new();
    for x0 in (0..w).step_by(STRIP) {
        let x1 = (x0 + STRIP).min(w);
        // Runs of rows holding text: (first row, height, leftmost and rightmost column).
        let mut runs: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut open: Option<(usize, usize, usize)> = None;
        for y in 0..=h {
            let ink = if y < h {
                let mut cols = (x0..x1).filter(|&x| glyphs[y * w + x] && labels[y * w + x] != page);
                cols.next().map(|l| (l, cols.next_back().unwrap_or(l)))
            } else {
                None
            };
            match (ink, open) {
                (Some((l, r)), None) => open = Some((y, l, r)),
                (Some((l, r)), Some((s, ol, or))) => open = Some((s, ol.min(l), or.max(r))),
                (None, Some((s, l, r))) => {
                    runs.push((s, y - s, l, r));
                    open = None;
                }
                (None, None) => {}
            }
        }
        // A line of text is much wider than it is tall; an icon, a box edge or a rule is not.
        runs.retain(|&(_, rh, l, r)| r - l >= 3 * rh);
        for pair in runs.windows(2) {
            let ((s0, h0, ..), (s1, h1, ..)) = (pair[0], pair[1]);
            let similar = (4..=64).contains(&h0)
                && (4..=64).contains(&h1)
                && h0.abs_diff(h1) * 100 <= h0.max(h1) * 35;
            let gap = s1 - (s0 + h0);
            // Lines of a paragraph sit closer than they are tall; rows of a list don't.
            if similar && gap >= 1 && gap * 10 <= h0 * 8 {
                pitches.push(s1 - s0);
            }
        }
    }
    pitches.sort_unstable();
    let median = *pitches.get(pitches.len() / 2)?;
    let agreeing = pitches.iter().filter(|p| p.abs_diff(median) <= 1).count();
    (agreeing >= 3).then_some(median as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic app screen: light page, white cards with 8 px corners 16 px apart and 24 px from
    /// the edge, a blue button in each, lines of dark glyphs 20 px apart.
    fn screen() -> (Vec<u8>, u32, u32) {
        let (w, h) = (640usize, 400usize);
        let mut px = vec![0xf4f5f7u32; w * h];
        let mut rect = |x0: usize, y0: usize, x1: usize, y1: usize, r: f32, c: u32| {
            for y in y0..y1 {
                for x in x0..x1 {
                    // Distance outside the rounded corner, as a crisp (unantialiased) mask.
                    let cx = (x as f32 + 0.5).clamp(x0 as f32 + r, x1 as f32 - r);
                    let cy = (y as f32 + 0.5).clamp(y0 as f32 + r, y1 as f32 - r);
                    if (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) <= r {
                        px[y * w + x] = c;
                    }
                }
            }
        };
        for i in 0..3 {
            let x0 = 24 + i * (184 + 16);
            rect(x0, 24, x0 + 184, 360, 8.0, 0xffffff);
            rect(x0 + 16, 296, x0 + 168, 336, 8.0, 0x2563eb);
            // Lines of "glyphs" 10 px apart: 7 × 14 rings (with a counter, like an `o`) and bars (an `l`).
            for line in 0..8 {
                let y = 48 + line * 20;
                for (n, gx) in (x0 + 16..x0 + 150 - line * 7).step_by(10).enumerate() {
                    if n % 3 == 0 {
                        rect(gx, y, gx + 7, y + 14, 0.0, 0x1f2328);
                        rect(gx + 2, y + 2, gx + 5, y + 12, 0.0, 0xffffff);
                    } else {
                        rect(gx, y, gx + 2, y + 14, 0.0, 0x1f2328);
                    }
                }
            }
        }
        let rgba = px
            .iter()
            .flat_map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8, 255])
            .collect();
        (rgba, w as u32, h as u32)
    }

    #[test]
    fn reads_roles_radius_spacing_and_line_height_off_a_screen() {
        let (px, w, h) = screen();
        let e = extract(&px, w, h).unwrap();
        let s = e.style;
        assert_eq!(s.background, 0xf4f5f7, "{e:?}");
        assert_eq!(s.surface, 0xffffff, "{e:?}");
        assert_eq!(s.accent, 0x2563eb, "{e:?}");
        assert_eq!(s.text, 0x1f2328, "{e:?}");
        assert_eq!(s.radius, 8.0, "{e:?}");
        assert_eq!(s.spacing, 8.0, "{e:?}");
        assert_eq!(s.line_height, Some(20.0), "{e:?}");
        assert!(e.notes.is_empty(), "{:?}", e.notes);
        assert_eq!(s.on_accent(), 0xffffff, "white reads better on blue");
    }

    #[test]
    fn wcag_checks_flag_low_contrast_pairs() {
        let good = Style {
            background: 0xffffff,
            surface: 0xf4f5f7,
            accent: 0x1f5fd1,
            text: 0x16181e,
            radius: 8.0,
            spacing: 8.0,
            line_height: None,
        };
        assert_eq!(good.failing().count(), 0);
        let pale = Style {
            text: 0xaaaaaa,
            accent: 0x9fc4ff,
            ..good
        };
        let failing: Vec<Pair> = pale.failing().map(|c| c.pair).collect();
        assert!(failing.contains(&Pair::TextOnBackground));
        assert!(failing.contains(&Pair::AccentOnBackground));
        // Black on white is the maximum, 21:1.
        let max = Style { text: 0, ..good }.check(Pair::TextOnBackground);
        assert!((max.ratio - 21.0).abs() < 1e-3, "{max:?}");
    }

    #[test]
    fn a_flat_image_says_what_it_could_not_find() {
        let px = [0x10u8, 0x12, 0x18, 255].repeat(64 * 64);
        let e = extract(&px, 64, 64).unwrap();
        assert_eq!(e.style.background, 0x101218);
        assert_eq!(e.style.text, 0xffffff, "assumed: white on a dark page");
        for n in [Note::NoText, Note::NoAccent, Note::NoSurface, Note::NoBoxes] {
            assert!(e.notes.contains(&n), "{n:?} in {:?}", e.notes);
        }
        assert!(extract(&px, 0, 64).is_none());
        assert!(extract(&px[..100], 64, 64).is_none());
    }

    #[test]
    fn style_survives_storage() {
        let (px, w, h) = screen();
        let s = extract(&px, w, h).unwrap().style;
        let bytes = postcard::to_allocvec(&s).unwrap();
        assert_eq!(postcard::from_bytes::<Style>(&bytes).unwrap(), s);
    }
}
