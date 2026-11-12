//! Rule-deck driven DRC on the flattened hierarchy, built on Manhattan region operations.
//!
//! Deck syntax (one rule per line, lengths in µm, areas in µm², `#` comments):
//! ```text
//! [NAME:] width      LAYER MIN
//! [NAME:] space      LAYER MIN
//! [NAME:] separation LAYER_A LAYER_B MIN
//! [NAME:] enclosure  INNER OUTER MIN
//! [NAME:] extension  LAYER_A LAYER_B MIN
//! [NAME:] area       LAYER MIN
//! ```
//! Rule kinds and layer names are case-insensitive. Unnamed rules are called
//! `<layer>.W`, `<layer>.S`, `<a>.SEP.<b>`, `<inner>.EN.<outer>`, `<a>.EX.<b>` and `<layer>.A`.
//!
//! Every check works on the merged geometry of each layer (all Manhattan shapes of the
//! flattened top cell; labels are ignored and non-Manhattan shapes are skipped and counted).
//! Coordinates are integers, so a region is a set of 1 x 1 dbu pixels and the morphology
//! below is exact for any (odd or even) MIN. Distances use the square (Chebyshev) metric:
//! two pieces `dx` apart horizontally and `dy` apart vertically are `max(dx, dy)` apart.
//!
//! - `width L MIN`: the parts of L not covered by any MIN x MIN square inside L (L minus its
//!   morphological opening with a MIN square). Long narrow bars, necks and slivers are flagged;
//!   square corners are not.
//! - `space L MIN`: empty space covered by L's closing with a MIN square (every MIN x MIN square
//!   through the point touches L), i.e. gaps narrower than MIN between facing edges whose
//!   projections overlap, including notches and holes of a single shape; plus diagonal
//!   corner-to-corner gaps: material inside the MIN x MIN square diagonally outside a convex
//!   corner. Shapes touching only at a corner are flagged (distance 0).
//! - `separation A B MIN`: B pieces (edge-connected components) that do not overlap A must stay
//!   MIN away from A. Flags the parts of such B pieces within MIN of A (touching counts).
//!   B pieces overlapping A are considered intentionally joined to it and are not checked, so
//!   put the layer whose shapes are checked individually second.
//! - `enclosure INNER OUTER MIN`: INNER pieces that overlap OUTER must lie inside OUTER with
//!   a margin of at least MIN on every side. Flags `INNER NOT (OUTER sized -MIN)` on those
//!   pieces; INNER pieces entirely outside OUTER are not checked.
//! - `extension A B MIN`: wherever an edge of `A AND B` lies on an edge of B (A crosses out of
//!   B, e.g. a poly gate leaving active), A must continue for at least MIN beyond that edge,
//!   with the full width of the crossing. Flags the missing part of that MIN deep band.
//!   Edges of `A AND B` that are edges of A inside B carry no requirement.
//! - `area L MIN`: edge-connected pieces of L with an area below MIN.
//!
//! Every connected piece of a rule's violation region becomes one [`Violation`] marked by its
//! bounding box, at most [`MAX_VIOLATIONS_PER_RULE`] per rule (see [`DrcReport::capped`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rstar::{AABB, RTree, RTreeObject};

use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::region::{BoolOp, Region};
use crate::tech::Tech;

/// Markers reported per rule; further pieces are only counted.
pub const MAX_VIOLATIONS_PER_RULE: usize = 10_000;

const MAX_DEPTH: u32 = 64;
const POLY_ENDCAP_UM: &str = "0.22";

/// A rule with lengths in dbu and areas in dbu². Layers are tech layer indices.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Width { layer: usize, min: i64 },
    Space { layer: usize, min: i64 },
    Separation { a: usize, b: usize, min: i64 },
    Enclosure { inner: usize, outer: usize, min: i64 },
    Extension { a: usize, b: usize, min: i64 },
    Area { layer: usize, min: i128 },
}

impl Rule {
    /// Layers the rule reads.
    pub fn layers(&self) -> Vec<usize> {
        match *self {
            Rule::Width { layer, .. } | Rule::Space { layer, .. } | Rule::Area { layer, .. } => vec![layer],
            Rule::Separation { a, b, .. } | Rule::Extension { a, b, .. } => vec![a, b],
            Rule::Enclosure { inner, outer, .. } => vec![inner, outer],
        }
    }

