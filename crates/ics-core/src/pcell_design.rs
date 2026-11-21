//! PCell Designer: turn a drawn layout into a parametric cell (graphical PCell, as in the
//! Virtuoso PCell Designer).
//!
//! A [`Template`] keeps a copy of the drawn layout (the *source*), its parameters and a list of
//! operations, all in source coordinates:
//!
//! - [`Op::Condition`]: items inside a region are dropped when an expression is false.
//! - [`Op::Stretch`]: a line across the cell; coordinates on the moving side shift by
//!   `amount(values) - amount(defaults)`, so shapes crossing the line grow.
//! - [`Op::Repeat`]: items inside a region are copied `count - 1` times `pitch` apart, and the
//!   items that span the region (diffusion, implants, a gate strap) are stretched to follow.
//! - [`Op::ContactFill`]: a rectangle refilled with the largest centred array of via cuts after
//!   stretching, so contacts are re-arrayed instead of stretched.
//!
//! Generation runs them in that order: conditions, stretches (each also moves the later lines
//! and regions), repeats, fills. Expressions ([`parse_expr`]) are in µm over the parameter
//! names. [`Template::lint`] reports mistakes, [`Template::check_corners`] runs DRC on the
//! parameter extremes, and the `suggest_*` helpers read a drawn device to propose contact
//! fills, a finger repeat (reducing the drawing to one unit) and W / L stretch lines.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::drc::{self, Deck, Violation};
use crate::geom::IRect;
use crate::hier::LayoutSource;
use crate::layout::{CellInst, Geometry, Layout};
use crate::tech::Tech;

/// Parameter values by name (µm for lengths, counts, 0 / 1 for booleans).
pub type Values = BTreeMap<String, f64>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    /// A length in µm, snapped to the manufacturing grid.
    #[default]
    Length,
    /// A whole number (finger count, rows).
    Int,
    /// On / off (0 or 1 in expressions).
    Bool,
}

impl ParamKind {
    pub const ALL: [ParamKind; 3] = [ParamKind::Length, ParamKind::Int, ParamKind::Bool];

    pub fn label(self) -> &'static str {
        match self {
            ParamKind::Length => "length",
            ParamKind::Int => "integer",
            ParamKind::Bool => "boolean",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    #[serde(default)]
    pub kind: ParamKind,
    pub default: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl Param {
    pub fn new(name: &str, kind: ParamKind, default: f64) -> Param {
        Param { name: name.into(), kind, default, min: None, max: None, description: String::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// A vertical line; coordinates move along x.
    #[default]
    X,
    /// A horizontal line; coordinates move along y.
    Y,
}

impl Axis {
    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
        }
    }

    fn lo(self, r: &IRect) -> i64 {
        match self {
            Axis::X => r.x0,
            Axis::Y => r.y0,
        }
    }

    fn hi(self, r: &IRect) -> i64 {
        match self {
            Axis::X => r.x1,
            Axis::Y => r.y1,
        }
    }

    fn vec(self, d: i64) -> (i64, i64) {
        match self {
            Axis::X => (d, 0),
            Axis::Y => (0, d),
        }
    }
}

/// Which side of a stretch line moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Right of a vertical line, above a horizontal one.
    #[default]
    High,
    /// Left / below; they move away (to lower coordinates) as the amount grows.
    Low,
    /// Both sides, half each, so the cell grows about the line.
    Both,
}

impl Side {
    pub const ALL: [Side; 3] = [Side::High, Side::Low, Side::Both];

    pub fn label(self, axis: Axis) -> &'static str {
        match (self, axis) {
            (Side::High, Axis::X) => "right side moves",
            (Side::High, Axis::Y) => "top side moves",
            (Side::Low, Axis::X) => "left side moves",
            (Side::Low, Axis::Y) => "bottom side moves",
            (Side::Both, _) => "both sides (centred)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Stretch {
        axis: Axis,
        /// Line position (x of a vertical line, y of a horizontal one), source dbu.
        at: i64,
        #[serde(default)]
        side: Side,
        /// Expression in µm; the line moves by its change from the defaults.
        amount: String,
        /// Layers that stretch; empty = all (instances only move with all layers).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        layers: Vec<usize>,
    },
    Repeat {
        axis: Axis,
        /// Items wholly inside are repeated.
        region: IRect,
        count: String,
        /// Expression in µm.
        pitch: String,
        /// Where items spanning the region are stretched (`count - 1`) × pitch; `None` is the
        /// middle of the region.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        layers: Vec<usize>,
    },
    Condition {
        region: IRect,
        /// Items inside are kept when this is non-zero.
        when: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        layers: Vec<usize>,
    },
    ContactFill {
        /// Area filled with cuts of `via` (the cuts' extent in the source).
        rect: IRect,
        via: String,
    },
}

impl Op {
    pub fn title(&self) -> &'static str {
        match self {
            Op::Stretch { .. } => "Stretch",
            Op::Repeat { .. } => "Repeat",
            Op::Condition { .. } => "Condition",
            Op::ContactFill { .. } => "Contact fill",
        }
    }

    /// One-line summary, e.g. "stretch x @ 0.41 by w".
    pub fn summary(&self, dbu: i64) -> String {
        let um = |v: i64| fmt_um(v, dbu);
        match self {
            Op::Stretch { axis, at, side, amount, .. } => {
                let s = match side {
                    Side::High => "",
                    Side::Low => " (low side)",
                    Side::Both => " (centred)",
                };
                format!("stretch {} @ {} by {amount}{s}", axis.label(), um(*at))
            }
            Op::Repeat { axis, count, pitch, .. } => format!("repeat along {}: {count} copies, pitch {pitch}", axis.label()),
            Op::Condition { when, .. } => format!("keep if {when}"),
            Op::ContactFill { via, rect } => format!("{via} fill {} × {} µm", um(rect.width()), um(rect.height())),
        }
    }

    fn exprs(&self) -> Vec<(&'static str, &str)> {
        match self {
            Op::Stretch { amount, .. } => vec![("amount", amount)],
            Op::Repeat { count, pitch, .. } => vec![("count", count), ("pitch", pitch)],
            Op::Condition { when, .. } => vec![("condition", when)],
            Op::ContactFill { .. } => vec![],
        }
    }

    fn layers(&self) -> &[usize] {
        match self {
            Op::Stretch { layers, .. } | Op::Repeat { layers, .. } | Op::Condition { layers, .. } => layers,
            Op::ContactFill { .. } => &[],
        }
    }
}

/// A parametric cell drawn by example.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// Cell name of the master; variants are named `<name>_<hash>`.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The drawn layout (without the cuts that contact fills own).
    pub source: Layout,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub ops: Vec<Op>,
    /// Layers that operations on all layers leave alone (markers, IDs, boundaries drawn
    /// once). An operation that names its layers still acts on them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<usize>,
}

fn fmt_um(v: i64, dbu: i64) -> String {
    let s = format!("{:.4}", v as f64 / dbu.max(1) as f64);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn snap(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g.div_euclid(2)).div_euclid(g) * g }
}

fn to_dbu(um: f64, tech: &Tech) -> i64 {
    snap((um * tech.dbu_per_um as f64).round() as i64, tech.grid)
}

fn inside(outer: &IRect, r: &IRect) -> bool {
    r.x0 >= outer.x0 && r.y0 >= outer.y0 && r.x1 <= outer.x1 && r.y1 <= outer.y1
}

/// A working item of generation: a shape, an instance (its origin as a zero-size rectangle) or
/// a contact-fill rectangle.
#[derive(Debug, Clone)]
struct Item {
    layer: Option<usize>,
    geom: Geometry,
    inst: Option<CellInst>,
    fill: Option<String>,
    /// Extent after stretching, before repeats: what repeat regions select on.
    home: IRect,
}

impl Item {
    fn bbox(&self) -> IRect {
        self.geom.bbox()
    }

    /// Whether an operation on `layers` (empty = all but the `fixed` ones) acts on the item.
    fn on(&self, layers: &[usize], fixed: &[usize]) -> bool {
        match self.layer {
            Some(l) => acts_on(layers, fixed, l),
            None => layers.is_empty(),
        }
    }

    fn map(&mut self, axis: Axis, f: &dyn Fn(i64) -> i64) {
        map_geom(&mut self.geom, axis, f);
    }

    fn translate(&mut self, (dx, dy): (i64, i64)) {
        self.geom = self.geom.transformed(&crate::layout::Trans::new(dx, dy));
    }

    /// Text identifying the item's geometry up to position, for finding translated copies.
    /// Labels compare by position only (S / D labels alternate between fingers).
    fn key(&self, d: (i64, i64)) -> String {
        let mut g = self.geom.transformed(&crate::layout::Trans::new(d.0, d.1));
        if let Geometry::Label { text, .. } = &mut g {
            text.clear();
        }
        canonical_points(&mut g);
        let inst = self.inst.as_ref().map(|i| (i.cell.to_string(), i.trans.rot, i.trans.mirror, i.array));
        format!("{:?}|{:?}|{:?}|{g:?}", self.layer, self.fill, inst)
    }
}