    /// Layer the violations are reported on.
    pub fn checked_layer(&self) -> usize {
        match *self {
            Rule::Width { layer, .. } | Rule::Space { layer, .. } | Rule::Area { layer, .. } => layer,
            Rule::Separation { b, .. } => b,
            Rule::Enclosure { inner, .. } => inner,
            Rule::Extension { a, .. } => a,
        }
    }

    /// Name used when the deck gives none, e.g. `metal1.W` or `contact.EN.active`.
    pub fn auto_name(&self, tech: &Tech) -> String {
        let n = |l: usize| layer_name(tech, l);
        match *self {
            Rule::Width { layer, .. } => format!("{}.W", n(layer)),
            Rule::Space { layer, .. } => format!("{}.S", n(layer)),
            Rule::Separation { a, b, .. } => format!("{}.SEP.{}", n(a), n(b)),
            Rule::Enclosure { inner, outer, .. } => format!("{}.EN.{}", n(inner), n(outer)),
            Rule::Extension { a, b, .. } => format!("{}.EX.{}", n(a), n(b)),
            Rule::Area { layer, .. } => format!("{}.A", n(layer)),
        }
    }

    /// Human readable, e.g. "metal1 width < 0.23 µm".
    pub fn description(&self, tech: &Tech) -> String {
        let n = |l: usize| layer_name(tech, l);
        let um = |v: i64| fmt_um(v, tech);
        match *self {
            Rule::Width { layer, min } => format!("{} width < {} µm", n(layer), um(min)),
            Rule::Space { layer, min } => format!("{} spacing < {} µm", n(layer), um(min)),
            Rule::Separation { a, b, min } => format!("{} separation from {} < {} µm", n(b), n(a), um(min)),
            Rule::Enclosure { inner, outer, min } => format!("{} enclosure by {} < {} µm", n(inner), n(outer), um(min)),
            Rule::Extension { a, b, min } => format!("{} extension past {} < {} µm", n(a), n(b), um(min)),
            Rule::Area { layer, min } => {
                let d = tech.dbu_per_um.max(1) as f64;
                format!("{} area < {} µm²", n(layer), min as f64 / (d * d))
            }
        }
    }