/// The diffusion role layers, quoted, for messages.
fn diffusion_names(tech: &Tech) -> String {
    let d = &tech.roles.diffusion;
    if d.is_empty() { "(none set)".into() } else { d.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ") }
}

const ROLES_HINT: &str = "If the PDK names them differently, set the technology's device layer roles";

fn no_gate(tech: &Tech) -> String {
    format!(
        "No gate found: no shape on the gate layer '{}' overlaps the diffusion layer(s) {}. {ROLES_HINT}",
        tech.roles.poly,
        diffusion_names(tech)
    )
}

/// Whether an operation on `layers` (empty = all but `fixed`) acts on `layer`.
fn acts_on(layers: &[usize], fixed: &[usize], layer: usize) -> bool {
    if layers.is_empty() { !fixed.contains(&layer) } else { layers.contains(&layer) }
}

fn map_geom(g: &mut Geometry, axis: Axis, f: &dyn Fn(i64) -> i64) {
    let m = |p: &mut (i64, i64)| match axis {
        Axis::X => p.0 = f(p.0),
        Axis::Y => p.1 = f(p.1),
    };
    match g {
        Geometry::Rect(r) => match axis {
            Axis::X => (r.x0, r.x1) = (f(r.x0), f(r.x1)),
            Axis::Y => (r.y0, r.y1) = (f(r.y0), f(r.y1)),
        },
        Geometry::Polygon { points } | Geometry::Path { points, .. } => points.iter_mut().for_each(m),
        Geometry::Label { at, .. } => m(at),
    }
}

fn map_rect(r: &mut IRect, axis: Axis, f: &dyn Fn(i64) -> i64) {
    let mut g = Geometry::Rect(*r);
    map_geom(&mut g, axis, f);
    if let Geometry::Rect(n) = g {
        *r = n;
    }
}

/// One stretch along an axis: coordinates above `at` move by `hi`, those below by `-lo`, the
/// line itself stays.
#[derive(Debug, Clone, Copy)]
struct Shift {
    at: i64,
    hi: i64,
    lo: i64,
    /// Manufacturing grid, for cut points on edges at other angles than 45°.
    grid: i64,
}

impl Shift {
    fn new(at: i64, side: Side, d: i64, g: i64) -> Shift {
        let (hi, lo) = match side {
            Side::High => (d, 0),
            Side::Low => (0, d),
            Side::Both => {
                let h = snap(d / 2, g);
                (h, d - h)
            }
        };
        Shift { at, hi, lo, grid: g }
    }

    fn map(&self, c: i64) -> i64 {
        if c > self.at {
            c + self.hi
        } else if c < self.at {
            c - self.lo
        } else {
            c
        }
    }

    /// Where the kept part of the low side ends (source coordinate) and where that lands. A
    /// shrinking side loses the band next to the line.
    fn low_end(&self) -> (i64, i64) {
        let x = self.at + self.lo.min(0);
        (x, x - self.lo)
    }

    fn high_end(&self) -> (i64, i64) {
        let x = self.at - self.hi.min(0);
        (x, x + self.hi)
    }
}

/// (along `axis`, across it) of a point, and back.
fn split(axis: Axis, p: (i64, i64)) -> (i64, i64) {
    match axis {
        Axis::X => p,
        Axis::Y => (p.1, p.0),
    }
}

fn join(axis: Axis, a: i64, o: i64) -> (i64, i64) {
    split(axis, (a, o))
}

/// Stretch the vertices of a polygon ring (`closed`) or path. `map` moves a coordinate along
/// the axis (`s.map`, possibly wrapped). An edge that is not parallel to the axis and crosses
/// the line keeps its angle: it is cut where each side's kept part ends and the two pieces
/// are joined along the axis, so a band is inserted (or removed) at the line as for Manhattan
/// shapes. Repeated vertices are dropped. `None` when an edge of a polygon along the axis
/// collapses to nothing (a part of the shape, or a notch, is squeezed out).
fn stretch_points(pts: &[(i64, i64)], closed: bool, axis: Axis, s: &Shift, map: &dyn Fn(i64) -> i64) -> Option<Vec<(i64, i64)>> {
    let n = pts.len();
    let edges = if closed { n } else { n.saturating_sub(1) };
    let side = |a: i64| (a - s.at).signum();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(n + 4);
    let push = |out: &mut Vec<(i64, i64)>, q: (i64, i64)| {
        if out.last() != Some(&q) {
            out.push(q);
        }
    };
    let mut collapsed = false;
    for k in 0..n {
        let (a0, o0) = split(axis, pts[k]);
        push(&mut out, join(axis, map(a0), o0));
        if k >= edges {
            continue;
        }
        let (a1, o1) = split(axis, pts[(k + 1) % n]);
        if closed && o0 == o1 && a0 != a1 && map(a0) == map(a1) {
            collapsed = true;
        }
        let (s0, s1) = (side(a0), side(a1));
        if o0 == o1 || s0 == s1 {
            continue;
        }
        for sd in [s0, s1] {
            if sd == 0 {
                continue;
            }
            let (x, img) = if sd < 0 { s.low_end() } else { s.high_end() };
            let t = ((x - a0) as f64 / (a1 - a0) as f64).clamp(0.0, 1.0);
            let mut o = o0 + ((o1 - o0) as f64 * t).round() as i64;
            if (o1 - o0).abs() != (a1 - a0).abs() {
                o = snap(o, s.grid);
            }
            push(&mut out, join(axis, img, o));
        }
    }
    if closed {
        while out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
    }
    (!collapsed).then_some(out)
}

/// Stretch one geometry (see [`stretch_points`]); `false` when a polygon collapses.
fn stretch_geom(g: &mut Geometry, axis: Axis, s: &Shift, map: &dyn Fn(i64) -> i64) -> bool {
    let (points, closed) = match g {
        Geometry::Polygon { points } => (points, true),
        Geometry::Path { points, .. } => (points, false),
        _ => {
            map_geom(g, axis, map);
            return true;
        }
    };
    match stretch_points(points, closed, axis, s, map) {
        Some(p) => {
            *points = p;
            true
        }
        None => false,
    }
}

/// Rectangles making up a shape: rectilinear polygons decomposed, any-angle ones and labels
/// give none.
fn rects_of(g: &Geometry) -> Vec<IRect> {
    match g {
        Geometry::Rect(r) => vec![*r],
        Geometry::Label { .. } => vec![],
        g => crate::region::Region::from_geometry(g).map(|r| r.rects().to_vec()).unwrap_or_else(|_| g.rects()),
    }
}

/// The rectangle a shape is, also when drawn as a four-corner polygon.
fn as_rect(g: &Geometry) -> Option<IRect> {
    match g {
        Geometry::Rect(r) => Some(*r),
        Geometry::Polygon { points } => crate::gds::as_rect(points),
        _ => None,
    }
}

/// A polygon ring from its smallest vertex, counter-clockwise; a path in its smaller
/// direction: the same figure gives the same points however it was drawn.
fn canonical_points(g: &mut Geometry) {
    match g {
        Geometry::Polygon { points } if !points.is_empty() => {
            let area2: i128 = (0..points.len())
                .map(|k| {
                    let (a, b) = (points[k], points[(k + 1) % points.len()]);
                    a.0 as i128 * b.1 as i128 - b.0 as i128 * a.1 as i128
                })
                .sum();
            if area2 < 0 {
                points.reverse();
            }
            let m = (0..points.len()).min_by_key(|&k| points[k]).unwrap_or(0);
            points.rotate_left(m);
        }
        Geometry::Path { points, .. } if points.iter().rev().lt(points.iter()) => points.reverse(),
        _ => {}
    }
}

impl Template {
    /// A template of the drawn layout `source`, without parameters yet.
    pub fn from_layout(name: &str, source: &Layout) -> Template {
        let mut source = source.clone();
        source.constraints.clear();
        source.rulers.clear();
        source.tracks.clear();
        Template { name: name.into(), description: String::new(), source, params: vec![], ops: vec![], fixed: vec![] }
    }

    /// Layers the source and the operations use (not the fills' cut layers).
    pub fn layers(&self) -> BTreeSet<usize> {
        let mut out: BTreeSet<usize> = self.source.shapes.iter().map(|s| s.layer).collect();
        out.extend(self.ops.iter().flat_map(|o| o.layers().iter().copied()));
        out.extend(self.fixed.iter().copied());
        out
    }

    /// Move every layer index through `map` (a technology change).
    pub fn remap_layers(&mut self, map: &dyn Fn(usize) -> usize) {
        for s in &mut self.source.shapes {
            s.layer = map(s.layer);
        }
        for op in &mut self.ops {
            if let Op::Stretch { layers, .. } | Op::Repeat { layers, .. } | Op::Condition { layers, .. } = op {
                for l in layers.iter_mut() {
                    *l = map(*l);
                }
            }
        }
        for l in &mut self.fixed {
            *l = map(*l);
        }
    }

    /// Scale the operations' positions (dbu) by `f`, for a new database unit; the source is
    /// scaled with the other layouts.
    pub fn scale_positions(&mut self, f: f64) {
        let s = |v: &mut i64| *v = (*v as f64 * f).round() as i64;
        let r = |q: &mut IRect| {
            for v in [&mut q.x0, &mut q.y0, &mut q.x1, &mut q.y1] {
                s(v);
            }
        };
        for op in &mut self.ops {
            match op {
                Op::Stretch { at, .. } => s(at),
                Op::Repeat { region, at, .. } => {
                    r(region);
                    if let Some(a) = at {
                        s(a);
                    }
                }
                Op::Condition { region, .. } => r(region),
                Op::ContactFill { rect, .. } => r(rect),
            }
        }
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }

    pub fn defaults(&self) -> Values {
        self.params.iter().map(|p| (p.name.clone(), p.default)).collect()
    }

    /// Defaults overridden by `given`, checked against the kinds and ranges.
    pub fn resolve(&self, given: &[(String, f64)]) -> Result<Values, String> {
        let mut v = self.defaults();
        for (n, x) in given {
            let Some(p) = self.param(n) else { return Err(format!("{}: no parameter '{n}'", self.name)) };
            if !x.is_finite() {
                return Err(format!("{n} = {x} is not a number"));
            }
            match p.kind {
                ParamKind::Int if x.fract() != 0.0 => return Err(format!("{n} must be a whole number (got {})", fmt_num(*x))),
                ParamKind::Bool if *x != 0.0 && *x != 1.0 => return Err(format!("{n} must be 0 or 1")),
                _ => {}
            }
            if let Some(lo) = p.min.filter(|lo| x < lo) {
                return Err(format!("{n} = {} is below its minimum {}", fmt_num(*x), fmt_num(lo)));
            }
            if let Some(hi) = p.max.filter(|hi| x > hi) {
                return Err(format!("{n} = {} is above its maximum {}", fmt_num(*x), fmt_num(hi)));
            }
            v.insert(n.clone(), *x);
        }
        Ok(v)
    }

    fn items(&self) -> Vec<Item> {
        let shapes: Vec<(usize, Geometry)> = self.source.shapes.iter().map(|s| (s.layer, s.geom.clone())).collect();
        self.items_of(shapes)
    }

    fn items_of(&self, shapes: Vec<(usize, Geometry)>) -> Vec<Item> {
        let mut out: Vec<Item> = shapes
            .into_iter()
            .map(|(layer, geom)| {
                let home = geom.bbox();
                Item { layer: Some(layer), geom, inst: None, fill: None, home }
            })
            .collect();
        for i in &self.source.instances {
            let r = IRect::new(i.trans.x, i.trans.y, i.trans.x, i.trans.y);
            out.push(Item { layer: None, geom: Geometry::Rect(r), inst: Some(i.clone()), fill: None, home: r });
        }
        out
    }

    /// Stretch lines (and repeat stretch positions) in source coordinates, with their layers.
    fn stretch_lines(&self, grid: i64) -> Vec<(Axis, i64, &[usize])> {
        self.ops
            .iter()
            .filter_map(|o| match o {
                Op::Stretch { axis, at, layers, .. } => Some((*axis, *at, layers.as_slice())),
                Op::Repeat { axis, region, at, layers, .. } => {
                    Some((*axis, at.unwrap_or_else(|| snap((axis.lo(region) + axis.hi(region)) / 2, grid)), layers.as_slice()))
                }
                _ => None,
            })
            .collect()
    }

    /// The source shapes generation starts from. A path that a stretch line runs along inside
    /// its width (a vertex within half the width of the line) cannot widen on one side, so it
    /// becomes its outline polygon and grows like a rectangle would.
    fn generation_shapes(&self, grid: i64) -> Vec<(usize, Geometry)> {
        let lines = self.stretch_lines(grid);
        let mut out = vec![];
        for s in &self.source.shapes {
            if let Geometry::Path { points, width } = &s.geom {
                let h = width.abs() / 2;
                let cut = lines.iter().any(|(axis, at, ls)| {
                    acts_on(ls, &self.fixed, s.layer) && points.iter().any(|&p| (split(*axis, p).0 - at).abs() < h)
                });
                let outline = crate::polybool::PolySet::from_geometry(&s.geom);
                if cut && outline.polygons().iter().all(|p| p.holes.is_empty()) {
                    out.extend(outline.polygons().iter().map(|p| (s.layer, Geometry::Polygon { points: p.outer.clone() })));
                    continue;
                }
            }
            out.push((s.layer, s.geom.clone()));
        }
        out
    }

    /// The layout for parameter values `given` (the others take their defaults).
    pub fn generate(&self, tech: &Tech, given: &[(String, f64)]) -> Result<Layout, String> {
        let values = self.resolve(given)?;
        let defaults = self.defaults();
        let mut cache: HashMap<String, Expr> = HashMap::new();
        let mut eval = |text: &str, vals: &Values, what: &str| -> Result<f64, String> {
            if !cache.contains_key(text) {
                let e = parse_expr(text).map_err(|e| format!("{what} '{text}': {e}"))?;
                cache.insert(text.to_string(), e);
            }
            cache[text].eval(vals).map_err(|e| format!("{what} '{text}': {e}"))
        };
        let g = tech.grid;
        let mut items = self.items_of(self.generation_shapes(g));
        // Contact fills become rectangles that stretch and repeat like any shape.
        for op in &self.ops {
            if let Op::ContactFill { rect, via } = op {
                let def = tech.via(via).ok_or_else(|| format!("contact fill: technology has no via '{via}'"))?;
                let cut = tech.layer_index(&def.cut).ok_or_else(|| format!("via '{via}': no cut layer '{}'", def.cut))?;
                items.push(Item {
                    layer: Some(cut),
                    geom: Geometry::Rect(*rect),
                    inst: None,
                    fill: Some(via.clone()),
                    home: *rect,
                });
            }
        }
        for (k, op) in self.ops.iter().enumerate() {
            if let Op::Condition { region, when, layers } = op
                && eval(when, &values, &format!("condition {}", k + 1))? == 0.0
            {
                items.retain(|it| !(it.on(layers, &self.fixed) && inside(region, &it.bbox())));
            }
        }
        // Stretches, each moving the later lines and the repeat regions with the geometry.
        let mut lines: Vec<Option<i64>> =
            self.ops.iter().map(|o| if let Op::Stretch { at, .. } = o { Some(*at) } else { None }).collect();
        let mut regions: Vec<Option<(IRect, Option<i64>, Axis)>> = self
            .ops
            .iter()
            .map(|o| if let Op::Repeat { region, at, axis, .. } = o { Some((*region, *at, *axis)) } else { None })
            .collect();
        for (k, op) in self.ops.iter().enumerate() {
            let Op::Stretch { axis, side, amount, layers, .. } = op else { continue };
            let what = format!("stretch {}", k + 1);
            let d = to_dbu(eval(amount, &values, &what)? - eval(amount, &defaults, &what)?, tech);
            if d == 0 {
                continue;
            }
            let at = lines[k].expect("stretch line");
            let (axis, side) = (*axis, *side);
            let sh = Shift::new(at, side, d, g);
            let f = move |c: i64| sh.map(c);
            let crossed = std::cell::Cell::new(false);
            let checked = |c: i64| {
                let n = f(c);
                if (c > at && n < at) || (c < at && n > at) {
                    crossed.set(true);
                }
                n
            };
            let mut collapsed = None;
            for it in items.iter_mut().filter(|it| it.on(layers, &self.fixed)) {
                if !stretch_geom(&mut it.geom, axis, &sh, &checked) && collapsed.is_none() {
                    collapsed = Some(it.layer);
                }
            }
            if crossed.get() {
                return Err(format!("{what} ({amount}): the value is too small, edges would pass the line"));
            }
            if let Some(l) = collapsed {
                let name = l.and_then(|l| tech.layer(l)).map_or("?", |l| l.name.as_str());
                return Err(format!("{what} ({amount}) collapses a {name} shape: the value is too small"));
            }
            for (j, o) in self.ops.iter().enumerate().skip(k + 1) {
                if let (Op::Stretch { axis: a2, .. }, Some(l)) = (o, lines[j].as_mut())
                    && *a2 == axis
                {
                    *l = f(*l);
                }
            }
            for (r, rat, raxis) in regions.iter_mut().flatten() {
                map_rect(r, axis, &f);
                if let Some(x) = rat.as_mut().filter(|_| *raxis == axis) {
                    *x = f(*x);
                }
            }
            if let Some(it) = items
                .iter()
                .find(|it| it.inst.is_none() && matches!(it.geom, Geometry::Rect(r) if r.width() <= 0 || r.height() <= 0))
            {
                let name = it.layer.and_then(|l| tech.layer(l)).map_or("?", |l| l.name.as_str());
                return Err(format!("{what} ({amount}) collapses a {name} shape: the value is too small"));
            }
        }
        for it in &mut items {
            it.home = it.bbox();
        }
        for (k, op) in self.ops.iter().enumerate() {
            let Op::Repeat { axis, count, pitch, layers, .. } = op else { continue };
            let what = format!("repeat {}", k + 1);
            let n = eval(count, &values, &what)?.round();
            if !(0.0..=10_000.0).contains(&n) {
                return Err(format!("{what}: count {} is out of range", fmt_num(n)));
            }
            let n = n as i64;
            let p = to_dbu(eval(pitch, &values, &what)?, tech);
            if p <= 0 && n > 1 {
                return Err(format!("{what}: pitch must be positive"));
            }
            let (region, at, _) = regions[k].expect("repeat region");
            let axis = *axis;
            let at = at.unwrap_or_else(|| snap((axis.lo(&region) + axis.hi(&region)) / 2, g));
            let ext = (n - 1) * p;
            let (members, mut rest): (Vec<Item>, Vec<Item>) =
                items.into_iter().partition(|it| it.on(layers, &self.fixed) && inside(&region, &it.home));
            if ext != 0 {
                let sh = Shift { at, hi: ext, lo: 0, grid: g };
                for it in &mut rest {
                    stretch_geom(&mut it.geom, axis, &sh, &|c| sh.map(c));
                }
            }
            for m in &members {
                for i in 1..n {
                    let mut c = m.clone();
                    c.translate(axis.vec(i * p));
                    rest.push(c);
                }
            }
            if n >= 1 {
                rest.extend(members);
            }
            items = rest;
        }
        let mut lay = Layout::default();
        for it in items {
            if let Some(mut inst) = it.inst {
                let o = it.geom.bbox();
                (inst.trans.x, inst.trans.y) = (o.x0, o.y0);
                let id = lay.alloc_id();
                inst.id = id;
                lay.instances.push(inst);
                continue;
            }
            let layer = it.layer.expect("shape layer");
            match (&it.fill, &it.geom) {
                (Some(via), Geometry::Rect(r)) => {
                    let def = tech.via(via).expect("checked above");
                    let cuts = crate::pcell::cut_grid(*r, def.cut_size, def.cut_spacing, g);
                    if cuts.is_empty() {
                        return Err(format!(
                            "contact fill: {} × {} µm is too small for one {} cut",
                            fmt_um(r.width(), tech.dbu_per_um),
                            fmt_um(r.height(), tech.dbu_per_um),
                            def.cut
                        ));
                    }
                    for c in cuts {
                        lay.add(layer, Geometry::Rect(c));
                    }
                }
                _ => {
                    lay.add(layer, it.geom);
                }
            }
        }
        Ok(lay)
    }

    /// Cell name of the variant for `values` (resolved).
    pub fn variant_name(&self, values: &Values) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (n, v) in values {
            for b in n.bytes().chain([0]).chain(v.to_bits().to_le_bytes()) {
                h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        format!("{}_{h:016x}", self.name)
    }

    /// "nf=2 w=1.5" (values that differ from the defaults first is not needed: all, by name).
    pub fn values_text(&self, values: &Values) -> String {
        let mut s = String::new();
        for p in &self.params {
            let v = values.get(&p.name).copied().unwrap_or(p.default);
            let t = match p.kind {
                ParamKind::Length => format!("{}um", fmt_num(v)),
                ParamKind::Int => fmt_num(v),
                ParamKind::Bool => (if v != 0.0 { "on" } else { "off" }).into(),
            };
            if !s.is_empty() {
                s.push(' ');
            }
            let _ = write!(s, "{}={t}", p.name);
        }
        s
    }

    /// Parameter values after stretching the generated cell by `d` dbu along its x
    /// (`along_x`) or y axis: through the first stretch line on that axis driven by one
    /// length parameter. `None` when there is none or the value leaves its range.
    pub fn stretched(&self, values: &Values, along_x: bool, d: i64, tech: &Tech) -> Option<Values> {
        let axis = if along_x { Axis::X } else { Axis::Y };
        for op in &self.ops {
            let Op::Stretch { axis: a, amount, .. } = op else { continue };
            if *a != axis {
                continue;
            }
            let Ok(e) = parse_expr(amount) else { continue };
            let vars = e.vars();
            let [name] = vars.iter().collect::<Vec<_>>()[..] else { continue };
            let Some(p) = self.param(name).filter(|p| p.kind == ParamKind::Length) else { continue };
            let v = values.get(name).copied().unwrap_or(p.default);
            let at = |x: f64| {
                let mut vv = values.clone();
                vv.insert(name.clone(), x);
                e.eval(&vv).ok()
            };
            let slope = at(v + 1.0)? - at(v)?;
            if slope.abs() < 1e-9 {
                continue;
            }
            let dbu = tech.dbu_per_um.max(1) as f64;
            let step = tech.grid.max(1) as f64 / dbu;
            let nv = ((v + d as f64 / dbu / slope) / step).round() * step;
            let ok = nv > 0.0 && p.min.is_none_or(|m| nv >= m - 1e-9) && p.max.is_none_or(|m| nv <= m + 1e-9);
            if !ok || (nv - v).abs() < 1e-12 {
                return None;
            }
            let mut out = values.clone();
            out.insert(name.clone(), nv);
            return Some(out);
        }
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Lint

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Template,
    Param(usize),
    Op(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub target: Target,
    pub message: String,
}

const FUNCTIONS: [&str; 8] = ["min", "max", "abs", "floor", "ceil", "round", "sqrt", "if"];

fn valid_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_') && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

/// Cut layers of the technology's vias.
fn cut_layers(tech: &Tech) -> BTreeSet<usize> {
    tech.vias.iter().filter_map(|v| tech.layer_index(&v.cut)).collect()
}

impl Template {
    /// Mistakes and gaps, most severe first.
    pub fn lint(&self, tech: &Tech) -> Vec<Finding> {
        let mut out = vec![];
        let mut push = |level, target, message: String| out.push(Finding { level, target, message });
        if !crate::project::valid_name(&self.name) {
            push(Level::Error, Target::Template, format!("'{}' is not a valid cell name (letters, digits, _ and -)", self.name));
        }
        if self.source.shapes.is_empty() && self.source.instances.is_empty() {
            push(Level::Error, Target::Template, "The source layout is empty: load a drawn cell".into());
        }
        if self.params.is_empty() {
            push(Level::Info, Target::Template, "No parameters yet: add one, or use a suggestion".into());
        }
        let mut used: BTreeSet<String> = BTreeSet::new();
        let names: BTreeSet<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
        for (k, op) in self.ops.iter().enumerate() {
            for (what, text) in op.exprs() {
                match parse_expr(text) {
                    Err(e) => push(Level::Error, Target::Op(k), format!("{what} '{text}': {e}")),
                    Ok(e) => {
                        for v in e.vars() {
                            if !names.contains(v.as_str()) {
                                push(Level::Error, Target::Op(k), format!("{what} uses '{v}', which is not a parameter"));
                            }
                            used.insert(v);
                        }
                    }
                }
            }
        }
        let dbu = tech.dbu_per_um.max(1) as f64;
        for (i, p) in self.params.iter().enumerate() {
            if !valid_ident(&p.name) || FUNCTIONS.contains(&p.name.as_str()) {
                push(
                    Level::Error,
                    Target::Param(i),
                    format!("'{}' is not a usable name (letters, digits, _; not a function)", p.name),
                );
            } else if matches!(p.name.as_str(), "t" | "nil" | "pcCellView") || p.name.starts_with("ics") {
                push(Level::Error, Target::Param(i), format!("'{}' is reserved in SKILL or by the SKILL export", p.name));
            }
            if self.params.iter().filter(|q| q.name == p.name).count() > 1 {
                push(Level::Error, Target::Param(i), format!("'{}' is defined twice", p.name));
            }
            if let (Some(lo), Some(hi)) = (p.min, p.max)
                && lo > hi
            {
                push(
                    Level::Error,
                    Target::Param(i),
                    format!("{}: minimum {} is above maximum {}", p.name, fmt_num(lo), fmt_num(hi)),
                );
            }
            if p.min.is_some_and(|m| p.default < m) || p.max.is_some_and(|m| p.default > m) {
                push(Level::Error, Target::Param(i), format!("{}: default {} is outside its range", p.name, fmt_num(p.default)));
            }
            match p.kind {
                ParamKind::Int if p.default.fract() != 0.0 => {
                    push(Level::Error, Target::Param(i), format!("{}: an integer default must be whole", p.name))
                }
                ParamKind::Length => {
                    let d = p.default * dbu;
                    if (d - d.round()).abs() > 1e-6 || (d.round() as i64) % tech.grid.max(1) != 0 {
                        push(
                            Level::Warning,
                            Target::Param(i),
                            format!("{}: default {} µm is off the {} nm grid", p.name, fmt_num(p.default), tech.grid),
                        );
                    }
                }
                _ => {}
            }
            if !used.contains(&p.name) {
                push(Level::Warning, Target::Param(i), format!("{} is not used by any operation", p.name));
            }
            if p.kind != ParamKind::Bool && (p.min.is_none() || p.max.is_none()) {
                push(
                    Level::Info,
                    Target::Param(i),
                    format!("{}: set a minimum and maximum so the corner check tests the extremes", p.name),
                );
            }
        }
        let items = self.items();
        let cuts = cut_layers(tech);
        let lname = |l: usize| tech.layer(l).map_or("?".to_string(), |d| d.name.clone());
        for (k, op) in self.ops.iter().enumerate() {
            match op {
                Op::Stretch { axis, at, side, layers, .. } => {
                    let moving = |c: i64| match side {
                        Side::High => c > *at,
                        Side::Low => c < *at,
                        Side::Both => c != *at,
                    };
                    let any = items.iter().filter(|it| it.on(layers, &self.fixed)).any(|it| {
                        let b = it.bbox();
                        moving(axis.lo(&b)) || moving(axis.hi(&b))
                    });
                    if !any {
                        push(Level::Warning, Target::Op(k), "Nothing is on the moving side of the line: it has no effect".into());
                    }
                    let crossed: BTreeSet<usize> = self
                        .source
                        .shapes
                        .iter()
                        .filter(|s| cuts.contains(&s.layer) && acts_on(layers, &self.fixed, s.layer))
                        .filter(|s| {
                            let b = s.bbox();
                            axis.lo(&b) < *at && *at < axis.hi(&b)
                        })
                        .map(|s| s.layer)
                        .collect();
                    for l in crossed {
                        push(
                            Level::Warning,
                            Target::Op(k),
                            format!("The line cuts through {} cuts, which would stretch: make them a contact fill", lname(l)),
                        );
                    }
                    let skew = self.source.shapes.iter().filter(|s| acts_on(layers, &self.fixed, s.layer)).any(|s| {
                        let (pts, closed) = match &s.geom {
                            Geometry::Polygon { points } => (points, true),
                            Geometry::Path { points, .. } => (points, false),
                            _ => return false,
                        };
                        let n = pts.len();
                        (0..if closed { n } else { n.saturating_sub(1) }).any(|i| {
                            let ((a0, o0), (a1, o1)) = (split(*axis, pts[i]), split(*axis, pts[(i + 1) % n]));
                            (a0 - at).signum() * (a1 - at).signum() < 0 && o0 != o1 && (o1 - o0).abs() != (a1 - a0).abs()
                        })
                    });
                    if skew {
                        push(
                            Level::Info,
                            Target::Op(k),
                            "The line crosses an edge at an angle other than 45°: its cut point is rounded to the grid".into(),
                        );
                    }
                }
                Op::Repeat { region, .. } | Op::Condition { region, .. } => {
                    if !items.iter().any(|it| it.on(op.layers(), &self.fixed) && inside(region, &it.bbox()))
                        && !self.ops.iter().any(|o| matches!(o, Op::ContactFill { rect, .. } if inside(region, rect)))
                    {
                        push(Level::Warning, Target::Op(k), "No item lies wholly inside the region".into());
                    }
                }
                Op::ContactFill { rect, via } => match tech.via(via) {
                    None => push(Level::Error, Target::Op(k), format!("The technology has no via '{via}'")),
                    Some(d) if rect.width() < d.cut_size || rect.height() < d.cut_size => {
                        push(Level::Error, Target::Op(k), format!("The rectangle is smaller than one {} cut", d.cut))
                    }
                    _ => {}
                },
            }
        }
        if let Err(e) = self.generate(tech, &[]) {
            push(Level::Error, Target::Template, format!("Generation at the defaults fails: {e}"));
        }
        out.extend(self.drawing_findings(tech));
        out.sort_by_key(|f| f.level);
        out
    }

    /// What an imported drawing often gets wrong: shapes on layers the technology lacks, the
    /// same figure drawn twice on a layer (a pin or label datatype mapped onto the drawing
    /// layer), vertices off the manufacturing grid, cuts a contact fill cannot take over, and
    /// placed cells the operations treat as one item.
    pub fn drawing_findings(&self, tech: &Tech) -> Vec<Finding> {
        let mut out = vec![];
        let mut push = |level, message: String| out.push(Finding { level, target: Target::Template, message });
        let name = |l: usize| tech.layer(l).map_or_else(|| format!("layer #{l}"), |d| d.name.clone());
        let g = tech.grid.max(1);
        // Per layer: shapes, exact duplicates, off-grid shapes.
        let mut per: BTreeMap<usize, (usize, usize, usize)> = BTreeMap::new();
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        for s in &self.source.shapes {
            let e = per.entry(s.layer).or_default();
            e.0 += 1;
            let mut geom = s.geom.clone();
            if let Some(r) = as_rect(&geom) {
                geom = Geometry::Rect(r);
            }
            canonical_points(&mut geom);
            if !matches!(geom, Geometry::Label { .. }) && !seen.insert((s.layer, format!("{geom:?}"))) {
                e.1 += 1;
            }
            let pts: Vec<(i64, i64)> = match &geom {
                Geometry::Rect(r) => vec![(r.x0, r.y0), (r.x1, r.y1)],
                Geometry::Polygon { points } | Geometry::Path { points, .. } => points.clone(),
                Geometry::Label { .. } => vec![],
            };
            if pts.iter().any(|p| p.0 % g != 0 || p.1 % g != 0) {
                e.2 += 1;
            }
        }
        for (&l, &(n, dup, off)) in &per {
            if tech.layer(l).is_none() {
                push(Level::Warning, format!("{n} shape(s) on layer #{l}, which the technology does not define"));
            }
            if dup > 0 {
                push(
                    Level::Warning,
                    format!(
                        "{}: {dup} shape(s) drawn twice at the same place (a pin or label datatype read onto the \
                         drawing layer?); repeats and stretches copy them too",
                        name(l)
                    ),
                );
            }
            if off > 0 {
                push(
                    Level::Warning,
                    format!("{}: {off} shape(s) with vertices off the {} grid", name(l), fmt_um(g, tech.dbu_per_um)),
                );
            }
        }
        for via in &tech.vias {
            let Some(cut) = tech.layer_index(&via.cut) else { continue };
            let odd = self
                .source
                .shapes
                .iter()
                .filter(|s| {
                    s.layer == cut && as_rect(&s.geom).is_none_or(|r| r.width() != via.cut_size || r.height() != via.cut_size)
                })
                .count();
            if odd > 0 {
                push(
                    Level::Info,
                    format!(
                        "{}: {odd} cut(s) are not {} µm squares; Contacts to fills leaves them, and a stretch across them \
                         widens them",
                        via.cut,
                        fmt_um(via.cut_size, tech.dbu_per_um)
                    ),
                );
            }
        }
        if !self.source.instances.is_empty() {
            push(
                Level::Info,
                format!(
                    "{} placed cell(s) in the drawing move as one item, only with operations on all layers, and their \
                     contacts do not become fills: Flatten instances turns them into shapes",
                    self.source.instances.len()
                ),
            );
        }
        out
    }

    /// Replace the placed cells of the drawing by their (flattened) shapes. Returns how many
    /// instances were flattened.
    pub fn flatten_instances(&mut self, src: &dyn crate::hier::LayoutSource) -> usize {
        let n = self.source.instances.len();
        if n == 0 {
            return 0;
        }
        let only = Layout { instances: std::mem::take(&mut self.source.instances), ..Layout::default() };
        for s in crate::hier::flatten(&only, src, 64) {
            self.source.add(s.layer, s.geom);
        }
        n
    }
}

// ---------------------------------------------------------------------------------------------
// Corner check

/// One parameter set of the corner check.
#[derive(Debug, Clone)]
pub struct CornerRun {
    pub label: String,
    pub values: Vec<(String, f64)>,
    pub result: Result<CornerResult, String>,
}

#[derive(Debug, Clone)]
pub struct CornerResult {
    pub layout: Layout,
    pub violations: Vec<Violation>,
}

impl Template {
    /// Parameter sets to verify: the defaults, each parameter alone at its minimum and
    /// maximum (booleans toggled) and all at their minimum / maximum together.
    pub fn corners(&self) -> Vec<(String, Vec<(String, f64)>)> {
        let mut out = vec![("defaults".to_string(), vec![])];
        let (mut lo_all, mut hi_all) = (vec![], vec![]);
        for p in &self.params {
            match p.kind {
                ParamKind::Bool => {
                    let v = if p.default != 0.0 { 0.0 } else { 1.0 };
                    out.push((format!("{}={}", p.name, if v != 0.0 { "on" } else { "off" }), vec![(p.name.clone(), v)]));
                }
                _ => {
                    for (tag, v, all) in [("min", p.min, &mut lo_all), ("max", p.max, &mut hi_all)] {
                        if let Some(v) = v.filter(|v| *v != p.default) {
                            out.push((format!("{} {tag} ({})", p.name, fmt_num(v)), vec![(p.name.clone(), v)]));
                            all.push((p.name.clone(), v));
                        }
                    }
                }
            }
        }
        for (tag, all) in [("all min", lo_all), ("all max", hi_all)] {
            if all.len() >= 2 {
                out.push((tag.to_string(), all));
            }
        }
        out
    }

    /// Generate every corner and run `deck` on it.
    pub fn check_corners(&self, tech: &Tech, deck: &Deck, src: &dyn LayoutSource) -> Vec<CornerRun> {
        self.corners()
            .into_iter()
            .map(|(label, values)| {
                let result = self.generate(tech, &values).map(|layout| {
                    let rep = drc::run(deck, &layout, src, tech);
                    CornerResult { layout, violations: rep.violations }
                });
                CornerRun { label, values, result }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Suggestions

impl Template {
    /// Replace each cluster of via cuts in the source by a [`Op::ContactFill`] of its extent.
    /// Returns the number of fills made.
    pub fn suggest_contact_fills(&mut self, tech: &Tech) -> usize {
        let mut made = 0;
        for via in &tech.vias {
            let Some(cut) = tech.layer_index(&via.cut) else { continue };
            let idx: Vec<usize> = (0..self.source.shapes.len())
                .filter(|&i| {
                    let s = &self.source.shapes[i];
                    s.layer == cut && as_rect(&s.geom).is_some_and(|r| r.width() == via.cut_size && r.height() == via.cut_size)
                })
                .collect();
            if idx.is_empty() {
                continue;
            }
            // Union-find over cuts at most a cut spacing (plus a grid step) apart.
            let mut parent: Vec<usize> = (0..idx.len()).collect();
            fn find(p: &mut [usize], i: usize) -> usize {
                let mut r = i;
                while p[r] != r {
                    r = p[r];
                }
                p[i] = r;
                r
            }
            let reach = via.cut_spacing + tech.grid.max(1);
            for a in 0..idx.len() {
                for b in a + 1..idx.len() {
                    let (ra, rb) = (self.source.shapes[idx[a]].bbox(), self.source.shapes[idx[b]].bbox());
                    let gx = (ra.x0.max(rb.x0) - ra.x1.min(rb.x1)).max(0);
                    let gy = (ra.y0.max(rb.y0) - ra.y1.min(rb.y1)).max(0);
                    if gx <= reach && gy <= reach && (gx == 0 || gy == 0) {
                        let (x, y) = (find(&mut parent, a), find(&mut parent, b));
                        parent[x] = y;
                    }
                }
            }
            let mut groups: BTreeMap<usize, IRect> = BTreeMap::new();
            for (a, &i) in idx.iter().enumerate() {
                let r = find(&mut parent, a);
                let b = self.source.shapes[i].bbox();
                groups.entry(r).and_modify(|g| *g = g.union(&b)).or_insert(b);
            }
            let ids: BTreeSet<u64> = idx.iter().map(|&i| self.source.shapes[i].id).collect();
            self.source.shapes.retain(|s| !ids.contains(&s.id));
            let top = tech.layer_index(&via.top);
            for b in groups.into_values() {
                let rect = self.fill_area(b, top, via.enc_bottom.max(via.enc_top));
                self.ops.push(Op::ContactFill { rect, via: via.name.clone() });
                made += 1;
            }
        }
        made
    }

    /// The area cuts `b` may fill: the smallest top-layer plate containing them less the
    /// larger enclosure (as a via array is drawn); `b` itself when no plate encloses it.
    fn fill_area(&self, b: IRect, top: Option<usize>, enc: i64) -> IRect {
        let Some(top) = top else { return b };
        self.source
            .shapes
            .iter()
            .filter(|s| s.layer == top)
            .flat_map(|s| rects_of(&s.geom))
            .map(|r| IRect::new(r.x0 + enc, r.y0 + enc, r.x1 - enc, r.y1 - enc))
            .filter(|r| inside(r, &b))
            .min_by_key(|r| r.width() as i128 * r.height() as i128)
            .unwrap_or(b)
    }

    fn gates(&self, tech: &Tech) -> Vec<IRect> {
        let Some(poly) = tech.poly() else { return vec![] };
        let diff = tech.diffusion_layers();
        let actives: Vec<IRect> =
            self.source.shapes.iter().filter(|s| diff.contains(&s.layer)).flat_map(|s| rects_of(&s.geom)).collect();
        let mut g: Vec<IRect> = self
            .source
            .shapes
            .iter()
            .filter(|s| s.layer == poly)
            .flat_map(|s| rects_of(&s.geom))
            .filter(|r| actives.iter().any(|a| r.x0 < a.x1 && a.x0 < r.x1 && r.y0 < a.y1 && a.y0 < r.y1))
            .collect();
        g.sort_by_key(|r| (r.x0, r.y0));
        g
    }

    /// Detect equally spaced identical gates and turn them into a finger repeat: the drawing
    /// is reduced to as few units (gate and the contact column after it) as the shapes that
    /// span the fingers allow, a parameter `nf` (default: the drawn count) repeats the last
    /// one, and the spanning shapes follow. Generation at the defaults reproduces the drawing.
    /// Run contact fills first.
    pub fn suggest_fingers(&mut self, tech: &Tech) -> Result<String, String> {
        if self.ops.iter().any(|o| matches!(o, Op::Stretch { .. } | Op::Repeat { .. })) {
            return Err("Detect fingers before adding stretch lines or repeats".into());
        }
        if self.param("nf").is_some() {
            return Err("There already is a parameter 'nf'".into());
        }
        let gates = self.gates(tech);
        let first = *gates.first().ok_or_else(|| no_gate(tech))?;
        let row: Vec<IRect> = gates
            .iter()
            .copied()
            .filter(|r| r.y0 == first.y0 && r.width() == first.width() && r.height() == first.height())
            .collect();
        let n = row.len() as i64;
        if n < 2 {
            return Err("Only one gate: draw at least two fingers to detect the pitch".into());
        }
        let p = row[1].x0 - row[0].x0;
        if row.windows(2).any(|w| w[1].x0 - w[0].x0 != p) {
            return Err("The gates are not equally spaced".into());
        }
        let mut last = String::from("No repeating unit found");
        for keep in 1..n {
            match self.reduce_fingers(tech, n, p, keep) {
                Ok(()) => {
                    let dbu = tech.dbu_per_um;
                    let kept = if keep == 1 {
                        "kept one unit".to_string()
                    } else {
                        format!("kept {keep} units (shapes spanning the fingers need them), so nf ≥ {keep}")
                    };
                    return Ok(format!("{n} fingers at {} µm pitch: {kept}; nf repeats the last one", fmt_um(p, dbu)));
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// [`Self::suggest_fingers`] keeping `keep` of the `n` drawn units, pitch `p`.
    fn reduce_fingers(&mut self, tech: &Tech, n: i64, p: i64, keep: i64) -> Result<(), String> {
        let mut items = self.items();
        for op in &self.ops {
            if let Op::ContactFill { rect, via } = op {
                items.push(Item { layer: None, geom: Geometry::Rect(*rect), inst: None, fill: Some(via.clone()), home: *rect });
            }
        }
        let at_key: HashMap<String, usize> = items.iter().enumerate().map(|(i, it)| (it.key((0, 0)), i)).collect();
        let next: Vec<Option<usize>> = items.iter().map(|it| at_key.get(&it.key((p, 0))).copied()).collect();
        let mut has_prev = vec![false; items.len()];
        for j in next.iter().flatten() {
            has_prev[*j] = true;
        }
        let (mut member, mut removed) = (vec![false; items.len()], vec![false; items.len()]);
        for start in (0..items.len()).filter(|&i| !has_prev[i]) {
            let mut chain = vec![start];
            while let Some(j) = next[*chain.last().unwrap()] {
                if chain.contains(&j) {
                    break;
                }
                chain.push(j);
            }
            let len = chain.len() as i64;
            if len >= n {
                // The last n occurrences are the units; keep the first `keep`, repeat the last kept.
                let m = (len - n + keep - 1) as usize;
                member[chain[m]] = true;
                for &j in &chain[m + 1..] {
                    removed[j] = true;
                }
            }
        }
        let region = (0..items.len())
            .filter(|&i| member[i])
            .map(|i| items[i].bbox())
            .reduce(|a, b| a.union(&b))
            .ok_or("No repeating unit found")?;
        let ext = (n - keep) * p;
        // Where spanning items stretch: the middle of the unit, or else any position whose
        // (at, at + ext] interval holds no edge of a spanning item (labels may sit anywhere).
        let rest: Vec<usize> = (0..items.len()).filter(|&i| !member[i] && !removed[i]).collect();
        let is_label = |i: usize| matches!(items[i].geom, Geometry::Label { .. });
        let coords: Vec<i64> = rest.iter().filter(|&&i| !is_label(i)).flat_map(|&i| xs(&items[i].geom)).collect();
        let clear = |at: i64| !coords.iter().any(|&c| c > at && c <= at + ext);
        let mid = snap((region.x0 + region.x1) / 2, tech.grid);
        let at = if clear(mid) {
            None
        } else {
            let mut cand: Vec<i64> = coords.iter().copied().chain([region.x0, region.x1]).filter(|&c| c <= region.x1).collect();
            cand.sort_unstable_by_key(|&c| ((c - mid).abs(), c));
            Some(
                cand.into_iter().find(|&c| clear(c)).ok_or(
                    "Shapes that span the fingers have edges between the gates, so the fingers cannot be cut out cleanly",
                )?,
            )
        };
        let line = at.unwrap_or(mid);
        let f = move |c: i64| {
            if c > line + ext {
                c - ext
            } else if c > line {
                line
            } else {
                c
            }
        };
        let before = self.clone();
        for &i in &rest {
            items[i].map(Axis::X, &f);
        }
        let (mut shapes, mut insts, mut fills) = (vec![], vec![], vec![]);
        let (n_src, n_inst) = (self.source.shapes.len(), self.source.instances.len());
        for (i, it) in items.into_iter().enumerate() {
            if removed[i] {
                continue;
            }
            if i < n_src {
                let mut s = self.source.shapes[i].clone();
                s.geom = it.geom;
                shapes.push(s);
            } else if i < n_src + n_inst {
                let mut inst = self.source.instances[i - n_src].clone();
                let o = it.geom.bbox();
                (inst.trans.x, inst.trans.y) = (o.x0, o.y0);
                insts.push(inst);
            } else if let (Some(via), Geometry::Rect(rect)) = (it.fill, it.geom) {
                fills.push(Op::ContactFill { rect, via });
            }
        }
        self.source.shapes = shapes;
        self.source.instances = insts;
        self.ops.retain(|o| !matches!(o, Op::ContactFill { .. }));
        self.ops.extend(fills);
        let mut nf = Param::new("nf", ParamKind::Int, n as f64);
        nf.min = Some(keep as f64);
        nf.max = Some((n * 4).max(16) as f64);
        nf.description = "Number of fingers".into();
        self.params.push(nf);
        let count = if keep == 1 { "nf".to_string() } else { format!("nf - {}", keep - 1) };
        self.ops.push(Op::Repeat { axis: Axis::X, region, count, pitch: fmt_um(p, tech.dbu_per_um), at, layers: vec![] });
        // The reduced template must give back the drawing (labels may move along their strap).
        let shapes_only = |l: &Layout| canonical(l).into_iter().filter(|s| !s.contains("Label")).collect::<Vec<_>>();
        let ok = match (before.generate(tech, &[]), self.generate(tech, &[])) {
            (Ok(a), Ok(b)) => shapes_only(&a) == shapes_only(&b),
            _ => false,
        };
        if !ok {
            *self = before;
            return Err("The reduced unit does not reproduce the drawing; draw the fingers identically".into());
        }
        Ok(())
    }

    /// A horizontal stretch line through the middle of the largest diffusion, driven by a new
    /// length parameter `w` (the drawn width).
    pub fn suggest_width(&mut self, tech: &Tech) -> Result<String, String> {
        if self.param("w").is_some() {
            return Err("There already is a parameter 'w'".into());
        }
        let diff = tech.diffusion_layers();
        let a = self
            .source
            .shapes
            .iter()
            .filter(|s| diff.contains(&s.layer))
            .flat_map(|s| rects_of(&s.geom))
            .max_by_key(|r| r.width() as i128 * r.height() as i128)
            .ok_or_else(|| {
                format!("No diffusion found: no shape is on the diffusion layer(s) {}. {ROLES_HINT}", diffusion_names(tech))
            })?;
        let dbu = tech.dbu_per_um;
        let at = snap((a.y0 + a.y1) / 2, tech.grid);
        let mut w = Param::new("w", ParamKind::Length, a.height() as f64 / dbu as f64);
        let lmin = diff.first().and_then(|&l| tech.layer(l)).map_or(0, |l| l.min_width).max(tech.grid);
        w.min = Some(lmin as f64 / dbu as f64);
        w.max = Some(w.default * 10.0);
        w.description = "Width (diffusion height)".into();
        self.params.push(w);
        self.ops.insert(
            self.first_stretch_slot(),
            Op::Stretch { axis: Axis::Y, at, side: Side::High, amount: "w".into(), layers: vec![] },
        );
        self.tighten_min(tech, "w");
        Ok(format!("w stretches the diffusion from {} µm, line at y = {}", fmt_um(a.height(), dbu), fmt_um(at, dbu)))
    }

    /// A vertical stretch line through the first gate, driven by a new length parameter `l`
    /// (the drawn gate length). A finger repeat's pitch follows.
    pub fn suggest_length(&mut self, tech: &Tech) -> Result<String, String> {
        if self.param("l").is_some() {
            return Err("There already is a parameter 'l'".into());
        }
        let g = *self.gates(tech).first().ok_or_else(|| no_gate(tech))?;
        let dbu = tech.dbu_per_um;
        let l0 = g.width();
        let at = snap((g.x0 + g.x1) / 2, tech.grid);
        let mut l = Param::new("l", ParamKind::Length, l0 as f64 / dbu as f64);
        let pmin = tech.poly().and_then(|p| tech.layer(p)).map_or(0, |d| d.min_width).max(tech.grid);
        l.min = Some(pmin as f64 / dbu as f64);
        l.max = Some(l.default * 20.0);
        l.description = "Gate length".into();
        self.params.push(l);
        let mut note = String::new();
        for op in &mut self.ops {
            if let Op::Repeat { axis: Axis::X, pitch, .. } = op
                && let Ok(Expr::Num(p)) = parse_expr(pitch)
            {
                *pitch = format!("{} + l", fmt_num(p - l0 as f64 / dbu as f64));
                note = ", the finger pitch follows".into();
            }
        }
        self.ops.insert(
            self.first_stretch_slot(),
            Op::Stretch { axis: Axis::X, at, side: Side::High, amount: "l".into(), layers: vec![] },
        );
        self.tighten_min(tech, "l");
        Ok(format!("l stretches the gate from {} µm{note}", fmt_um(l0, dbu)))
    }

    /// Raise the minimum of length parameter `name` to the smallest grid value (between its
    /// minimum and default) at which the cell still generates.
    fn tighten_min(&mut self, tech: &Tech, name: &str) {
        let Some(p) = self.param(name).cloned() else { return };
        let step = tech.grid.max(1) as f64 / tech.dbu_per_um.max(1) as f64;
        let (mut lo, mut hi) = (((p.min.unwrap_or(step)) / step).round() as i64, (p.default / step).round() as i64);
        let ok = |t: &Template, k: i64| t.generate(tech, &[(name.to_string(), k as f64 * step)]).is_ok();
        if lo >= hi || ok(self, lo) {
            return;
        }
        // Invariant: lo fails, hi works.
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if ok(self, mid) { hi = mid } else { lo = mid }
        }
        if let Some(q) = self.params.iter_mut().find(|q| q.name == name) {
            q.min = Some(hi as f64 * step);
        }
    }

    /// Index after the last stretch (stretches keep their drawing order).
    fn first_stretch_slot(&self) -> usize {
        self.ops.iter().rposition(|o| matches!(o, Op::Stretch { .. })).map_or(0, |k| k + 1)
    }
}

fn xs(g: &Geometry) -> Vec<i64> {
    match g {
        Geometry::Rect(r) => vec![r.x0, r.x1],
        Geometry::Polygon { points } | Geometry::Path { points, .. } => points.iter().map(|p| p.0).collect(),
        Geometry::Label { at, .. } => vec![at.0],
    }
}

/// Order-independent text of a layout's geometry (labels without text), for comparisons.
pub fn canonical(l: &Layout) -> Vec<String> {
    let mut v: Vec<String> = l
        .shapes
        .iter()
        .map(|s| {
            let mut g = s.geom.clone();
            if let Geometry::Label { text, .. } = &mut g {
                text.clear();
            }
            format!("{}|{g:?}", s.layer)
        })
        .chain(l.instances.iter().map(|i| format!("{}|{:?}|{:?}", i.cell, i.trans, i.array)))
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------------------------
// SKILL export

/// A float literal SKILL reads back exactly (always with a decimal point, never an exponent).
fn skill_num(v: f64) -> String {
    let s = format!("{v}");
    let s = if s.contains('.') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") };
    if v < 0.0 { format!("({s})") } else { s }
}

/// An integer literal; negative ones in parentheses, as `list(1 -2)` reads as `list(1 - 2)`.
fn skill_int(v: i64) -> String {
    if v < 0 { format!("({v})") } else { v.to_string() }
}

fn skill_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The expression as SKILL computing a float, with the semantics of [`Expr::eval`].
pub fn skill_expr(e: &Expr) -> String {
    let b = |c: String| format!("if({c} 1.0 0.0)");
    let nz = |e: &Expr| format!("({} != 0.0)", skill_expr(e));
    match e {
        Expr::Num(v) => skill_num(*v),
        Expr::Var(n) => format!("float({n})"),
        Expr::Neg(x) => format!("(-{})", skill_expr(x)),
        Expr::Not(x) => b(format!("{} == 0.0", skill_expr(x))),
        Expr::Bin(op, l, r) => {
            let (a, c) = (skill_expr(l), skill_expr(r));
            match op {
                '+' | '-' | '*' | '/' => format!("({a} {op} {c})"),
                // Remainder truncating toward zero, as Rust's `%` on floats.
                '%' => format!("let((icsA icsB) icsA = {a} icsB = {c} icsA - icsB * float(fix(icsA / icsB)))"),
                '|' => b(format!("{} || {}", nz(l), nz(r))),
                '&' => b(format!("{} && {}", nz(l), nz(r))),
                '=' => b(format!("abs({a} - {c}) < 0.000000001")),
                '~' => b(format!("abs({a} - {c}) >= 0.000000001")),
                'l' => b(format!("{a} <= {c} + 0.000000001")),
                'g' => b(format!("{a} + 0.000000001 >= {c}")),
                '<' => b(format!("{a} < {c} - 0.000000001")),
                _ => b(format!("{a} > {c} + 0.000000001")),
            }
        }
        Expr::Call(f, args) => {
            let a: Vec<String> = args.iter().map(skill_expr).collect();
            match f.as_str() {
                "min" | "max" | "abs" | "sqrt" => format!("{f}({})", a.join(" ")),
                "floor" => format!("float(floor({}))", a.join(" ")),
                "ceil" => format!("float(ceiling({}))", a.join(" ")),
                "round" => format!("float(round({}))", a.join(" ")),
                _ => format!("if({} != 0.0 {} {})", a[0], a.get(1).map_or("0.0", |s| s), a.get(2).map_or("0.0", |s| s)),
            }
        }
    }
}

fn skill_of(text: &str, what: &str) -> Result<String, String> {
    parse_expr(text).map(|e| skill_expr(&e)).map_err(|e| format!("{what} '{text}': {e}"))
}

const SKILL_ENGINE: &str = r#"    icsSnap = lambda((v) if(icsGrid <= 1 v icsGrid * floor((v + fix(icsGrid / 2.0)) / float(icsGrid))))
    icsUm = lambda((u) funcall(icsSnap round(u * icsDbu)))
    icsOn = lambda((oIt oLs) if(oLs then and(nth(1 oIt) member(nth(1 oIt) oLs) t) else !and(nth(1 oIt) member(nth(1 oIt) icsFixed))))
    icsBox = lambda((bIt) let((bK bP bXs bYs bW)
      bK = car(bIt)  bP = nth(2 bIt)
      bXs = mapcar(lambda((bQ) car(bQ)) bP)  bYs = mapcar(lambda((bQ) cadr(bQ)) bP)
      cond(
        (eq(bK 'label)
          bW = max(strlen(car(nth(3 bIt))) 1) * cadr(nth(3 bIt)) * 6
          bW = fix(bW / 10.0)
          list(car(bXs) car(bYs) car(bXs) + bW car(bYs) + cadr(nth(3 bIt))))
        (eq(bK 'path)
          bW = fix(nth(3 bIt) / 2.0)
          list(apply('min bXs) - bW apply('min bYs) - bW apply('max bXs) + bW apply('max bYs) + bW))
        (eq(bK 'inst) list(car(bXs) car(bYs) car(bXs) car(bYs)))
        (eq(bK 'poly) list(apply('min bXs) apply('min bYs) apply('max bXs) apply('max bYs)))
        (t list(car(bXs) car(bYs) cadr(bXs) cadr(bYs))))))
    icsInside = lambda((iR iB) and(nth(0 iB) >= nth(0 iR) nth(1 iB) >= nth(1 iR) nth(2 iB) <= nth(2 iR) nth(3 iB) <= nth(3 iR)))
    icsMap = lambda((mIt mAx mF) list(car(mIt) nth(1 mIt)
      mapcar(lambda((mQ) if(eq(mAx 'x) list(funcall(mF car(mQ)) cadr(mQ)) list(car(mQ) funcall(mF cadr(mQ))))) nth(2 mIt))
      nth(3 mIt) nth(4 mIt)))
    icsMove = lambda((vIt vDx vDy) list(car(vIt) nth(1 vIt)
      mapcar(lambda((vQ) list(car(vQ) + vDx cadr(vQ) + vDy)) nth(2 vIt)) nth(3 vIt) nth(4 vIt)))
    icsPt = lambda((pQ) list(car(pQ) / icsDbu cadr(pQ) / icsDbu))
    icsAl = lambda((aQ aX) if(eq(aX 'x) car(aQ) cadr(aQ)))
    icsOt = lambda((aQ aX) if(eq(aX 'x) cadr(aQ) car(aQ)))
    icsMk = lambda((kA kO kX) if(eq(kX 'x) list(kA kO) list(kO kA)))
    icsSg = lambda((gC) if(gC > icsAt then 'hi else if(gC < icsAt then 'lo else 'on)))
    icsPush = lambda((uL uQ) if(and(uL equal(car(uL) uQ)) uL cons(uQ uL)))
    ;; Stretch an item with icsSh (line icsAt, sides icsHi / icsLo). A polygon or path edge
    ;; crossing the line at an angle keeps it: it is cut where each side's kept part ends and
    ;; joined along the axis. icsGone is set when a polygon edge along the axis collapses.
    icsSt = lambda((tIt tX) if(memq(car(tIt) '(poly path)) then
      let((tC tQ tA tO tSa tSb tV tW tT tI tF)
        tC = eq(car(tIt) 'poly)
        tQ = nth(2 tIt)
        when(tC tQ = append(tQ list(car(tQ))))
        tA = car(tQ)
        tO = nil
        foreach(tB cdr(tQ)
          tSa = funcall(icsAl tA tX)  tSb = funcall(icsAl tB tX)
          tV = funcall(icsOt tA tX)  tW = funcall(icsOt tB tX)
          icsCut = if(abs(tW - tV) == abs(tSb - tSa) lambda((cV) cV) icsSnap)
          tO = funcall(icsPush tO funcall(icsMk funcall(icsSh tSa) tV tX))
          when(and(tC tV == tW tSa != tSb funcall(icsSh tSa) == funcall(icsSh tSb)) icsGone = t)
          when(and(tV != tW !eq(funcall(icsSg tSa) funcall(icsSg tSb)))
            foreach(tE list(funcall(icsSg tSa) funcall(icsSg tSb))
              unless(eq(tE 'on)
                tT = if(eq(tE 'lo) icsAt + min(icsLo 0) icsAt - min(icsHi 0))
                tI = if(eq(tE 'lo) tT - icsLo tT + icsHi)
                tF = max(0.0 min(1.0 float(tT - tSa) / float(tSb - tSa)))
                tO = funcall(icsPush tO funcall(icsMk tI funcall(icsCut tV + round(float(tW - tV) * tF)) tX)))))
          tA = tB)
        unless(tC tO = funcall(icsPush tO funcall(icsMk funcall(icsSh funcall(icsAl tA tX)) funcall(icsOt tA tX) tX)))
        when(and(tC cdr(tO) equal(car(tO) car(last(tO)))) tO = cdr(tO))
        list(car(tIt) nth(1 tIt) reverse(tO) nth(3 tIt) nth(4 tIt)))
      else funcall(icsMap tIt tX icsSh)))
"#;

const SKILL_OUTPUT: &str = r#"    ;; Create the shapes; contact fills become the largest centred array of cuts.
    foreach(oIt icsItems
      let((oP oL oX)
        oP = nth(2 oIt)  oL = nth(1 oIt)  oX = nth(3 oIt)
        cond(
          (eq(car(oIt) 'rect) dbCreateRect(pcCellView oL list(funcall(icsPt car(oP)) funcall(icsPt cadr(oP)))))
          (eq(car(oIt) 'poly) dbCreatePolygon(pcCellView oL mapcar(icsPt oP)))
          (eq(car(oIt) 'path) dbCreatePath(pcCellView oL mapcar(icsPt oP) oX / icsDbu))
          (eq(car(oIt) 'label)
            dbCreateLabel(pcCellView oL funcall(icsPt car(oP)) car(oX) "lowerLeft" "R0" "roman" cadr(oX) / icsDbu))
          (eq(car(oIt) 'inst)
            dbCreateInstByMasterName(pcCellView car(oX) cadr(oX) "layout" nil funcall(icsPt car(oP)) caddr(oX)))
          (eq(car(oIt) 'fill)
            let((fC fS fX0 fY0 fNx fNy fOx fOy fL fX fY)
              fC = car(oX)  fS = cadr(oX)
              fX0 = car(car(oP))  fY0 = cadr(car(oP))
              fL = car(cadr(oP)) - fX0
              if(fL < fC then fNx = 0 fOx = 0 else
                fNx = fix((fL - fC) / float(fC + fS)) + 1
                fOx = fix((fL - (fNx * fC + (fNx - 1) * fS)) / 2.0)
                when(icsGrid > 1 fOx = fix(fOx / float(icsGrid)) * icsGrid))
              fL = cadr(cadr(oP)) - fY0
              if(fL < fC then fNy = 0 fOy = 0 else
                fNy = fix((fL - fC) / float(fC + fS)) + 1
                fOy = fix((fL - (fNy * fC + (fNy - 1) * fS)) / 2.0)
                when(icsGrid > 1 fOy = fix(fOy / float(icsGrid)) * icsGrid))
              when(fNx == 0 || fNy == 0
                error("%s: a contact fill is too small for one %s cut" icsName oL))
              for(fJ 0 fNy - 1
                for(fI 0 fNx - 1
                  fX = fX0 + fOx + fI * (fC + fS)
                  fY = fY0 + fOy + fJ * (fC + fS)
                  dbCreateRect(pcCellView oL list(funcall(icsPt list(fX fY)) funcall(icsPt list(fX + fC fY + fC)))))))))))
"#;

impl Template {
    /// The PCell as a SKILL `pcDefinePCell` for Cadence Virtuoso (library `lib`), generating
    /// the same layout as [`Template::generate`]: the drawing is data (database units) and a
    /// small engine in the body runs the operations. Layer names are the technology's; flag
    /// parameters are integers 0 / 1. Returns the text and warnings.
    pub fn to_skill(&self, tech: &Tech, lib: &str) -> Result<(String, Vec<String>), String> {
        let mut warnings = vec![];
        let lname = |l: usize| tech.layer(l).map(|d| d.name.clone()).ok_or_else(|| format!("no layer {l}"));
        let defaults = self.defaults();
        let dbu = tech.dbu_per_um.max(1);
        let mut s = String::new();
        let _ = writeln!(s, ";; {}: PCell exported from the IC Studio PCell Designer", self.name);
        if !self.description.is_empty() {
            let _ = writeln!(s, ";; {}", self.description);
        }
        let _ = writeln!(
            s,
            ";; Technology {}: layer names as in IC Studio (map them if Virtuoso names differ).\n\
             ;; The drawing below is in database units of 1/{dbu} um; shapes are made in user units (um).\n\
             ;; Parameters: lengths in um, counts as integers, switches as 0 / 1.",
            tech.name
        );
        let _ = writeln!(s, "pcDefinePCell(\n  list(ddGetObj({}) {} \"layout\")\n  (", skill_str(lib), skill_str(&self.name));
        for p in &self.params {
            let v = match p.kind {
                ParamKind::Length => skill_num(p.default),
                _ => skill_int(p.default.round() as i64),
            };
            let note = if p.description.is_empty() { String::new() } else { format!("  ; {}", p.description) };
            let _ = writeln!(s, "    ({} {v}){note}", p.name);
        }
        s.push_str("  )\n");
        // Locals: engine helpers, the per-operation lines / regions and the working values.
        let mut locals: Vec<String> = [
            "icsName",
            "icsDbu",
            "icsGrid",
            "icsFixed",
            "icsItems",
            "icsSnap",
            "icsUm",
            "icsOn",
            "icsBox",
            "icsInside",
            "icsMap",
            "icsMove",
            "icsPt",
            "icsD",
            "icsHi",
            "icsLo",
            "icsAt",
            "icsSh",
            "icsCross",
            "icsN",
            "icsP",
            "icsExt",
            "icsReg",
            "icsMem",
            "icsRest",
            "icsAl",
            "icsOt",
            "icsMk",
            "icsSg",
            "icsPush",
            "icsSt",
            "icsGone",
            "icsCut",
        ]
        .map(String::from)
        .to_vec();
        for (k, op) in self.ops.iter().enumerate() {
            match op {
                Op::Stretch { .. } => locals.push(format!("icsA{k}")),
                Op::Repeat { .. } => locals.extend([format!("icsR{k}"), format!("icsRa{k}")]),
                _ => {}
            }
        }
        let _ = writeln!(s, "  let(({})", locals.join(" "));
        let _ = writeln!(
            s,
            "    icsName = {}\n    icsDbu = {}\n    icsGrid = {}",
            skill_str(&self.name),
            skill_num(dbu as f64),
            tech.grid
        );
        s.push_str(SKILL_ENGINE);
        // The drawing: list(kind layer points extra home).
        s.push_str("    ;; The drawing: list(kind layer points extra)\n    icsItems = list(\n");
        let pts = |p: &[(i64, i64)]| {
            p.iter().map(|(x, y)| format!("list({} {})", skill_int(*x), skill_int(*y))).collect::<Vec<_>>().join(" ")
        };
        for (layer, geom) in self.generation_shapes(tech.grid) {
            let l = skill_str(&lname(layer)?);
            let line = match &geom {
                Geometry::Rect(r) => format!("list('rect {l} list({}) nil nil)", pts(&[(r.x0, r.y0), (r.x1, r.y1)])),
                Geometry::Polygon { points } => format!("list('poly {l} list({}) nil nil)", pts(points)),
                Geometry::Path { points, width } => format!("list('path {l} list({}) {width} nil)", pts(points)),
                Geometry::Label { text, at, size } => {
                    format!("list('label {l} list({}) list({} {size}) nil)", pts(&[*at]), skill_str(text))
                }
            };
            let _ = writeln!(s, "      {line}");
        }
        for i in &self.source.instances {
            if i.array.is_some() {
                warnings.push(format!("instance of {} is an array: exported as its first element", i.cell));
            }
            let orient = match (i.trans.rot % 4, i.trans.mirror) {
                (0, false) => "R0",
                (1, false) => "R90",
                (2, false) => "R180",
                (3, false) => "R270",
                (0, true) => "MX",
                (1, true) => "MXR90",
                (2, true) => "MY",
                _ => "MYR90",
            };
            let _ = writeln!(
                s,
                "      list('inst nil list({}) list({} {} {}) nil)",
                pts(&[(i.trans.x, i.trans.y)]),
                skill_str(&i.cell.lib),
                skill_str(&i.cell.cell),
                skill_str(orient)
            );
        }
        for op in &self.ops {
            if let Op::ContactFill { rect, via } = op {
                let def = tech.via(via).ok_or_else(|| format!("contact fill: technology has no via '{via}'"))?;
                let _ = writeln!(
                    s,
                    "      list('fill {} list({}) list({} {}) nil)",
                    skill_str(&def.cut),
                    pts(&[(rect.x0, rect.y0), (rect.x1, rect.y1)]),
                    def.cut_size,
                    def.cut_spacing
                );
            }
        }
        s.push_str("    )\n");
        let layers = |ls: &[usize]| -> Result<String, String> {
            if ls.is_empty() {
                return Ok("nil".into());
            }
            let names: Vec<String> = ls.iter().map(|&l| lname(l).map(|n| skill_str(&n))).collect::<Result<_, _>>()?;
            Ok(format!("list({})", names.join(" ")))
        };
        let _ = writeln!(s, "    icsFixed = {}", layers(&self.fixed)?);
        let rect = |r: &IRect| format!("list({} {} {} {})", skill_int(r.x0), skill_int(r.y0), skill_int(r.x1), skill_int(r.y1));
        for (k, op) in self.ops.iter().enumerate() {
            match op {
                Op::Stretch { at, .. } => {
                    let _ = writeln!(s, "    icsA{k} = {}", skill_int(*at));
                }
                Op::Repeat { region, at, .. } => {
                    let _ = writeln!(s, "    icsR{k} = {}\n    icsRa{k} = {}", rect(region), at.map_or("nil".into(), skill_int));
                }
                _ => {}
            }
        }
        for (k, op) in self.ops.iter().enumerate() {
            if let Op::Condition { region, when, layers: ls } = op {
                let c = skill_of(when, &format!("condition {}", k + 1))?;
                let _ = writeln!(
                    s,
                    "    ;; {}. keep if {when}\n    when({c} == 0.0\n      icsItems = setof(cIt icsItems !and(funcall(icsOn cIt {}) funcall(icsInside {} funcall(icsBox cIt)))))",
                    k + 1,
                    layers(ls)?,
                    rect(region)
                );
            }
        }
        for (k, op) in self.ops.iter().enumerate() {
            let Op::Stretch { axis, side, amount, layers: ls, .. } = op else { continue };
            let what = format!("stretch {}", k + 1);
            let e = parse_expr(amount).map_err(|e| format!("{what} '{amount}': {e}"))?;
            let d0 = e.eval(&defaults).map_err(|e| format!("{what} '{amount}': {e}"))?;
            let ax = axis.label();
            let (hi, lo) = match side {
                Side::High => ("icsD", "0"),
                Side::Low => ("0", "icsD"),
                Side::Both => ("funcall(icsSnap fix(icsD / 2.0))", "icsD - icsHi"),
            };
            let _ = writeln!(s, "    ;; {}. stretch along {ax} by {amount}", k + 1);
            let _ = writeln!(s, "    icsD = funcall(icsUm {} - {})", skill_expr(&e), skill_num(d0));
            let _ = writeln!(
                s,
                "    unless(icsD == 0\n      icsHi = {hi}\n      icsLo = {lo}\n      icsAt = icsA{k}\n      icsCross = nil\n      icsGone = nil"
            );
            s.push_str(
                "      icsSh = lambda((sC) let((sN)\n        sN = if(sC > icsAt sC + icsHi if(sC < icsAt sC - icsLo sC))\n        \
                 when(or(and(sC > icsAt sN < icsAt) and(sC < icsAt sN > icsAt)) icsCross = t)\n        sN))\n",
            );
            let _ = writeln!(
                s,
                "      icsItems = mapcar(lambda((sIt) if(funcall(icsOn sIt {}) funcall(icsSt sIt '{ax}) sIt)) icsItems)",
                layers(ls)?
            );
            let _ = writeln!(
                s,
                "      when(icsCross error(\"%s: {what} ({}): the value is too small, edges would pass the line\" icsName))",
                amount.replace('"', "'")
            );
            let _ = writeln!(
                s,
                "      when(icsGone error(\"%s: {what} ({}) collapses a shape: the value is too small\" icsName))",
                amount.replace('"', "'")
            );
            for (j, o) in self.ops.iter().enumerate().skip(k + 1) {
                match o {
                    Op::Stretch { axis: a2, .. } if a2 == axis => {
                        let _ = writeln!(s, "      icsA{j} = funcall(icsSh icsA{j})");
                    }
                    Op::Repeat { axis: a2, at, .. } => {
                        let r = format!("icsR{j}");
                        let m = match axis {
                            Axis::X => format!("list(funcall(icsSh nth(0 {r})) nth(1 {r}) funcall(icsSh nth(2 {r})) nth(3 {r}))"),
                            Axis::Y => format!("list(nth(0 {r}) funcall(icsSh nth(1 {r})) nth(2 {r}) funcall(icsSh nth(3 {r})))"),
                        };
                        let _ = writeln!(s, "      {r} = {m}");
                        if a2 == axis && at.is_some() {
                            let _ = writeln!(s, "      icsRa{j} = funcall(icsSh icsRa{j})");
                        }
                    }
                    _ => {}
                }
            }
            let _ = writeln!(
                s,
                "      when(exists(sIt icsItems and(memq(car(sIt) '(rect fill))\n          \
                 or(car(cadr(nth(2 sIt))) - car(car(nth(2 sIt))) <= 0 cadr(cadr(nth(2 sIt))) - cadr(car(nth(2 sIt))) <= 0)))\n        \
                 error(\"%s: {what} ({}) collapses a shape: the value is too small\" icsName)))",
                amount.replace('"', "'")
            );
        }
        s.push_str("    ;; Extents after stretching select what repeats.\n");
        s.push_str(
            "    icsItems = mapcar(lambda((hIt) list(car(hIt) nth(1 hIt) nth(2 hIt) nth(3 hIt) funcall(icsBox hIt))) icsItems)\n",
        );
        for (k, op) in self.ops.iter().enumerate() {
            let Op::Repeat { axis, count, pitch, layers: ls, .. } = op else { continue };
            let what = format!("repeat {}", k + 1);
            let (ax, lo, hi, dx, dy) = match axis {
                Axis::X => ("x", 0, 2, "rI * icsP", "0"),
                Axis::Y => ("y", 1, 3, "0", "rI * icsP"),
            };
            let ls = layers(ls)?;
            let _ = writeln!(s, "    ;; {}. repeat along {ax}: {count} copies, pitch {pitch}", k + 1);
            let _ = writeln!(s, "    icsN = round({})", skill_of(count, &what)?);
            let _ = writeln!(
                s,
                "    unless(and(icsN >= 0 icsN <= 10000) error(\"%s: {what}: count %d is out of range\" icsName icsN))"
            );
            let _ = writeln!(s, "    icsP = funcall(icsUm {})", skill_of(pitch, &what)?);
            let _ = writeln!(s, "    when(and(icsP <= 0 icsN > 1) error(\"%s: {what}: pitch must be positive\" icsName))");
            let _ = writeln!(
                s,
                "    icsReg = icsR{k}\n    icsAt = or(icsRa{k} funcall(icsSnap fix((nth({lo} icsReg) + nth({hi} icsReg)) / 2.0)))\n    icsExt = (icsN - 1) * icsP"
            );
            let _ =
                writeln!(s, "    icsMem = setof(rIt icsItems and(funcall(icsOn rIt {ls}) funcall(icsInside icsReg nth(4 rIt))))");
            let _ = writeln!(
                s,
                "    icsRest = setof(rIt icsItems !and(funcall(icsOn rIt {ls}) funcall(icsInside icsReg nth(4 rIt))))"
            );
            let _ = writeln!(
                s,
                "    unless(icsExt == 0\n      icsHi = icsExt  icsLo = 0  icsSh = lambda((eC) if(eC > icsAt eC + icsExt eC))\n      \
                 icsRest = mapcar(lambda((rIt) funcall(icsSt rIt '{ax})) icsRest))"
            );
            let _ =
                writeln!(s, "    for(rI 1 icsN - 1 foreach(rIt icsMem icsRest = cons(funcall(icsMove rIt {dx} {dy}) icsRest)))");
            s.push_str("    when(icsN >= 1 icsRest = append(icsRest icsMem))\n    icsItems = icsRest\n");
        }
        s.push_str(SKILL_OUTPUT);
        s.push_str("    t))\n");
        Ok((s, warnings))
    }
}

// ---------------------------------------------------------------------------------------------
// Expressions

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Var(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(&'static str),
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    const OPS: [&str; 17] = ["<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "(", ")", ",", "<", ">", "!", "%"];
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let st = i;
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let save = i;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                if i < b.len() && (b[i] as char).is_ascii_digit() {
                    while i < b.len() && (b[i] as char).is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            out.push(Tok::Num(s[st..i].parse().map_err(|_| format!("bad number '{}'", &s[st..i]))?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let st = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Id(s[st..i].to_string()));
        } else if let Some(op) = OPS.iter().find(|o| s[i..].starts_with(**o)) {
            out.push(Tok::Op(op));
            i += op.len();
        } else {
            return Err(format!("unexpected '{c}'"));
        }
    }
    Ok(out)
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek_op(&self, op: &str) -> bool {
        matches!(self.t.get(self.i), Some(Tok::Op(o)) if *o == op)
    }

    fn eat(&mut self, op: &str) -> bool {
        let ok = self.peek_op(op);
        if ok {
            self.i += 1;
        }
        ok
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[(&str, char)]; 5] = [
            &[("||", '|')],
            &[("&&", '&')],
            &[("==", '='), ("!=", '~'), ("<=", 'l'), (">=", 'g'), ("<", '<'), (">", '>')],
            &[("+", '+'), ("-", '-')],
            &[("*", '*'), ("/", '/'), ("%", '%')],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        'outer: loop {
            for (text, c) in LEVELS[level] {
                if self.eat(text) {
                    let rhs = self.binary(level + 1)?;
                    lhs = Expr::Bin(*c, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        match self.t.get(self.i).cloned() {
            Some(Tok::Num(v)) => {
                self.i += 1;
                Ok(Expr::Num(v))
            }
            Some(Tok::Id(name)) => {
                self.i += 1;
                if self.eat("(") {
                    let mut args = vec![];
                    if !self.eat(")") {
                        loop {
                            args.push(self.binary(0)?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return Err("expected ',' or ')'".into());
                            }
                        }
                    }
                    if !FUNCTIONS.contains(&name.as_str()) {
                        return Err(format!("unknown function '{name}'"));
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Some(Tok::Op("(")) => {
                self.i += 1;
                let e = self.binary(0)?;
                if !self.eat(")") {
                    return Err("missing ')'".into());
                }
                Ok(e)
            }
            Some(Tok::Op(o)) => Err(format!("unexpected '{o}'")),
            None => Err("unexpected end".into()),
        }
    }
}

/// Parse an arithmetic expression: numbers, parameter names, `+ - * / %`, comparisons,
/// `&& || !` and the functions min, max, abs, floor, ceil, round, sqrt and if(c, a, b).
pub fn parse_expr(s: &str) -> Result<Expr, String> {
    let t = lex(s)?;
    if t.is_empty() {
        return Err("empty expression".into());
    }
    let mut p = Parser { t, i: 0 };
    let e = p.binary(0)?;
    if p.i != p.t.len() {
        return Err("unexpected text after the expression".into());
    }
    Ok(e)
}

impl Expr {
    pub fn eval(&self, v: &Values) -> Result<f64, String> {
        let b = |x: bool| if x { 1.0 } else { 0.0 };
        Ok(match self {
            Expr::Num(x) => *x,
            Expr::Var(n) => *v.get(n).ok_or_else(|| format!("unknown parameter '{n}'"))?,
            Expr::Neg(e) => -e.eval(v)?,
            Expr::Not(e) => b(e.eval(v)? == 0.0),
            Expr::Bin(op, l, r) => {
                let (a, c) = (l.eval(v)?, r.eval(v)?);
                match op {
                    '+' => a + c,
                    '-' => a - c,
                    '*' => a * c,
                    '/' if c == 0.0 => return Err("division by zero".into()),
                    '/' => a / c,
                    '%' if c == 0.0 => return Err("division by zero".into()),
                    '%' => a % c,
                    '|' => b(a != 0.0 || c != 0.0),
                    '&' => b(a != 0.0 && c != 0.0),
                    '=' => b((a - c).abs() < 1e-9),
                    '~' => b((a - c).abs() >= 1e-9),
                    'l' => b(a <= c + 1e-9),
                    'g' => b(a + 1e-9 >= c),
                    '<' => b(a < c - 1e-9),
                    _ => b(a > c + 1e-9),
                }
            }
            Expr::Call(f, args) => {
                let a: Vec<f64> = args.iter().map(|e| e.eval(v)).collect::<Result<_, _>>()?;
                let need = |n: usize| if a.len() == n { Ok(()) } else { Err(format!("{f} takes {n} argument(s)")) };
                match f.as_str() {
                    "min" | "max" if a.is_empty() => return Err(format!("{f} needs arguments")),
                    "min" => a.iter().copied().fold(f64::INFINITY, f64::min),
                    "max" => a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    "abs" => {
                        need(1)?;
                        a[0].abs()
                    }
                    "floor" => {
                        need(1)?;
                        a[0].floor()
                    }
                    "ceil" => {
                        need(1)?;
                        a[0].ceil()
                    }
                    "round" => {
                        need(1)?;
                        a[0].round()
                    }
                    "sqrt" => {
                        need(1)?;
                        a[0].sqrt()
                    }
                    _ => {
                        need(3)?;
                        if a[0] != 0.0 { a[1] } else { a[2] }
                    }
                }
            }
        })
    }

    /// Parameter names used.
    pub fn vars(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        fn walk(e: &Expr, out: &mut BTreeSet<String>) {
            match e {
                Expr::Num(_) => {}
                Expr::Var(n) => {
                    out.insert(n.clone());
                }
                Expr::Neg(x) | Expr::Not(x) => walk(x, out),
                Expr::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                Expr::Call(_, args) => args.iter().for_each(|a| walk(a, out)),
            }
        }
        walk(self, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::NoCells;
    use crate::pcell::PCell;

    fn vals(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
        pairs.iter().map(|(n, v)| (n.to_string(), *v)).collect()
    }

    #[test]
    fn expressions() {
        let v: Values = [("w".to_string(), 2.0), ("nf".to_string(), 3.0)].into();
        let e = |s: &str| parse_expr(s).unwrap().eval(&v).unwrap();
        assert_eq!(e("(nf - 1) * 0.5 + w"), 3.0);
        assert_eq!(e("-w + 2*3"), 4.0);
        assert_eq!(e("max(w, 5, 1)"), 5.0);
        assert_eq!(e("if(nf > 2, 1, 0)"), 1.0);
        assert_eq!(e("nf >= 3 && !(w < 1)"), 1.0);
        assert_eq!(e("1.5e1"), 15.0);
        assert!(parse_expr("w +").is_err());
        assert!(parse_expr("foo(1)").is_err());
        assert!(parse_expr("w)").is_err());
        assert!(parse_expr("q").unwrap().eval(&v).is_err());
        assert_eq!(parse_expr("(nf-1)*w").unwrap().vars().into_iter().collect::<Vec<_>>(), ["nf", "w"]);
    }

    fn plate() -> (Tech, Template) {
        let tech = Tech::generic();
        let m1 = tech.metal1().unwrap();
        let mut l = Layout::default();
        l.add_rect(m1, 0, 0, 1000, 500);
        l.add_rect(m1, 2000, 0, 2500, 500);
        let mut t = Template::from_layout("plate", &l);
        t.params.push(Param::new("w", ParamKind::Length, 1.0));
        t.ops.push(Op::Stretch { axis: Axis::X, at: 500, side: Side::High, amount: "w".into(), layers: vec![] });
        (tech, t)
    }

    #[test]
    fn stretch_grows_crossing_shapes_and_moves_the_rest() {
        let (tech, t) = plate();
        let lay = t.generate(&tech, &vals(&[("w", 1.5)])).unwrap();
        let rects: Vec<IRect> = lay.shapes.iter().map(|s| s.bbox()).collect();
        assert_eq!(rects, [IRect::new(0, 0, 1500, 500), IRect::new(2500, 0, 3000, 500)]);
        assert_eq!(canonical(&t.generate(&tech, &[]).unwrap()), canonical(&t.source));
        let err = t.generate(&tech, &vals(&[("w", 0.4)])).unwrap_err();
        assert!(err.contains("too small"), "{err}");
        assert!(t.generate(&tech, &vals(&[("q", 1.0)])).is_err());
    }

    /// A fixed layer stays put under operations on all layers, but not under one that names it.
    #[test]
    fn fixed_layers_stay_put() {
        let (tech, mut t) = plate();
        let mk = tech.layer_index("text").unwrap();
        t.source.add_rect(mk, 0, 0, 2500, 500);
        t.fixed = vec![mk];
        let lay = t.generate(&tech, &vals(&[("w", 1.5)])).unwrap();
        let on = |l: &Layout, layer: usize| l.shapes.iter().filter(|s| s.layer == layer).map(|s| s.bbox()).collect::<Vec<_>>();
        assert_eq!(on(&lay, mk), [IRect::new(0, 0, 2500, 500)], "fixed marker does not stretch");
        assert_eq!(on(&lay, tech.metal1().unwrap()), [IRect::new(0, 0, 1500, 500), IRect::new(2500, 0, 3000, 500)]);
        t.ops[0] = Op::Stretch { axis: Axis::X, at: 500, side: Side::High, amount: "w".into(), layers: vec![mk] };
        let lay = t.generate(&tech, &vals(&[("w", 1.5)])).unwrap();
        assert_eq!(on(&lay, mk), [IRect::new(0, 0, 3000, 500)], "named layers still stretch");
        let skill = t.to_skill(&tech, "lib").unwrap().0;
        assert!(skill.contains("icsFixed = list(\"text\")"), "{skill}");
        let back: Template = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.fixed, [mk]);
    }

    /// An imported drawing: duplicates, off-grid vertices, odd cuts and placed via cells are
    /// reported; flattening turns the placed cells into shapes.
    #[test]
    fn drawing_findings_and_flatten() {
        let (tech, mut t) = plate();
        let m1 = tech.metal1().unwrap();
        let co = tech.layer_index(&tech.vias[0].cut).unwrap();
        t.source.add_rect(m1, 0, 0, 1000, 500);
        t.source.add(m1, Geometry::Polygon { points: vec![(2000, 0), (2500, 0), (2500, 500), (2000, 500)] });
        t.source.add_rect(m1, 3000, 0, 3333, 500);
        t.source.add_rect(co, 100, 100, 100 + tech.vias[0].cut_size * 2, 100 + tech.vias[0].cut_size);
        let via = crate::project::CellRef::new("lib", "via");
        let mut v = Layout::default();
        v.add_rect(co, 0, 0, tech.vias[0].cut_size, tech.vias[0].cut_size);
        t.source.add_instance(via.clone(), crate::layout::Trans { x: 4000, ..crate::layout::Trans::IDENTITY }, None);
        let text: Vec<String> = t.drawing_findings(&tech).into_iter().map(|f| f.message).collect();
        let has = |s: &str| text.iter().any(|m| m.contains(s));
        assert!(has("2 shape(s) drawn twice"), "{text:?}");
        assert!(has("off the"), "{text:?}");
        assert!(has("1 cut(s) are not"), "{text:?}");
        assert!(has("1 placed cell(s)"), "{text:?}");
        let src: HashMap<crate::project::CellRef, Layout> = [(via, v)].into();
        assert_eq!(t.flatten_instances(&src), 1);
        assert!(t.source.instances.is_empty());
        assert!(t.source.shapes.iter().any(|s| s.layer == co && s.bbox().x0 == 4000));
    }

    /// Suggestions that find no device say which role layers they looked on.
    #[test]
    fn missing_device_names_the_role_layers() {
        let (tech, plate) = plate();
        let t = Template::from_layout("plate", &plate.source);
        let err = t.clone().suggest_fingers(&tech).unwrap_err();
        assert!(err.contains("'poly'") && err.contains("'active'") && err.contains("roles"), "{err}");
        let err = t.clone().suggest_width(&tech).unwrap_err();
        assert!(err.contains("'active'"), "{err}");
    }

    #[test]
    fn centred_stretch_and_later_lines_move() {
        let (tech, mut t) = plate();
        t.ops[0] = Op::Stretch { axis: Axis::X, at: 500, side: Side::Both, amount: "w".into(), layers: vec![] };
        t.ops.push(Op::Stretch { axis: Axis::X, at: 2200, side: Side::High, amount: "2*w".into(), layers: vec![] });
        let lay = t.generate(&tech, &vals(&[("w", 1.2)])).unwrap();
        let rects: Vec<IRect> = lay.shapes.iter().map(|s| s.bbox()).collect();
        // +200 split 100 / 100; the second line moved to 2300 and adds 400 to its right.
        assert_eq!(rects, [IRect::new(-100, 0, 1100, 500), IRect::new(2100, 0, 3000, 500)]);
    }

    #[test]
    fn mos_round_trip_through_suggestions() {
        let tech = Tech::generic();
        let drawn = PCell::mos(false, 1000, 180, 3).generate(&tech).unwrap();
        let mut t = Template::from_layout("my_nmos", &drawn);
        let fills = t.suggest_contact_fills(&tech);
        assert!(fills >= 4, "{fills} fills");
        assert_eq!(t.generate(&tech, &[]).unwrap().shapes.len(), drawn.shapes.len(), "fills re-make the drawn cuts");
        let msg = t.suggest_fingers(&tech).unwrap();
        assert!(msg.contains("3 fingers"), "{msg}");
        t.suggest_width(&tech).unwrap();
        t.suggest_length(&tech).unwrap();
        // The defaults give back the drawing, and width and finger count follow the built-in
        // generator (the gate strap label aside, which stays near the first unit).
        let no_labels = |l: &Layout| canonical(l).into_iter().filter(|s| !s.contains("Label")).collect::<Vec<_>>();
        assert_eq!(no_labels(&t.generate(&tech, &[]).unwrap()), no_labels(&drawn));
        for (w, nf) in [(2000, 3), (1500, 5), (3000, 4), (1000, 2)] {
            let want = PCell::mos(false, w, 180, nf).generate(&tech).unwrap();
            let got = t.generate(&tech, &vals(&[("w", w as f64 / 1000.0), ("nf", nf as f64)])).unwrap();
            assert_eq!(no_labels(&got), no_labels(&want), "w={w} nf={nf}");
        }
        assert!(t.generate(&tech, &vals(&[("l", 0.3), ("nf", 6.0)])).is_ok());
        let findings = t.lint(&tech);
        assert!(findings.iter().all(|f| f.level != Level::Error), "{findings:?}");
        let deck = drc::parse_deck(&tech.deck_text(), &tech).unwrap();
        let runs = t.check_corners(&tech, &deck, &NoCells);
        assert!(runs.len() >= 7);
        for r in &runs {
            let c = r.result.as_ref().unwrap_or_else(|e| panic!("{}: {e}", r.label));
            assert!(c.violations.is_empty(), "{}: {:?}", r.label, c.violations);
        }
    }

    #[test]
    fn repeat_condition_and_stretched_values() {
        let tech = Tech::generic();
        let m1 = tech.metal1().unwrap();
        let m2 = tech.layer_index("metal2").unwrap();
        let mut l = Layout::default();
        l.add_rect(m1, 0, 0, 300, 300);
        l.add_rect(m2, 0, 0, 300, 300);
        l.add_rect(m1, -100, 400, 500, 600);
        let mut t = Template::from_layout("arr", &l);
        t.params.push(Param::new("n", ParamKind::Int, 1.0));
        t.params.push(Param::new("top", ParamKind::Bool, 1.0));
        t.ops.push(Op::Repeat {
            axis: Axis::X,
            region: IRect::new(0, 0, 300, 300),
            count: "n".into(),
            pitch: "0.5".into(),
            at: None,
            layers: vec![m1],
        });
        t.ops.push(Op::Condition { region: IRect::new(0, 0, 300, 300), when: "top".into(), layers: vec![m2] });
        let lay = t.generate(&tech, &vals(&[("n", 3.0), ("top", 0.0)])).unwrap();
        let mut got: Vec<(usize, IRect)> = lay.shapes.iter().map(|s| (s.layer, s.bbox())).collect();
        got.sort_by_key(|g| (g.0, g.1.x0, g.1.y0));
        assert_eq!(
            got,
            [
                (m1, IRect::new(-100, 400, 1500, 600)),
                (m1, IRect::new(0, 0, 300, 300)),
                (m1, IRect::new(500, 0, 800, 300)),
                (m1, IRect::new(1000, 0, 1300, 300)),
            ]
        );
        assert!(t.generate(&tech, &vals(&[("n", 1.5)])).is_err());
        let (tech, p) = plate();
        let v = p.stretched(&p.defaults(), true, 500, &tech).unwrap();
        assert_eq!(v["w"], 1.5);
        assert!(p.stretched(&p.defaults(), false, 500, &tech).is_none());
    }

    /// The SKILL export, run by the SKILL interpreter, makes the same layouts as `generate`.
    #[test]
    fn skill_export_matches_generate() {
        let tech = Tech::generic();
        let drawn = PCell::mos(false, 1000, 180, 3).generate(&tech).unwrap();
        let mut t = Template::from_layout("pc_nmos", &drawn);
        t.suggest_contact_fills(&tech);
        t.suggest_fingers(&tech).unwrap();
        t.suggest_width(&tech).unwrap();
        t.suggest_length(&tech).unwrap();
        // A centred stretch, a condition and a flag exercise the other paths.
        let m2 = tech.layer_index("metal2").unwrap();
        t.source.add_rect(m2, 0, 1700, 400, 2000);
        t.params.push(Param::new("strap", ParamKind::Bool, 1.0));
        t.params.push(Param::new("ext", ParamKind::Length, 0.4));
        t.ops.push(Op::Condition {
            region: IRect::new(0, 1700, 400, 2000),
            when: "strap && nf % 2 == 1".into(),
            layers: vec![m2],
        });
        t.ops.insert(
            2,
            Op::Stretch { axis: Axis::Y, at: 1850, side: Side::Both, amount: "max(ext, 0.2) * 2".into(), layers: vec![m2] },
        );
        // Any-angle shapes across the stretch lines and the finger repeat: a 45° diamond and
        // a 45° path.
        t.source.add(m2, Geometry::Polygon { points: vec![(1000, -1500), (3500, 1000), (1000, 3500), (-1500, 1000)] });
        t.source.add(m2, Geometry::Path { points: vec![(-1000, -1000), (3000, 3000)], width: 200 });
        // A path the width line runs inside (it becomes a polygon) and edges at other angles.
        let wy = t.ops.iter().find(|o| matches!(o, Op::Stretch { axis: Axis::Y, side: Side::High, .. }));
        let Some(Op::Stretch { at: wy, .. }) = wy else { panic!("no w line") };
        t.source.add(m2, Geometry::Path { points: vec![(-800, wy + 20), (2500, wy + 20)], width: 200 });
        t.source.add(m2, Geometry::Polygon { points: vec![(-1000, -1000), (3000, -800), (-1000, 3000)] });
        let (text, warnings) = t.to_skill(&tech, "pclib").unwrap();
        assert!(warnings.is_empty());
        let sets: Vec<Vec<(String, f64)>> = vec![
            vec![],
            vals(&[("nf", 5.0), ("w", 2.0)]),
            vals(&[("nf", 2.0), ("l", 0.3), ("strap", 0.0)]),
            vals(&[("nf", 4.0), ("w", 1.5), ("ext", 0.9)]),
        ];
        let mut src = text.clone();
        src.push_str("cv = dbOpenCellViewByType(\"work\" \"top\" \"layout\" \"maskLayout\" \"w\")\n");
        for (i, set) in sets.iter().enumerate() {
            let ps: Vec<String> = set
                .iter()
                .map(|(n, v)| {
                    let kind = t.param(n).unwrap().kind;
                    if kind == ParamKind::Length {
                        format!("list(\"{n}\" \"float\" {v:?})")
                    } else {
                        format!("list(\"{n}\" \"int\" {})", *v as i64)
                    }
                })
                .collect();
            src.push_str(&format!(
                "dbCreateParamInstByMasterName(cv \"pclib\" \"pc_nmos\" \"layout\" \"I{i}\" {}:0 \"R0\" 1 list({}))\n",
                i * 100,
                ps.join(" ")
            ));
        }
        let mut p = crate::project::Project::new("t");
        p.tech = tech.clone();
        let r = crate::skill::run(&mut p, &src, None).unwrap_or_else(|e| panic!("{e}\n{text}"));
        let top = p.cell(&crate::project::CellRef::new("work", "top")).and_then(|c| c.layout.clone()).unwrap();
        assert_eq!(top.instances.len(), sets.len(), "{}", r.output);
        for (inst, set) in top.instances.iter().zip(&sets) {
            let got = p.cell(&inst.cell).and_then(|c| c.layout.clone()).unwrap();
            let want = t.generate(&tech, set).unwrap();
            assert_eq!(canonical(&got), canonical(&want), "{set:?}");
        }
        // Errors surface as SKILL errors.
        let bad = format!(
            "{text}\ncv = dbOpenCellViewByType(\"work\" \"t2\" \"layout\" \"maskLayout\" \"w\")\n\
            dbCreateParamInstByMasterName(cv \"pclib\" \"pc_nmos\" \"layout\" \"X\" 0:0 \"R0\" 1 list(list(\"w\" \"float\" 0.05)))"
        );
        let mut q = crate::project::Project::new("t");
        q.tech = tech.clone();
        let e = crate::skill::run(&mut q, &bad, None).unwrap_err();
        assert!(e.to_string().contains("too small"), "{e}");
    }

    /// The drawing as polygons (as some imports give it) reads like the drawing as rectangles.
    #[test]
    fn suggestions_read_polygons() {
        let tech = Tech::generic();
        let drawn = PCell::mos(false, 1000, 180, 3).generate(&tech).unwrap();
        let mut as_polys = drawn.clone();
        for s in &mut as_polys.shapes {
            if let Geometry::Rect(r) = s.geom {
                s.geom = Geometry::Polygon { points: vec![(r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1), (r.x0, r.y0)] };
            }
        }
        let suggest = |l: &Layout| {
            let mut t = Template::from_layout("m", l);
            assert!(t.suggest_contact_fills(&tech) >= 4);
            t.suggest_fingers(&tech).unwrap();
            t.suggest_width(&tech).unwrap();
            t.suggest_length(&tech).unwrap();
            t
        };
        let (a, b) = (suggest(&drawn), suggest(&as_polys));
        assert_eq!(a.ops, b.ops);
        assert_eq!(a.params, b.params);
        let rects = |l: Layout| {
            let mut l = l;
            for s in &mut l.shapes {
                if let Some(r) = as_rect(&s.geom) {
                    s.geom = Geometry::Rect(r);
                }
            }
            canonical(&l).into_iter().filter(|s| !s.contains("Label")).collect::<Vec<_>>()
        };
        for set in [vals(&[]), vals(&[("w", 2.0), ("nf", 5.0), ("l", 0.3)])] {
            assert_eq!(rects(a.generate(&tech, &set).unwrap()), rects(b.generate(&tech, &set).unwrap()), "{set:?}");
        }
    }

    #[test]
    fn stretch_keeps_any_angle_edges() {
        let tech = Tech::generic();
        let m1 = tech.metal1().unwrap();
        let one = |g: Geometry, side: Side, w: f64| {
            let mut l = Layout::default();
            l.add(m1, g);
            let mut t = Template::from_layout("c", &l);
            t.params.push(Param::new("w", ParamKind::Length, 1.0));
            t.ops.push(Op::Stretch { axis: Axis::X, at: 1200, side, amount: "w".into(), layers: vec![] });
            t.generate(&tech, &vals(&[("w", w)])).map(|l| l.shapes[0].geom.clone())
        };
        let chamfer = Geometry::Polygon { points: vec![(0, 0), (1000, 0), (1500, 500), (0, 500)] };
        let pts = |g: Geometry| match g {
            Geometry::Polygon { points } | Geometry::Path { points, .. } => points,
            g => panic!("{g:?}"),
        };
        // Growing inserts a band along x at the line; the 45° edge keeps its angle.
        assert_eq!(
            pts(one(chamfer.clone(), Side::High, 1.5).unwrap()),
            [(0, 0), (1000, 0), (1200, 200), (1700, 200), (2000, 500), (0, 500)]
        );
        assert_eq!(
            pts(one(chamfer.clone(), Side::Both, 1.4).unwrap()),
            [(-200, 0), (800, 0), (1000, 200), (1400, 200), (1700, 500), (-200, 500)]
        );
        // Shrinking removes the band (1200, 1400): the cut ends meet in a step across.
        assert_eq!(
            pts(one(chamfer.clone(), Side::High, 0.8).unwrap()),
            [(0, 0), (1000, 0), (1200, 200), (1200, 400), (1300, 500), (0, 500)]
        );
        // A vertex on the line stays; its edge into the moving side moves with that side.
        let tri = Geometry::Polygon { points: vec![(1200, 0), (1700, 500), (1200, 500)] };
        assert_eq!(pts(one(tri, Side::High, 1.5).unwrap()), [(1200, 0), (1700, 0), (2200, 500), (1200, 500)]);
        let path = Geometry::Path { points: vec![(700, 0), (1700, 1000)], width: 100 };
        assert_eq!(pts(one(path, Side::High, 1.5).unwrap()), [(700, 0), (1200, 500), (1700, 500), (2200, 1000)]);
        // An arm squeezed to nothing is an error, as for a rectangle.
        let ell = Geometry::Polygon { points: vec![(0, 0), (1600, 0), (1600, 200), (1200, 200), (1200, 1000), (0, 1000)] };
        let err = one(ell.clone(), Side::High, 0.6).unwrap_err();
        assert!(err.contains("collapses a metal1 shape"), "{err}");
        assert!(one(ell, Side::High, 0.7).is_ok());
    }

    #[test]
    fn paths_cut_along_their_width_grow_like_rects() {
        let tech = Tech::generic();
        let m1 = tech.metal1().unwrap();
        let mut l = Layout::default();
        l.add(m1, Geometry::Path { points: vec![(0, 100), (1000, 100)], width: 200 });
        // A vertical wire the line crosses lengthwise stays a path.
        l.add(m1, Geometry::Path { points: vec![(3000, 0), (3000, 1000)], width: 200 });
        let mut t = Template::from_layout("p", &l);
        t.params.push(Param::new("w", ParamKind::Length, 1.0));
        t.ops.push(Op::Stretch { axis: Axis::Y, at: 150, side: Side::High, amount: "w".into(), layers: vec![] });
        let lay = t.generate(&tech, &vals(&[("w", 1.5)])).unwrap();
        assert!(matches!(lay.shapes[0].geom, Geometry::Polygon { .. }));
        assert_eq!(as_rect(&lay.shapes[0].geom), Some(IRect::new(-100, 0, 1100, 700)));
        assert_eq!(lay.shapes[1].geom, Geometry::Path { points: vec![(3000, 0), (3000, 1500)], width: 200 });
    }

    #[test]
    fn cuts_on_other_angles_snap_to_the_grid() {
        let tech = Tech::generic();
        assert!(tech.grid > 1);
        let m1 = tech.metal1().unwrap();
        let mut l = Layout::default();
        l.add(m1, Geometry::Polygon { points: vec![(0, 0), (1000, 0), (1300, 1000), (0, 1000)] });
        let mut t = Template::from_layout("s", &l);
        t.params.push(Param::new("w", ParamKind::Length, 1.0));
        t.ops.push(Op::Stretch { axis: Axis::X, at: 1100, side: Side::High, amount: "w".into(), layers: vec![] });
        let lay = t.generate(&tech, &vals(&[("w", 1.5)])).unwrap();
        let Geometry::Polygon { points } = &lay.shapes[0].geom else { panic!() };
        assert!(points.iter().all(|p| p.0 % tech.grid == 0 && p.1 % tech.grid == 0), "{points:?}");
        assert!(points.contains(&(1100, snap(333, tech.grid))) && points.contains(&(1600, snap(333, tech.grid))), "{points:?}");
        assert!(t.lint(&tech).iter().any(|f| f.level == Level::Info && f.message.contains("other than 45°")));
    }

    #[test]
    fn finger_keys_ignore_vertex_order() {
        let it = |points: Vec<(i64, i64)>| Item {
            layer: Some(1),
            geom: Geometry::Polygon { points },
            inst: None,
            fill: None,
            home: IRect::default(),
        };
        let a = it(vec![(0, 0), (100, 0), (100, 50), (0, 50)]);
        let b = it(vec![(300, 50), (300, 0), (200, 0), (200, 50)]);
        assert_eq!(a.key((200, 0)), b.key((0, 0)));
    }

    #[test]
    fn lint_reports_mistakes() {
        let (tech, mut t) = plate();
        t.params.push(Param::new("unused", ParamKind::Length, 1.0));
        t.ops.push(Op::Stretch { axis: Axis::Y, at: 9000, side: Side::High, amount: "q + 1".into(), layers: vec![] });
        let f = t.lint(&tech);
        let has = |s: &str| f.iter().any(|x| x.message.contains(s));
        assert!(has("not a parameter"), "{f:?}");
        assert!(has("not used"), "{f:?}");
        assert!(has("no effect"), "{f:?}");
        assert_eq!(f[0].level, Level::Error);
    }
}