    /// Deck line for this rule (without a name).
    pub fn deck_line(&self, tech: &Tech) -> String {
        let n = |l: usize| layer_name(tech, l);
        let um = |v: i64| fmt_um(v, tech);
        match *self {
            Rule::Width { layer, min } => format!("width {} {}", n(layer), um(min)),
            Rule::Space { layer, min } => format!("space {} {}", n(layer), um(min)),
            Rule::Separation { a, b, min } => format!("separation {} {} {}", n(a), n(b), um(min)),
            Rule::Enclosure { inner, outer, min } => format!("enclosure {} {} {}", n(inner), n(outer), um(min)),
            Rule::Extension { a, b, min } => format!("extension {} {} {}", n(a), n(b), um(min)),
            Rule::Area { layer, min } => {
                let d = tech.dbu_per_um.max(1) as f64;
                format!("area {} {}", n(layer), min as f64 / (d * d))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedRule {
    pub name: String,
    pub rule: Rule,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Deck {
    pub rules: Vec<NamedRule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeckError {
    /// 1-based line number.
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for DeckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub rule: String,
    /// Human readable, e.g. "metal1 width < 0.23 µm".
    pub description: String,
    pub layer: Option<usize>,
    /// Bounding box of the offending geometry (top-cell coordinates).
    pub marker: IRect,
}

#[derive(Debug, Clone, Default)]
pub struct DrcReport {
    pub violations: Vec<Violation>,
    pub rules_run: usize,
    /// Flattened shapes checked.
    pub shapes: usize,
    /// Shapes skipped because they are not Manhattan.
    pub skipped: usize,
    pub millis: f64,
    /// Rules with more than [`MAX_VIOLATIONS_PER_RULE`] violations and their full count.
    pub capped: Vec<(String, usize)>,
}

fn layer_name(tech: &Tech, l: usize) -> String {
    tech.layer(l).map_or_else(|| format!("#{l}"), |d| d.name.clone())
}

fn fmt_um(v: i64, tech: &Tech) -> String {
    format!("{}", v as f64 / tech.dbu_per_um.max(1) as f64)
}

/// Parse a deck. All errors are reported, not just the first.
pub fn parse_deck(text: &str, tech: &Tech) -> Result<Deck, Vec<DeckError>> {
    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line, tech) {
            Ok(r) => rules.push(r),
            Err(es) => errors.extend(es.into_iter().map(|message| DeckError { line: i + 1, message })),
        }
    }
    if errors.is_empty() { Ok(Deck { rules }) } else { Err(errors) }
}

fn parse_line(line: &str, tech: &Tech) -> Result<NamedRule, Vec<String>> {
    let (name, body) = match line.split_once(':') {
        Some((n, b)) => {
            let n = n.trim();
            if n.is_empty() || n.contains(char::is_whitespace) {
                return Err(vec![format!("invalid rule name '{n}'")]);
            }
            (Some(n.to_string()), b)
        }
        None => (None, line),
    };
    let toks: Vec<&str> = body.split_whitespace().collect();
    let Some(kind) = toks.first().map(|k| k.to_ascii_lowercase()) else {
        return Err(vec!["missing rule after the name".into()]);
    };
    let n_layers = match kind.as_str() {
        "width" | "space" | "area" => 1,
        "separation" | "enclosure" | "extension" => 2,
        _ => {
            return Err(vec![format!(
                "unknown rule kind '{kind}' (expected width, space, separation, enclosure, extension or area)"
            )]);
        }
    };
    if toks.len() != n_layers + 2 {
        let expect = if n_layers == 1 { "LAYER MIN" } else { "LAYER LAYER MIN" };
        return Err(vec![format!("'{kind}' expects {expect} ({} arguments), got {}", n_layers + 1, toks.len() - 1)]);
    }
    let mut errs = Vec::new();
    let mut layers = Vec::new();
    for t in &toks[1..=n_layers] {
        match tech.layer_index(t) {
            Some(l) => layers.push(l),
            None => errs.push(format!("unknown layer '{t}'")),
        }
    }
    let num = toks[n_layers + 1];
    let value = num.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    if value.is_none() {
        errs.push(format!("bad number '{num}' (expected a non-negative value in µm)"));
    }
    let Some(v) = value.filter(|_| errs.is_empty()) else { return Err(errs) };
    let d = tech.dbu_per_um as f64;
    let min = (v * d).round() as i64;
    let rule = match kind.as_str() {
        "width" => Rule::Width { layer: layers[0], min },
        "space" => Rule::Space { layer: layers[0], min },
        "area" => Rule::Area { layer: layers[0], min: (v * d * d).round() as i128 },
        "separation" => Rule::Separation { a: layers[0], b: layers[1], min },
        "enclosure" => Rule::Enclosure { inner: layers[0], outer: layers[1], min },
        _ => Rule::Extension { a: layers[0], b: layers[1], min },
    };
    let name = name.unwrap_or_else(|| rule.auto_name(tech));
    Ok(NamedRule { name, rule })
}

/// A deck derived from the technology: width / space per layer plus via enclosure rules.
pub fn default_deck_text(tech: &Tech) -> String {
    let um = |v: i64| fmt_um(v, tech);
    let mut lines: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !lines.contains(&s) {
            lines.push(s);
        }
    };
    for l in &tech.layers {
        if l.min_width > 0 {
            push(format!("width {} {}", l.name, um(l.min_width)));
        }
        if l.min_spacing > 0 {
            push(format!("space {} {}", l.name, um(l.min_spacing)));
        }
    }
    for v in &tech.vias {
        let def = |n: &str| tech.layer_index(n).and_then(|i| tech.layer(i));
        let (Some(bottom), Some(cut), Some(top)) = (def(&v.bottom), def(&v.cut), def(&v.top)) else { continue };
        if v.cut_size > cut.min_width {
            push(format!("width {} {}", cut.name, um(v.cut_size)));
        }
        if v.cut_spacing > cut.min_spacing {
            push(format!("space {} {}", cut.name, um(v.cut_spacing)));
        }
        push(format!("enclosure {} {} {}", cut.name, bottom.name, um(v.enc_bottom)));
        push(format!("enclosure {} {} {}", cut.name, top.name, um(v.enc_top)));
    }
    let def = |n: &str| tech.layer_index(n).and_then(|i| tech.layer(i));
    if let (Some(poly), Some(active)) = (def("poly"), def("active")) {
        push(format!("extension {} {} {POLY_ENDCAP_UM}", poly.name, active.name));
    }
    let mut out = format!("# DRC deck derived from technology '{}' (lengths in µm)\n", tech.name);
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Run the technology's deck ([`Tech::deck_text`]).
pub fn run_tech_deck(top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Result<DrcReport, Vec<DeckError>> {
    Ok(run(&parse_deck(&tech.deck_text(), tech)?, top, src, tech))
}

pub fn run(deck: &Deck, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> DrcReport {
    let start = Instant::now();
    let n = tech.layers.len();
    let mut used = vec![false; n];
    for l in deck.rules.iter().flat_map(|r| r.rule.layers()) {
        if l < n {
            used[l] = true;
        }
    }
    let mut rects: Vec<Vec<IRect>> = vec![Vec::new(); n];
    let (mut shapes, mut skipped) = (0, 0);
    for f in flatten(top, src, MAX_DEPTH) {
        if f.layer >= n || matches!(f.geom, Geometry::Label { .. }) {
            continue;
        }
        shapes += 1;
        match &f.geom {
            Geometry::Rect(r) if used[f.layer] => rects[f.layer].push(*r),
            Geometry::Rect(_) => {}
            g => match Region::from_geometry(g) {
                Ok(r) if used[f.layer] => rects[f.layer].extend_from_slice(r.rects()),
                Ok(_) => {}
                Err(_) => skipped += 1,
            },
        }
    }
    let regions = par_map(&rects, |r| Region::from_rects(r.iter().copied()));
    let empty = Region::default();
    let layer = |l: usize| regions.get(l).unwrap_or(&empty);
    let results = par_map(&deck.rules, |nr| check(&nr.rule, &layer).parts());

    let mut report = DrcReport { rules_run: deck.rules.len(), shapes, skipped, ..Default::default() };
    for (nr, parts) in deck.rules.iter().zip(results) {
        if parts.len() > MAX_VIOLATIONS_PER_RULE {
            report.capped.push((nr.name.clone(), parts.len()));
        }
        let description = nr.rule.description(tech);
        let checked = nr.rule.checked_layer();
        for p in parts.iter().take(MAX_VIOLATIONS_PER_RULE) {
            if let Some(marker) = p.bbox() {
                report.violations.push(Violation {
                    rule: nr.name.clone(),
                    description: description.clone(),
                    layer: Some(checked),
                    marker,
                });
            }
        }
    }
    report.millis = start.elapsed().as_secs_f64() * 1000.0;
    report
}

fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(items.len());
    if workers <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    *slots[i].lock().expect("result slot") = Some(r);
                }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().expect("result slot").expect("every item mapped")).collect()
}

/// The violation region of one rule.
fn check<'a>(rule: &Rule, layer: &impl Fn(usize) -> &'a Region) -> Region {
    match *rule {
        Rule::Width { layer: l, min } => {
            let a = layer(l);
            if min <= 1 {
                return Region::default();
            }
            let k = min - 1;
            a.boolean(&a.eroded(0, 0, k, k).dilated(0, 0, k, k), BoolOp::Not)
        }
        Rule::Space { layer: l, min } => space(layer(l), min),
        Rule::Separation { a, b, min } => {
            let (a, b) = (layer(a), layer(b));
            let (_, free) = split_overlapping(b, a);
            a.sized(min).boolean(&free, BoolOp::And)
        }
        Rule::Enclosure { inner, outer, min } => {
            let outer = layer(outer);
            let (hit, _) = split_overlapping(layer(inner), outer);
            hit.boolean(&outer.sized(-min), BoolOp::Not)
        }
        Rule::Extension { a, b, min } => extension(layer(a), layer(b), min),
        Rule::Area { layer: l, min } => {
            Region::from_rects(layer(l).parts().into_iter().filter(|p| p.area() < min).flat_map(|p| p.rects().to_vec()))
        }
    }
}

fn space(a: &Region, min: i64) -> Region {
    if min <= 0 || a.is_empty() {
        return Region::default();
    }
    let k = min - 1;
    let gaps = a.dilated(0, 0, k, k).eroded(0, 0, k, k).boolean(a, BoolOp::Not);
    gaps.boolean(&Region::from_rects(corner_gaps(a, min)), BoolOp::Or)
}

/// For every convex corner, the box from the corner to the nearest material inside the
/// MIN x MIN square diagonally outside it (widened to 2 dbu across a zero gap).
/// Material whose projection overlaps the corner's edges is left to the closing.
fn corner_gaps(a: &Region, min: i64) -> Vec<IRect> {
    let tree = RTree::bulk_load(a.rects().iter().map(|&r| Boxed(r)).collect());
    let inside = |x: i64, y: i64| {
        tree.locate_in_envelope_intersecting(AABB::from_point([x, y]))
            .any(|b| b.0.x0 <= x && x < b.0.x1 && b.0.y0 <= y && y < b.0.y1)
    };
    let mut out = Vec::new();
    for (sx, sy) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
        let corners = a.boolean(&a.translated(-sx, 0), BoolOp::Not).boolean(&a.translated(0, -sy), BoolOp::Not);
        for p in corners.rects() {
            let cx = if sx > 0 { p.x1 } else { p.x0 };
            let cy = if sy > 0 { p.y1 } else { p.y0 };
            let q = IRect::new(cx, cy, cx + sx * min, cy + sy * min);
            let reach = |c: &IRect| {
                let dx = if sx > 0 { c.x0 - cx } else { cx - c.x1 };
                let dy = if sy > 0 { c.y0 - cy } else { cy - c.y1 };
                (dx.max(dy), dx + dy)
            };
            let nearest = tree
                .locate_in_envelope_intersecting(aabb(q))
                .filter(|b| b.0.intersects(&q))
                .map(|b| IRect::new(b.0.x0.max(q.x0), b.0.y0.max(q.y0), b.0.x1.min(q.x1), b.0.y1.min(q.y1)))
                .min_by_key(reach);
            let Some(c) = nearest else { continue };
            let mut m = IRect::new(cx, cy, if sx > 0 { c.x0 } else { c.x1 }, if sy > 0 { c.y0 } else { c.y1 });
            let (near_x, near_y) = (if sx > 0 { c.x0 } else { c.x1 - 1 }, if sy > 0 { c.y0 } else { c.y1 - 1 });
            let (back_x, back_y) = (if sx > 0 { cx - 1 } else { cx }, if sy > 0 { cy - 1 } else { cy });
            let projected = (m.width() == 0 && m.height() > 0 && inside(back_x, near_y))
                || (m.height() == 0 && m.width() > 0 && inside(near_x, back_y));
            if projected {
                continue;
            }
            if m.width() == 0 {
                (m.x0, m.x1) = (m.x0 - 1, m.x1 + 1);
            }
            if m.height() == 0 {
                (m.y0, m.y1) = (m.y0 - 1, m.y1 + 1);
            }
            out.push(m);
        }
    }
    out
}

fn extension(a: &Region, b: &Region, min: i64) -> Region {
    let x = a.boolean(b, BoolOp::And);
    if min <= 0 || x.is_empty() {
        return Region::default();
    }
    let k = min - 1;
    let mut required = Vec::new();
    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let edge = x.translated(dx, dy).boolean(&x, BoolOp::Not).boolean(b, BoolOp::Not);
        let (l, r) = (if dx < 0 { k } else { 0 }, if dx > 0 { k } else { 0 });
        let (d, u) = (if dy < 0 { k } else { 0 }, if dy > 0 { k } else { 0 });
        required.extend_from_slice(edge.dilated(l, d, r, u).rects());
    }
    Region::from_rects(required).boolean(a, BoolOp::Not)
}

/// Pieces of `a` that overlap `b` (positive area) and the remaining pieces.
fn split_overlapping(a: &Region, b: &Region) -> (Region, Region) {
    if b.is_empty() {
        return (Region::default(), a.clone());
    }
    let tree = RTree::bulk_load(b.rects().iter().map(|&r| Boxed(r)).collect());
    let (mut hit, mut free) = (Vec::new(), Vec::new());
    for p in a.parts() {
        let overlaps = p.rects().iter().any(|r| tree.locate_in_envelope_intersecting(aabb(*r)).any(|q| q.0.intersects(r)));
        if overlaps { &mut hit } else { &mut free }.extend_from_slice(p.rects());
    }
    (Region::from_rects(hit), Region::from_rects(free))
}

struct Boxed(IRect);

impl RTreeObject for Boxed {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        aabb(self.0)
    }
}

fn aabb(r: IRect) -> AABB<[i64; 2]> {
    AABB::from_corners([r.x0, r.y0], [r.x1, r.y1])
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::hier::NoCells;
    use crate::layout::Trans;
    use crate::project::CellRef;

    fn tech() -> Tech {
        Tech::generic()
    }

    fn li(name: &str) -> usize {
        tech().layer_index(name).unwrap()
    }

    type Coords = (i64, i64, i64, i64);

    fn layout(shapes: &[(&str, Coords)]) -> Layout {
        let mut l = Layout::default();
        for &(layer, (x0, y0, x1, y1)) in shapes {
            l.add_rect(li(layer), x0, y0, x1, y1);
        }
        l
    }

    fn markers(deck: &str, lay: &Layout) -> Vec<IRect> {
        let t = tech();
        let d = parse_deck(deck, &t).unwrap();
        run(&d, lay, &NoCells, &t).violations.into_iter().map(|v| v.marker).collect()
    }

    fn r(x0: i64, y0: i64, x1: i64, y1: i64) -> IRect {
        IRect::new(x0, y0, x1, y1)
    }

    #[test]
    fn width() {
        let deck = "width metal1 0.23";
        assert_eq!(markers(deck, &layout(&[("metal1", (0, 0, 220, 2000))])), [r(0, 0, 220, 2000)]);
        assert!(markers(deck, &layout(&[("metal1", (0, 0, 230, 2000))])).is_empty());
        let l_shape = layout(&[("metal1", (0, 0, 2000, 230)), ("metal1", (0, 0, 230, 2000))]);
        assert!(markers(deck, &l_shape).is_empty());
        let neck =
            layout(&[("metal1", (0, 0, 1000, 1000)), ("metal1", (1000, 400, 2000, 600)), ("metal1", (2000, 0, 3000, 1000))]);
        assert_eq!(markers(deck, &neck), [r(1000, 400, 2000, 600)]);
        let odd = "width metal1 0.231";
        assert_eq!(markers(odd, &layout(&[("metal1", (0, 0, 230, 900))])).len(), 1);
        assert!(markers(odd, &layout(&[("metal1", (0, 0, 231, 900))])).is_empty());
        let mut v = layout(&[]);
        v.add(li("metal1"), Geometry::Path { points: vec![(0, 0), (1000, 0), (1000, 1000)], width: 230 });
        assert!(markers(deck, &v).is_empty());
    }

    #[test]
    fn space() {
        let deck = "space metal1 0.23";
        let pair = |gap: i64| layout(&[("metal1", (0, 0, 500, 1000)), ("metal1", (500 + gap, 200, 1500, 800))]);
        assert_eq!(markers(deck, &pair(200)), [r(500, 200, 700, 800)]);
        assert!(markers(deck, &pair(230)).is_empty());
        assert!(markers(deck, &pair(0)).is_empty());
        let odd = "space metal1 0.231";
        assert_eq!(markers(odd, &pair(230)).len(), 1);
        assert!(markers(odd, &pair(231)).is_empty());
        // Notch of a single U-shaped polygon.
        let u = |w: i64| {
            layout(&[("metal1", (0, 0, 1000 + w, 300)), ("metal1", (0, 0, 500, 1500)), ("metal1", (500 + w, 0, 1000 + w, 1500))])
        };
        assert_eq!(markers(deck, &u(200)), [r(500, 300, 700, 1500)]);
        assert!(markers(deck, &u(230)).is_empty());
        // Diagonal: measured as max(dx, dy).
        let diag = |dx: i64, dy: i64| layout(&[("metal1", (0, 0, 500, 500)), ("metal1", (500 + dx, 500 + dy, 1500, 1500))]);
        assert_eq!(markers(deck, &diag(100, 150)), [r(500, 500, 600, 650)]);
        assert_eq!(markers(deck, &diag(0, 100)), [r(499, 500, 501, 600)]);
        assert!(markers(deck, &diag(100, 230)).is_empty());
        assert!(markers(deck, &diag(300, 100)).is_empty());
        assert_eq!(markers(deck, &diag(0, 0)), [r(499, 499, 501, 501)]);
        // A hole narrower than the rule is a gap.
        let mut ring = Layout::default();
        let hole = Region::from_rects([r(0, 0, 2000, 2000)]).boolean(&Region::from_rects([r(500, 500, 700, 1500)]), BoolOp::Not);
        for g in hole.to_geometry() {
            ring.add(li("metal1"), g);
        }
        assert_eq!(markers(deck, &ring), [r(500, 500, 700, 1500)]);
    }

    #[test]
    fn separation() {
        let deck = "separation poly contact 0.16";
        let lay = |x: i64| layout(&[("poly", (0, 0, 180, 2000)), ("contact", (x, 500, x + 220, 720))]);
        assert_eq!(markers(deck, &lay(280)), [r(280, 500, 340, 720)]);
        assert_eq!(markers(deck, &lay(180)), [r(180, 500, 340, 720)]);
        assert!(markers(deck, &lay(340)).is_empty());
        assert!(markers(deck, &lay(-20)).is_empty(), "contacts landing on poly are exempt");
        let diag = layout(&[("poly", (0, 0, 180, 2000)), ("contact", (280, 2100, 500, 2320))]);
        assert_eq!(markers(deck, &diag), [r(280, 2100, 340, 2160)]);
    }

    #[test]
    fn enclosure() {
        let deck = "enclosure contact active 0.1";
        let lay = |x: i64| layout(&[("active", (0, 0, 1000, 1000)), ("contact", (x, 400, x + 220, 620))]);
        assert!(markers(deck, &lay(100)).is_empty());
        assert!(markers(deck, &lay(680)).is_empty());
        assert_eq!(markers(deck, &lay(50)), [r(50, 400, 100, 620)]);
        assert_eq!(markers(deck, &lay(900)), [r(900, 400, 1120, 620)]);
        assert!(markers(deck, &lay(2000)).is_empty(), "cuts outside the outer layer are not checked");
        let edge = layout(&[("active", (0, 0, 1000, 1000)), ("contact", (100, 850, 320, 1070))]);
        assert_eq!(markers(deck, &edge), [r(100, 900, 320, 1070)]);
    }

    #[test]
    fn extension() {
        let deck = "extension poly active 0.22";
        let gate = |bottom: i64, top: i64| layout(&[("active", (0, 0, 1500, 1000)), ("poly", (600, -bottom, 780, 1000 + top))]);
        assert!(markers(deck, &gate(220, 300)).is_empty());
        assert_eq!(markers(deck, &gate(100, 300)), [r(600, -220, 780, -100)]);
        assert_eq!(markers(deck, &gate(220, 0)), [r(600, 1000, 780, 1220)]);
        // Poly edges inside active and narrow diffusion beside the gate carry no requirement.
        assert!(markers(deck, &layout(&[("active", (0, 0, 1500, 1000)), ("poly", (600, 200, 780, 800))])).is_empty());
        assert!(markers(deck, &layout(&[("active", (0, 0, 800, 1000)), ("poly", (600, -220, 780, 1220))])).is_empty());
        // Poly covering the active edge must extend past it too.
        assert_eq!(
            markers(deck, &layout(&[("active", (0, 0, 800, 1000)), ("poly", (700, -220, 900, 1220))])),
            [r(900, 0, 1020, 1000)]
        );
    }

    #[test]
    fn area() {
        let deck = "M1.A: area metal1 0.1";
        assert_eq!(markers(deck, &layout(&[("metal1", (0, 0, 200, 300))])), [r(0, 0, 200, 300)]);
        assert!(markers(deck, &layout(&[("metal1", (0, 0, 400, 400))])).is_empty());
        assert!(markers(deck, &layout(&[("metal1", (0, 0, 200, 300)), ("metal1", (200, 0, 400, 300))])).is_empty());
        let t = tech();
        let rep = run(&parse_deck(deck, &t).unwrap(), &layout(&[("metal1", (0, 0, 10, 10))]), &NoCells, &t);
        assert_eq!(rep.violations[0].rule, "M1.A");
        assert_eq!(rep.violations[0].description, "metal1 area < 0.1 µm²");
    }

    #[test]
    fn deck_parsing() {
        let t = tech();
        let d =
            parse_deck("M1.W: width metal1 0.23\n  # comment\nSPACE Metal1 0.25 # trailing\nenclosure contact active 0.1", &t)
                .unwrap();
        assert_eq!(d.rules.len(), 3);
        assert_eq!(d.rules[0], NamedRule { name: "M1.W".into(), rule: Rule::Width { layer: li("metal1"), min: 230 } });
        assert_eq!(d.rules[1].name, "metal1.S");
        assert_eq!(d.rules[2].name, "contact.EN.active");
        let bad = "width metal1 0.23\nwidth metal9 x\nfoo metal1 1\nspace metal1\n\nseparation poly 1\nbad name: width metal1 1\narea metal1 -1";
        let errs = parse_deck(bad, &t).unwrap_err();
        let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
        assert_eq!(lines, [2, 2, 3, 4, 6, 7, 8], "{errs:?}");
        assert!(errs[0].message.contains("metal9"));
        assert!(errs[1].message.contains("bad number"));
        assert!(errs[2].message.contains("unknown rule kind"));
        assert!(errs[3].to_string().starts_with("line 4:"));
    }

    #[test]
    fn default_deck_round_trip() {
        let t = tech();
        let text = default_deck_text(&t);
        let deck = parse_deck(&text, &t).unwrap();
        let names: Vec<&str> = deck.rules.iter().map(|r| r.name.as_str()).collect();
        for n in
            ["metal1.W", "metal1.S", "nwell.S", "contact.EN.active", "contact.EN.poly", "contact.EN.metal1", "via1.EN.metal2"]
        {
            assert!(names.contains(&n), "{n} missing in {names:?}");
        }
        assert_eq!(names.iter().filter(|n| **n == "contact.EN.metal1").count(), 1);
        assert!(deck.rules.contains(&NamedRule {
            name: "poly.EX.active".into(),
            rule: Rule::Extension { a: li("poly"), b: li("active"), min: 220 }
        }));
        let lines: String = deck.rules.iter().map(|r| r.rule.deck_line(&t) + "\n").collect();
        assert_eq!(parse_deck(&lines, &t).unwrap(), deck);
        assert_eq!(t.deck_text(), text);
        let custom = Tech { drc_deck: Some("width metal1 1".into()), ..t };
        assert_eq!(custom.deck_text(), "width metal1 1");
    }

    #[test]
    fn hierarchy_skipped_and_cap() {
        let t = tech();
        let leaf = CellRef::new("w", "leaf");
        let mut src = HashMap::new();
        src.insert(leaf.clone(), layout(&[("metal1", (0, 0, 100, 1000))]));
        let mut top = Layout::default();
        top.add_instance(leaf, Trans::new(5000, 0), None);
        top.add(li("metal1"), Geometry::Polygon { points: vec![(0, 0), (100, 0), (50, 80)] });
        top.add(li("text"), Geometry::Label { text: "x".into(), at: (0, 0), size: 10 });
        let rep = run(&parse_deck("width metal1 0.23", &t).unwrap(), &top, &src, &t);
        assert_eq!((rep.shapes, rep.skipped, rep.rules_run), (2, 1, 1));
        assert_eq!(rep.violations[0].marker, r(5000, 0, 5100, 1000));
        assert_eq!(rep.violations[0].layer, Some(li("metal1")));

        let bars: Vec<(&str, Coords)> =
            (0..MAX_VIOLATIONS_PER_RULE as i64 + 5).map(|i| ("metal1", (i * 1000, 0, i * 1000 + 100, 1000))).collect();
        let rep = run(&parse_deck("width metal1 0.23", &t).unwrap(), &layout(&bars), &NoCells, &t);
        assert_eq!(rep.violations.len(), MAX_VIOLATIONS_PER_RULE);
        assert_eq!(rep.capped, [("metal1.W".to_string(), MAX_VIOLATIONS_PER_RULE + 5)]);
    }

    #[test]
    fn demo_cells() {
        let p = crate::examples::demo_project();
        for cell in ["cmos_inverter", "inv_chain", "devices"] {
            let lay = p.layout(&CellRef::new("work", cell)).unwrap();
            let rep = run_tech_deck(lay, &p, &p.tech).unwrap();
            let found: Vec<String> = rep.violations.iter().map(|v| format!("{} {:?}", v.rule, v.marker)).collect();
            assert!(found.is_empty(), "{cell}: {found:#?}");
            assert_eq!(rep.skipped, 0);
        }
    }

    #[test]
    #[ignore]
    fn bench_inv_tiles() {
        let p = crate::examples::demo_project();
        let lay = p.layout(&CellRef::new("work", "inv_tiles")).unwrap();
        let rep = run_tech_deck(lay, &p, &p.tech).unwrap();
        println!(
            "inv_tiles: {} shapes, {} rules, {} violations in {:.0} ms",
            rep.shapes,
            rep.rules_run,
            rep.violations.len(),
            rep.millis
        );
    }
}
