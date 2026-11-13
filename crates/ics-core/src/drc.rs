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
//! [NAME:] antenna    METAL RATIO
//! [NAME:] density    LAYER MIN% MAX% WINDOW [STEP]
//! [NAME:] flag       LAYER
//! NAME = LAYER and|or|not|xor LAYER
//! NAME = LAYER [not] interact|inside|outside|enclose|touch LAYER
//! NAME = LAYER size DIST
//! NAME = LAYER wide MIN
//! NAME = LAYER area MIN MAX      (µm², MAX may be inf)
//! NAME = holes|extents LAYER
//! #@ TEXT                        (description of the next rule)
//! ```
//! A `NAME = ...` line defines a derived layer (e.g. `gate = poly and active`) from technology
//! layers or earlier derived layers; later lines use it like any layer. `not` is `A` minus `B`,
//! `size` grows (DIST > 0) or shrinks (DIST < 0) with square corners, `wide` keeps the parts of
//! the layer at least MIN wide (covered by a MIN x MIN square inside it). Wide-metal spacing is
//! `m1w = metal1 wide 10` with `separation m1w metal1 0.6`. Derived layers are only computed
//! when a rule needs them; violations on them carry no technology layer. The selections keep
//! whole edge-connected pieces of the first layer (see [`Select`]); `area` keeps the pieces with
//! MIN <= area < MAX; `holes` gives the enclosed empty areas of each piece and `extents` its
//! bounding box. `flag` reports every piece of a layer, so any derived layer is a check.
//! A line starting with `#@` gives the next rule its description ([`NamedRule::comment`]),
//! reported instead of the generated one.
//! Decks written in SVRF are translated by [`crate::svrf`] (see [`parse_any`]).
//! Rule kinds, operators and layer names are case-insensitive. Unnamed rules are called
//! `<layer>.W`, `<layer>.S`, `<a>.SEP.<b>`, `<inner>.EN.<outer>`, `<a>.EX.<b>`, `<layer>.A`,
//! `<layer>.ANT`, `<layer>.DEN` and `<layer>.FLAG`.
//!
//! Every check works on the merged geometry of each layer (all Manhattan shapes of the
//! flattened top cell; labels are ignored and non-Manhattan shapes are skipped and counted).
//! Coordinates are integers, so a region is a set of 1 x 1 dbu pixels and the morphology
//! below is exact for any (odd or even) MIN. Facing edges are measured across; corner-to-corner
//! distances in `space` and `separation` are Euclidean (`sqrt(dx² + dy²)`). Sizing (`size`,
//! `enclosure`, `extension`, `wide`) uses square corners.
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
//! - `antenna M RATIO`: for every net formed by poly, diffusion and the metals up to M (through
//!   the technology vias), not connected to diffusion, the area of M over the gate area
//!   (`poly AND active`) of the net must not exceed RATIO. Flags the M pieces of such nets.
//! - `density L MIN% MAX% WINDOW [STEP]`: L must cover between MIN% and MAX% of every WINDOW x
//!   WINDOW µm window, stepped by STEP µm (default WINDOW / 2) over the extent of the layout
//!   (windows are clipped to it). Flags the failing windows.
//!
//! Every connected piece of a rule's violation region becomes one [`Violation`] marked by its
//! bounding box, at most [`MAX_VIOLATIONS_PER_RULE`] per rule (see [`DrcReport::capped`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rstar::{AABB, RTree, RTreeObject};

use crate::geom::IRect;
use crate::hier::{CellBoxes, FlatShape, LayoutSource, flat_bbox, flatten, flatten_in, flatten_in_cached};
use crate::layout::{CellInst, Geometry, Layout, Trans};
use crate::project::CellRef;
use crate::region::{BoolOp, Region};
use crate::tech::Tech;

/// Markers reported per rule; further pieces are only counted.
pub const MAX_VIOLATIONS_PER_RULE: usize = 10_000;

const MAX_DEPTH: u32 = 64;
/// Metal to gate area ratio of the derived deck's antenna rules.
const ANTENNA_RATIO: u32 = 400;

/// A layer computed from earlier layers. Indices count the technology layers first, then the
/// deck's derived layers in definition order (see [`Deck::layer_names`]).
#[derive(Debug, Clone, PartialEq)]
pub enum LayerExpr {
    Bool {
        a: usize,
        op: BoolOp,
        b: usize,
    },
    /// Grow (> 0) or shrink (< 0) by `by` dbu.
    Size {
        a: usize,
        by: i64,
    },
    /// Parts at least `min` dbu wide (morphological opening with a `min` square).
    Wide {
        a: usize,
        min: i64,
    },
    /// The pieces of `a` that relate to `b` as `how` says (or do not, when `not`).
    Select {
        a: usize,
        how: Select,
        not: bool,
        b: usize,
    },
    /// Holes of the pieces of `a`.
    Holes {
        a: usize,
    },
    /// Bounding box of each piece of `a`.
    Extents {
        a: usize,
    },
    /// Pieces of `a` with `min <= area < max` (dbu²).
    AreaRange {
        a: usize,
        min: Option<i128>,
        max: Option<i128>,
    },
    /// Union of any number of layers (none: empty).
    Union(Vec<usize>),
    /// The violation region of a rule.
    Check(Box<Rule>),
}

/// Piece selection of [`LayerExpr::Select`]; pieces are edge-connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    /// Overlaps or touches `b`.
    Interact,
    /// Lies entirely inside `b`.
    Inside,
    /// Does not overlap `b` (touching allowed).
    Outside,
    /// Contains a whole piece of `b`.
    Enclose,
    /// Touches `b` without overlapping it.
    Touch,
}

impl Select {
    pub fn keyword(self) -> &'static str {
        match self {
            Select::Interact => "interact",
            Select::Inside => "inside",
            Select::Outside => "outside",
            Select::Enclose => "enclose",
            Select::Touch => "touch",
        }
    }

    fn parse(s: &str) -> Option<Select> {
        Some(match s.to_ascii_lowercase().as_str() {
            "interact" => Select::Interact,
            "inside" => Select::Inside,
            "outside" => Select::Outside,
            "enclose" => Select::Enclose,
            "touch" => Select::Touch,
            _ => return None,
        })
    }
}

impl LayerExpr {
    pub fn inputs(&self) -> Vec<usize> {
        match self {
            LayerExpr::Bool { a, b, .. } | LayerExpr::Select { a, b, .. } => vec![*a, *b],
            LayerExpr::Size { a, .. }
            | LayerExpr::Wide { a, .. }
            | LayerExpr::Holes { a }
            | LayerExpr::Extents { a }
            | LayerExpr::AreaRange { a, .. } => vec![*a],
            LayerExpr::Union(v) => v.clone(),
            LayerExpr::Check(r) => r.layers(),
        }
    }

    /// The layer computed from `layers` (indexed like [`Deck::layer_names`]; missing ones are
    /// empty). `Check` expressions on density rules see no layout extent.
    pub fn evaluate(&self, layers: &[Region]) -> Region {
        self.eval(layers, None)
    }

    fn eval(&self, layers: &[Region], extent: Option<IRect>) -> Region {
        let empty = Region::default();
        let get = |l: usize| layers.get(l).unwrap_or(&empty);
        match self {
            LayerExpr::Bool { a, op, b } => get(*a).boolean(get(*b), *op),
            LayerExpr::Size { a, by } => get(*a).sized(*by),
            LayerExpr::Wide { a, min } => {
                let k = (min - 1).max(0);
                get(*a).eroded(0, 0, k, k).dilated(0, 0, k, k)
            }
            LayerExpr::Select { a, how, not, b } => select(get(*a), get(*b), *how, *not),
            LayerExpr::Holes { a } => holes(get(*a)),
            LayerExpr::Extents { a } => Region::from_rects(get(*a).parts().iter().filter_map(Region::bbox)),
            LayerExpr::AreaRange { a, min, max } => Region::from_rects(
                get(*a)
                    .parts()
                    .into_iter()
                    .filter(|p| min.is_none_or(|m| p.area() >= m) && max.is_none_or(|m| p.area() < m))
                    .flat_map(|p| p.rects().to_vec()),
            ),
            LayerExpr::Union(v) => Region::from_rects(v.iter().flat_map(|&l| get(l).rects().iter().copied())),
            LayerExpr::Check(r) => check(r, &get, extent),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DerivedLayer {
    pub name: String,
    pub expr: LayerExpr,
}

impl DerivedLayer {
    /// Deck line for this layer, e.g. `gate = poly and active`.
    pub fn deck_line(&self, tech: &Tech, names: &[String]) -> String {
        let n = |l: usize| layer_name(names, l);
        match self.expr {
            LayerExpr::Bool { a, op, b } => {
                let op = match op {
                    BoolOp::And => "and",
                    BoolOp::Or => "or",
                    BoolOp::Not => "not",
                    BoolOp::Xor => "xor",
                };
                format!("{} = {} {op} {}", self.name, n(a), n(b))
            }
            LayerExpr::Size { a, by } => format!("{} = {} size {}", self.name, n(a), fmt_um(by, tech)),
            LayerExpr::Wide { a, min } => format!("{} = {} wide {}", self.name, n(a), fmt_um(min, tech)),
            LayerExpr::Select { a, how, not, b } => {
                format!("{} = {} {}{} {}", self.name, n(a), if not { "not " } else { "" }, how.keyword(), n(b))
            }
            LayerExpr::Holes { a } => format!("{} = holes {}", self.name, n(a)),
            LayerExpr::Extents { a } => format!("{} = extents {}", self.name, n(a)),
            LayerExpr::AreaRange { a, min, max } => {
                let d = tech.dbu_per_um.max(1) as f64;
                let um2 = |v: i128| format!("{}", v as f64 / (d * d));
                format!("{} = {} area {} {}", self.name, n(a), min.map_or("0".into(), um2), max.map_or("inf".into(), um2))
            }
            LayerExpr::Union(ref v) => {
                let parts: Vec<String> = v.iter().map(|&l| n(l)).collect();
                format!("# {} = union of {}", self.name, if parts.is_empty() { "nothing".into() } else { parts.join(", ") })
            }
            LayerExpr::Check(ref r) => format!("# {} = violations of: {}", self.name, r.deck_line(tech, names)),
        }
    }
}

/// A rule with lengths in dbu and areas in dbu². Layers index [`Deck::layer_names`].
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Width {
        layer: usize,
        min: i64,
    },
    Space {
        layer: usize,
        min: i64,
    },
    Separation {
        a: usize,
        b: usize,
        min: i64,
    },
    Enclosure {
        inner: usize,
        outer: usize,
        min: i64,
    },
    Extension {
        a: usize,
        b: usize,
        min: i64,
    },
    Area {
        layer: usize,
        min: i128,
    },
    /// Metal area over gate area of nets not tied to diffusion, with the vias (bottom, cut, top)
    /// up to the metal's level.
    Antenna {
        layer: usize,
        ratio: f64,
        poly: usize,
        active: usize,
        vias: Vec<(usize, usize, usize)>,
    },
    /// Coverage between `min` and `max` percent per `window` square stepped by `step` (dbu).
    Density {
        layer: usize,
        min: f64,
        max: f64,
        window: i64,
        step: i64,
    },
    /// Every piece of the layer is a violation.
    Flag {
        layer: usize,
    },
}

impl Rule {
    /// Rules that need the whole layout: `antenna` (nets) and `density` (windows).
    pub fn is_global(&self) -> bool {
        matches!(self, Rule::Antenna { .. } | Rule::Density { .. })
    }

    /// Distance the rule looks across (dbu); for `area`, the side of the minimum square.
    pub fn reach(&self) -> i64 {
        match *self {
            Rule::Width { min, .. }
            | Rule::Space { min, .. }
            | Rule::Separation { min, .. }
            | Rule::Enclosure { min, .. }
            | Rule::Extension { min, .. } => min,
            Rule::Area { min, .. } => (min.max(0) as f64).sqrt().ceil() as i64,
            _ => 0,
        }
    }

    /// Layers the rule reads.
    pub fn layers(&self) -> Vec<usize> {
        match *self {
            Rule::Width { layer, .. }
            | Rule::Space { layer, .. }
            | Rule::Area { layer, .. }
            | Rule::Density { layer, .. }
            | Rule::Flag { layer } => vec![layer],
            Rule::Separation { a, b, .. } | Rule::Extension { a, b, .. } => vec![a, b],
            Rule::Enclosure { inner, outer, .. } => vec![inner, outer],
            Rule::Antenna { layer, poly, active, ref vias, .. } => {
                let mut v = vec![layer, poly, active];
                v.extend(vias.iter().flat_map(|&(b, c, t)| [b, c, t]));
                v
            }
        }
    }

    /// Layer the violations are reported on.
    pub fn checked_layer(&self) -> usize {
        match *self {
            Rule::Width { layer, .. }
            | Rule::Space { layer, .. }
            | Rule::Area { layer, .. }
            | Rule::Antenna { layer, .. }
            | Rule::Density { layer, .. }
            | Rule::Flag { layer } => layer,
            Rule::Separation { b, .. } => b,
            Rule::Enclosure { inner, .. } => inner,
            Rule::Extension { a, .. } => a,
        }
    }

    /// Name used when the deck gives none, e.g. `metal1.W` or `contact.EN.active`.
    pub fn auto_name(&self, names: &[String]) -> String {
        let n = |l: usize| layer_name(names, l);
        match *self {
            Rule::Width { layer, .. } => format!("{}.W", n(layer)),
            Rule::Space { layer, .. } => format!("{}.S", n(layer)),
            Rule::Separation { a, b, .. } => format!("{}.SEP.{}", n(a), n(b)),
            Rule::Enclosure { inner, outer, .. } => format!("{}.EN.{}", n(inner), n(outer)),
            Rule::Extension { a, b, .. } => format!("{}.EX.{}", n(a), n(b)),
            Rule::Area { layer, .. } => format!("{}.A", n(layer)),
            Rule::Antenna { layer, .. } => format!("{}.ANT", n(layer)),
            Rule::Density { layer, .. } => format!("{}.DEN", n(layer)),
            Rule::Flag { layer } => format!("{}.FLAG", n(layer)),
        }
    }

    /// Human readable, e.g. "metal1 width < 0.23 µm".
    pub fn description(&self, tech: &Tech, names: &[String]) -> String {
        let n = |l: usize| layer_name(names, l);
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
            Rule::Antenna { layer, ratio, .. } => format!("{} antenna ratio > {ratio}", n(layer)),
            Rule::Density { layer, min, max, window, .. } => {
                format!("{} density outside {min}-{max} % ({} µm windows)", n(layer), um(window))
            }
            Rule::Flag { layer } => format!("{} present", n(layer)),
        }
    }

    /// Deck line for this rule (without a name).
    pub fn deck_line(&self, tech: &Tech, names: &[String]) -> String {
        let n = |l: usize| layer_name(names, l);
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
            Rule::Antenna { layer, ratio, .. } => format!("antenna {} {ratio}", n(layer)),
            Rule::Density { layer, min, max, window, step } => {
                format!("density {} {min} {max} {} {}", n(layer), um(window), um(step))
            }
            Rule::Flag { layer } => format!("flag {}", n(layer)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedRule {
    pub name: String,
    pub rule: Rule,
    /// Description reported instead of [`Rule::description`] (e.g. an SVRF `@` comment).
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Deck {
    pub derived: Vec<DerivedLayer>,
    pub rules: Vec<NamedRule>,
}

impl Deck {
    /// The farthest a check looks from a piece of geometry (dbu): the largest rule distance,
    /// minimum area side and derived-layer sizing. Live DRC reads this far around an edit.
    pub fn reach(&self, tech: &Tech) -> i64 {
        let rules = self.rules.iter().map(|r| r.rule.reach());
        let derived = self.derived.iter().map(|d| match d.expr {
            LayerExpr::Size { by, .. } => by.abs(),
            LayerExpr::Wide { min, .. } => min,
            LayerExpr::Check(ref r) => r.reach(),
            _ => 0,
        });
        let total: i64 = derived.sum();
        rules.max().unwrap_or(0) + total + tech.grid.max(1)
    }

    /// The deck without its global rules (`antenna`, `density`), as live DRC runs it.
    pub fn local(&self) -> Deck {
        Deck { derived: self.derived.clone(), rules: self.rules.iter().filter(|r| !r.rule.is_global()).cloned().collect() }
    }

    /// Every layer a rule can refer to: the technology layers, then the derived layers.
    pub fn layer_names(&self, tech: &Tech) -> Vec<String> {
        tech.layers.iter().map(|l| l.name.clone()).chain(self.derived.iter().map(|d| d.name.clone())).collect()
    }
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

fn layer_name(names: &[String], l: usize) -> String {
    names.get(l).cloned().unwrap_or_else(|| format!("#{l}"))
}

fn lookup(names: &[String], name: &str, errs: &mut Vec<String>) -> Option<usize> {
    let l = names.iter().position(|n| n.eq_ignore_ascii_case(name));
    if l.is_none() {
        errs.push(format!("unknown layer '{name}'"));
    }
    l
}

fn fmt_um(v: i64, tech: &Tech) -> String {
    format!("{}", v as f64 / tech.dbu_per_um.max(1) as f64)
}

/// A deck from [`parse_any`], with the notes of an SVRF translation.
#[derive(Debug, Clone, Default)]
pub struct ParsedDeck {
    pub deck: Deck,
    pub notes: Vec<String>,
}

/// Parse an IC Studio deck, or translate an SVRF one ([`crate::svrf::looks_like_svrf`]). SVRF
/// statements that are not translated become notes, not errors.
pub fn parse_any(text: &str, tech: &Tech) -> Result<ParsedDeck, Vec<DeckError>> {
    if crate::svrf::looks_like_svrf(text) {
        let t = crate::svrf::translate(text, tech, &[]);
        let mut notes: Vec<String> = t.notes.iter().map(|n| format!("SVRF {n}")).collect();
        notes.insert(0, format!("SVRF: {} of {} rule check(s) translated", t.translated, t.checks));
        return Ok(ParsedDeck { deck: t.deck, notes });
    }
    parse_deck(text, tech).map(|deck| ParsedDeck { deck, notes: vec![] })
}

/// Read a deck file: an IC Studio deck, or SVRF with its `INCLUDE`s resolved relative to the
/// file. Errors are prefixed with the file name.
pub fn load_deck(path: &std::path::Path, tech: &Tech) -> Result<ParsedDeck, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = deck_file_text(&text, path)?;
    parse_any(&text.0, tech)
        .map(|mut d| {
            d.notes.extend(text.1);
            d
        })
        .map_err(|errs| {
            errs.iter().map(|e| format!("{}:{}: {}", path.display(), e.line, e.message)).collect::<Vec<_>>().join("\n")
        })
}

/// The text of deck file `path` with SVRF `INCLUDE`s inlined, and the includes that failed.
pub fn deck_file_text(text: &str, path: &std::path::Path) -> Result<(String, Vec<String>), String> {
    if !crate::svrf::looks_like_svrf(text) {
        return Ok((text.to_string(), vec![]));
    }
    Ok(crate::svrf::inline_includes(text, path.parent().unwrap_or(std::path::Path::new("."))))
}

/// Parse a deck. All errors are reported, not just the first.
pub fn parse_deck(text: &str, tech: &Tech) -> Result<Deck, Vec<DeckError>> {
    let mut deck = Deck::default();
    let mut names = deck.layer_names(tech);
    let mut errors = Vec::new();
    let mut comment: Option<String> = None;
    for (i, raw) in text.lines().enumerate() {
        if let Some(c) = raw.trim().strip_prefix("#@") {
            comment = Some(c.trim().to_string());
            continue;
        }
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let parsed = match line.split_once('=') {
            Some((lhs, rhs)) => parse_derived(lhs, rhs, &names, tech).map(|d| {
                names.push(d.name.clone());
                deck.derived.push(d);
            }),
            None => {
                let c = comment.take();
                parse_line(line, &names, tech).map(|r| deck.rules.push(NamedRule { comment: c, ..r }))
            }
        };
        if let Err(es) = parsed {
            errors.extend(es.into_iter().map(|message| DeckError { line: i + 1, message }));
        }
    }
    if errors.is_empty() { Ok(deck) } else { Err(errors) }
}

fn parse_derived(lhs: &str, rhs: &str, names: &[String], tech: &Tech) -> Result<DerivedLayer, Vec<String>> {
    let name = lhs.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
        return Err(vec![format!("invalid layer name '{name}'")]);
    }
    let mut errs = Vec::new();
    if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        errs.push(format!("layer '{name}' is already defined"));
    }
    let toks: Vec<&str> = rhs.split_whitespace().collect();
    let um2 = |t: &str, errs: &mut Vec<String>| -> Option<Option<i128>> {
        if t.eq_ignore_ascii_case("inf") {
            return Some(None);
        }
        let d = tech.dbu_per_um as f64;
        match t.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0) {
            Some(v) => Some(Some((v * d * d).round() as i128)),
            None => {
                errs.push(format!("bad area '{t}' (expected µm² or inf)"));
                None
            }
        }
    };
    let expr = match toks.as_slice() {
        [op, a] if op.eq_ignore_ascii_case("holes") || op.eq_ignore_ascii_case("extents") => {
            let a = lookup(names, a, &mut errs);
            a.map(|a| if op.eq_ignore_ascii_case("holes") { LayerExpr::Holes { a } } else { LayerExpr::Extents { a } })
        }
        [a, not, how, b] if not.eq_ignore_ascii_case("not") && Select::parse(how).is_some() => {
            let (a, b) = (lookup(names, a, &mut errs), lookup(names, b, &mut errs));
            a.zip(b).map(|(a, b)| LayerExpr::Select { a, how: Select::parse(how).expect("checked"), not: true, b })
        }
        [a, how, b] if Select::parse(how).is_some() => {
            let (a, b) = (lookup(names, a, &mut errs), lookup(names, b, &mut errs));
            a.zip(b).map(|(a, b)| LayerExpr::Select { a, how: Select::parse(how).expect("checked"), not: false, b })
        }
        [a, kw, lo, hi] if kw.eq_ignore_ascii_case("area") => {
            let a = lookup(names, a, &mut errs);
            let (min, max) = (um2(lo, &mut errs), um2(hi, &mut errs));
            match (a, min, max) {
                (Some(a), Some(min), Some(max)) => Some(LayerExpr::AreaRange { a, min, max }),
                _ => None,
            }
        }
        [_, _, _] => parse_simple(&toks, names, tech, &mut errs),
        _ => {
            errs.push(format!(
                "expected '{name} = LAYER and|or|not|xor LAYER', '{name} = LAYER [not] interact|inside|outside|enclose|touch LAYER', '{name} = LAYER size|wide DIST', '{name} = LAYER area MIN MAX' or '{name} = holes|extents LAYER'"
            ));
            None
        }
    };
    match expr {
        Some(expr) if errs.is_empty() => Ok(DerivedLayer { name: name.to_string(), expr }),
        _ => Err(errs),
    }
}

/// `A and|or|not|xor B` or `A size|wide DIST`.
fn parse_simple(toks: &[&str], names: &[String], tech: &Tech, errs: &mut Vec<String>) -> Option<LayerExpr> {
    let a = lookup(names, toks[0], errs);
    let kind = toks[1].to_ascii_lowercase();
    let op = match kind.as_str() {
        "and" => Some(BoolOp::And),
        "or" => Some(BoolOp::Or),
        "not" => Some(BoolOp::Not),
        "xor" => Some(BoolOp::Xor),
        _ => None,
    };
    if let Some(op) = op {
        let b = lookup(names, toks[2], errs);
        a.zip(b).map(|(a, b)| LayerExpr::Bool { a, op, b })
    } else if kind == "size" || kind == "wide" {
        let wide = kind == "wide";
        let by = toks[2]
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && (!wide || *v > 0.0))
            .map(|v| (v * tech.dbu_per_um as f64).round() as i64);
        if by.is_none() {
            let what = if wide { "a positive width" } else { "a value" };
            errs.push(format!("bad distance '{}' (expected {what} in µm)", toks[2]));
        }
        a.zip(by).map(|(a, by)| if wide { LayerExpr::Wide { a, min: by } } else { LayerExpr::Size { a, by } })
    } else {
        errs.push(format!(
            "unknown operator '{}' (expected and, or, not, xor, interact, inside, outside, enclose, touch, size or wide)",
            toks[1]
        ));
        None
    }
}

fn parse_line(line: &str, names: &[String], tech: &Tech) -> Result<NamedRule, Vec<String>> {
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
    let (n_layers, n_nums, optional, expect) = match kind.as_str() {
        "width" | "space" | "area" => (1, 1, 0, "LAYER MIN"),
        "separation" | "enclosure" | "extension" => (2, 1, 0, "LAYER LAYER MIN"),
        "antenna" => (1, 1, 0, "METAL RATIO"),
        "density" => (1, 3, 1, "LAYER MIN% MAX% WINDOW [STEP]"),
        "flag" => (1, 0, 0, "LAYER"),
        _ => {
            return Err(vec![format!(
                "unknown rule kind '{kind}' (expected width, space, separation, enclosure, extension, area, antenna, density or flag)"
            )]);
        }
    };
    let args = toks.len() - 1;
    if args < n_layers + n_nums || args > n_layers + n_nums + optional {
        return Err(vec![format!("'{kind}' expects {expect}, got {args} argument(s)")]);
    }
    let mut errs = Vec::new();
    let mut layers = Vec::new();
    for t in &toks[1..=n_layers] {
        if let Some(l) = lookup(names, t, &mut errs) {
            layers.push(l);
        }
    }
    let mut nums = Vec::new();
    for num in &toks[n_layers + 1..] {
        match num.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0) {
            Some(v) => nums.push(v),
            None => errs.push(format!("bad number '{num}' (expected a non-negative value)")),
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let d = tech.dbu_per_um as f64;
    let dbu = |v: f64| (v * d).round() as i64;
    if kind == "flag" {
        return Ok(NamedRule {
            name: name.unwrap_or_else(|| Rule::Flag { layer: layers[0] }.auto_name(names)),
            rule: Rule::Flag { layer: layers[0] },
            comment: None,
        });
    }
    let (l0, v) = (layers[0], nums[0]);
    let rule = match kind.as_str() {
        "width" => Rule::Width { layer: l0, min: dbu(v) },
        "space" => Rule::Space { layer: l0, min: dbu(v) },
        "area" => Rule::Area { layer: l0, min: (v * d * d).round() as i128 },
        "separation" => Rule::Separation { a: l0, b: layers[1], min: dbu(v) },
        "enclosure" => Rule::Enclosure { inner: l0, outer: layers[1], min: dbu(v) },
        "extension" => Rule::Extension { a: l0, b: layers[1], min: dbu(v) },
        "antenna" => antenna_rule(l0, v, tech).map_err(|e| vec![e])?,
        _ => {
            let (min, max, window) = (nums[0], nums[1], dbu(nums[2]));
            if min > max || max > 100.0 {
                return Err(vec![format!("density limits {min} - {max} must satisfy 0 <= MIN <= MAX <= 100")]);
            }
            if window <= 0 {
                return Err(vec!["the density window must be larger than 0".into()]);
            }
            let step = nums.get(3).map_or(window / 2, |&s| dbu(s)).max(1);
            Rule::Density { layer: l0, min, max, window, step }
        }
    };
    let name = name.unwrap_or_else(|| rule.auto_name(names));
    Ok(NamedRule { name, rule, comment: None })
}

/// Antenna rule for metal `layer`: the technology vias up to its level.
fn antenna_rule(layer: usize, ratio: f64, tech: &Tech) -> Result<Rule, String> {
    let lid = |n: &str| tech.layer_index(n);
    let (Some(poly), Some(active)) = (tech.poly(), tech.diffusion()) else {
        return Err("antenna rules need poly and diffusion layers".into());
    };
    let vias: Vec<(usize, usize, usize)> =
        tech.vias.iter().filter_map(|v| Some((lid(&v.bottom)?, lid(&v.cut)?, lid(&v.top)?))).collect();
    let n = tech.layers.len();
    if layer >= n || layer == poly || layer == active || !vias.iter().any(|&(b, _, t)| b == layer || t == layer) {
        let name = tech.layer(layer).map_or_else(|| format!("#{layer}"), |l| l.name.clone());
        return Err(format!("antenna: '{name}' is not a metal of the via stack"));
    }
    let mut level = vec![0usize; n];
    for _ in 0..vias.len() {
        for &(b, _, t) in &vias {
            level[t] = level[t].max(level[b] + 1);
        }
    }
    let vias = vias.into_iter().filter(|&(b, _, t)| level[b] <= level[layer] && level[t] <= level[layer]).collect();
    Ok(Rule::Antenna { layer, ratio, poly, active, vias })
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
        if l.min_area > 0 {
            let d = tech.dbu_per_um.max(1) as f64;
            push(format!("area {} {}", l.name, l.min_area as f64 / (d * d)));
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
    if let (Some(poly), Some(active)) = (tech.poly().and_then(|i| tech.layer(i)), tech.diffusion().and_then(|i| tech.layer(i))) {
        push(format!("extension {} {} {}", poly.name, active.name, um(tech.rules.poly_endcap)));
        for v in &tech.vias {
            if let Some(top) = def(&v.top).filter(|t| t.routing && t.name != poly.name && t.name != active.name) {
                push(format!("antenna {} {ANTENNA_RATIO}", top.name));
            }
        }
    }
    let mut out = format!("# DRC deck derived from technology '{}' (lengths in µm)\n", tech.name);
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    if !tech.drc_extra.is_empty() {
        out.push_str("# technology extras\n");
        for l in &tech.drc_extra {
            out.push_str(l.trim());
            out.push('\n');
        }
    }
    out
}

/// Run the technology's deck ([`Tech::deck_text`]).
pub fn run_tech_deck(top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Result<DrcReport, Vec<DeckError>> {
    Ok(run(&parse_any(&tech.deck_text(), tech)?.deck, top, src, tech))
}

pub fn run(deck: &Deck, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> DrcReport {
    run_shapes(deck, flatten(top, src, MAX_DEPTH), tech, None)
}

/// Live DRC of the area `window`: only the geometry within [`Deck::reach`] of it is read, the
/// global rules (`antenna`, `density`) are skipped and only violations touching `window` are
/// reported. Pieces that extend beyond the halo are cut, so piece-based checks (`area`,
/// selections) are exact only for pieces near the window; sign-off is [`run`].
pub fn run_window(deck: &Deck, top: &Layout, src: &dyn LayoutSource, tech: &Tech, window: IRect) -> DrcReport {
    let h = 2 * deck.reach(tech) + tech.grid.max(1);
    let halo = IRect::new(window.x0 - h, window.y0 - h, window.x1 + h, window.y1 + h);
    let mut rep = run_shapes(&deck.local(), flatten_in(top, src, MAX_DEPTH, halo), tech, Some(window));
    rep.rules_run = deck.rules.len();
    rep
}

/// [`run`] computed hierarchically ([`HierDrc`]); the report is the same.
pub fn run_hier(deck: &Deck, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> DrcReport {
    HierDrc::new(deck, tech).run(top, src, tech)
}

/// How far a rule's verdict at a point depends on the geometry around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Only on the geometry within the halo ([`HierDrc`]): `width`, `space`, `extension`, `flag`.
    Local,
    /// Violations lie inside whole pieces of this layer and depend on the piece and the
    /// geometry within the halo of it: `area`, `separation` (B pieces), `enclosure` (inner pieces).
    Piece(usize),
    /// On the whole layout: global rules and rules reading piece selections, holes or extents.
    Flat,
}

fn scopes(deck: &Deck, n: usize) -> Vec<Scope> {
    let mut local = vec![true; n];
    for d in &deck.derived {
        let ok = match &d.expr {
            LayerExpr::Bool { .. } | LayerExpr::Size { .. } | LayerExpr::Wide { .. } | LayerExpr::Union(_) => {
                d.expr.inputs().iter().all(|&l| local.get(l).copied().unwrap_or(true))
            }
            LayerExpr::Check(r) => rule_scope(r, &local) == Scope::Local,
            _ => false,
        };
        local.push(ok);
    }
    deck.rules.iter().map(|r| rule_scope(&r.rule, &local)).collect()
}

fn rule_scope(rule: &Rule, local: &[bool]) -> Scope {
    if rule.is_global() || !rule.layers().iter().all(|&l| local.get(l).copied().unwrap_or(true)) {
        return Scope::Flat;
    }
    match *rule {
        Rule::Area { layer, .. } => Scope::Piece(layer),
        Rule::Separation { b, .. } => Scope::Piece(b),
        Rule::Enclosure { inner, .. } => Scope::Piece(inner),
        _ => Scope::Local,
    }
}

fn grown(r: IRect, d: i64) -> IRect {
    IRect { x0: r.x0 - d, y0: r.y0 - d, x1: r.x1 + d, y1: r.y1 + d }
}

fn covers(outer: &IRect, inner: &IRect) -> bool {
    outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && inner.x1 <= outer.x1 && inner.y1 <= outer.y1
}

fn rect_area(r: &IRect) -> i128 {
    r.width() as i128 * r.height() as i128
}

/// Points within `h` of two different groups of boxes (`(box, group)`).
fn interactions(items: &[(IRect, usize)], h: i64) -> Region {
    let tree = RTree::bulk_load(items.iter().enumerate().map(|(k, it)| Tagged(grown(it.0, h), k)).collect());
    let mut out = Vec::new();
    for (k, &(b, g)) in items.iter().enumerate() {
        let e = grown(b, h);
        for t in tree.locate_in_envelope_intersecting(aabb(e)) {
            if t.1 > k && items[t.1].1 != g && t.0.intersects(&e) {
                out.push(IRect::new(e.x0.max(t.0.x0), e.y0.max(t.0.y0), e.x1.min(t.0.x1), e.y1.min(t.0.y1)));
            }
        }
    }
    Region::from_rects(out)
}

/// Nearby boxes grouped into windows no larger than the sum of their padded areas and at most
/// `64 * pad` wide and high (unless a single box is larger), so they run in parallel.
fn group_boxes(boxes: &[IRect], pad: i64) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by_key(|&i| (boxes[i].y0, boxes[i].x0));
    let cap = 64 * pad.max(1);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let (mut bb, mut sum) = (IRect::default(), 0i128);
    for i in order {
        let b = grown(boxes[i], pad);
        if let Some(g) = groups.last_mut() {
            let u = bb.union(&b);
            if u.width() <= cap && u.height() <= cap && rect_area(&u) <= sum + rect_area(&b) {
                g.push(i);
                (bb, sum) = (u, sum + rect_area(&b));
                continue;
            }
        }
        groups.push(vec![i]);
        (bb, sum) = (b, rect_area(&b));
    }
    groups
}

/// Shapes checked, non-Manhattan shapes skipped and the bounding box of the cell's own shapes.
fn own_counts(l: &Layout, n: usize) -> (usize, usize, Option<IRect>) {
    let (mut shapes, mut skipped, mut bbox) = (0, 0, None::<IRect>);
    for s in l.shapes.iter().filter(|s| s.layer < n && !matches!(s.geom, Geometry::Label { .. })) {
        shapes += 1;
        if !matches!(s.geom, Geometry::Rect(_)) && Region::from_geometry(&s.geom).is_err() {
            skipped += 1;
        }
        let b = s.bbox();
        bbox = Some(bbox.map_or(b, |x| x.union(&b)));
    }
    (shapes, skipped, bbox)
}

/// The checked result of one cell definition.
struct CellDrc {
    /// Violation region of every rule in cell coordinates (empty for [`Scope::Flat`] rules).
    regions: Vec<Region>,
    /// Bounding box of the flattened shapes.
    bbox: Option<IRect>,
    shapes: usize,
    skipped: usize,
    /// Levels of hierarchy below the cell.
    height: u32,
}

/// What a re-checked zone replaces in earlier results.
enum Mask {
    /// The area itself: the zone, plus for piece rules every piece overlapping it.
    Cut(Region),
    /// Every violation piece overlapping the zone (`area` rules, whose violations are whole pieces).
    Pieces(Region),
}

impl Mask {
    fn apply(&self, old: &Region) -> Region {
        match self {
            Mask::Cut(m) => old.boolean(m, BoolOp::Not),
            Mask::Pieces(z) => split_overlapping(old, z).1,
        }
    }

    fn bbox(&self) -> Option<IRect> {
        match self {
            Mask::Cut(m) | Mask::Pieces(m) => m.bbox(),
        }
    }
}

/// Exact rule regions inside a zone: `res` replaces what `mask` removes.
struct Zone {
    mask: Vec<Mask>,
    res: Vec<Region>,
}

#[derive(Default)]
struct Found {
    res: Vec<(usize, Region)>,
    pieces: Vec<(usize, Region)>,
    pending: Vec<(usize, Region)>,
}

/// Hierarchical DRC with results equal to the flat [`run`].
///
/// Each cell definition is checked once and cached (per deck). In a parent, the violations of
/// every placement and of the parent's own shapes checked alone are valid wherever only one of
/// them lies within the halo (twice the deck's reach, see [`Scope`]); the remaining
/// interaction zone, where placements overlap or come near each other or the parent's shapes,
/// is re-checked on the flattened geometry around it. Piece rules (`area`, `separation`,
/// `enclosure`) re-check every piece overlapping the zone in full, widening the window until
/// the piece is complete. Global rules and rules on piece selections run on the flat layout.
/// Rotated placements can differ from flat DRC only where two diagonal corner gaps of a
/// `space` rule tie.
pub struct HierDrc {
    deck: Deck,
    n: usize,
    halo: i64,
    scope: Vec<Scope>,
    cells: HashMap<CellRef, Arc<CellDrc>>,
    /// Cells checked by their interaction zones.
    #[cfg(test)]
    zoned: usize,
}

impl HierDrc {
    pub fn new(deck: &Deck, tech: &Tech) -> Self {
        let n = tech.layers.len();
        let scope = scopes(deck, n);
        let rules = deck
            .rules
            .iter()
            .zip(&scope)
            .filter(|(r, s)| **s != Scope::Flat && !matches!(r.rule, Rule::Area { .. }))
            .map(|(r, _)| r.rule.reach().max(0));
        let derived: i64 = deck
            .derived
            .iter()
            .map(|d| match d.expr {
                LayerExpr::Size { by, .. } => by.abs(),
                LayerExpr::Wide { min, .. } => min.max(0),
                LayerExpr::Check(ref r) => r.reach().max(0),
                _ => 0,
            })
            .sum();
        let halo = 2 * (rules.max().unwrap_or(0) + derived) + 2 * tech.grid.max(1) + 4;
        Self {
            deck: deck.clone(),
            n,
            halo,
            scope,
            cells: HashMap::new(),
            #[cfg(test)]
            zoned: 0,
        }
    }

    pub fn deck(&self) -> &Deck {
        &self.deck
    }

    /// Forget the cached cells (after a cell definition changed).
    pub fn clear(&mut self) {
        self.cells.clear();
    }

    pub fn run(&mut self, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> DrcReport {
        let start = Instant::now();
        let (regions, shapes, skipped) = self.regions(top, src);
        let mut rep = report_of(&self.deck, tech, &regions, None);
        (rep.shapes, rep.skipped) = (shapes, skipped);
        rep.millis = start.elapsed().as_secs_f64() * 1000.0;
        rep
    }

    fn rules(&self, f: impl Fn(Scope) -> bool) -> Vec<usize> {
        (0..self.deck.rules.len()).filter(|&k| f(self.scope[k])).collect()
    }

    /// Every rule's violation region of `top`, the shapes checked and skipped.
    fn regions(&mut self, top: &Layout, src: &dyn LayoutSource) -> (Vec<Region>, usize, usize) {
        match self.check_layout(top, src, 0) {
            Some(c) => (self.fill_flat(c.regions, top, src), c.shapes, c.skipped),
            None => {
                let all: Vec<usize> = (0..self.deck.rules.len()).collect();
                let used = used_layers(&self.deck, self.n, all.iter().copied());
                let layers = layers_of(&self.deck, self.n, &flatten(top, src, MAX_DEPTH), &used, true);
                (self.checks(&layers, &all, true), layers.shapes, layers.skipped)
            }
        }
    }

    /// `regions` with the [`Scope::Flat`] rules computed on the flattened layout.
    fn fill_flat(&self, mut regions: Vec<Region>, top: &Layout, src: &dyn LayoutSource) -> Vec<Region> {
        let flat = self.rules(|s| s == Scope::Flat);
        if !flat.is_empty() {
            let used = used_layers(&self.deck, self.n, flat.iter().copied());
            let layers = layers_of(&self.deck, self.n, &flatten(top, src, MAX_DEPTH), &used, true);
            let mut res = self.checks(&layers, &flat, true);
            for &k in &flat {
                regions[k] = std::mem::take(&mut res[k]);
            }
        }
        regions
    }

    /// Rule regions of `rules` (others empty).
    fn checks(&self, layers: &Layers, rules: &[usize], parallel: bool) -> Vec<Region> {
        let empty = Region::default();
        let layer = |l: usize| layers.regions.get(l).unwrap_or(&empty);
        let f = |k: &usize| check(&self.deck.rules[*k].rule, &layer, layers.extent);
        let res: Vec<Region> = if parallel { par_map(rules, f) } else { rules.iter().map(f).collect() };
        let mut out = vec![Region::default(); self.deck.rules.len()];
        for (&k, r) in rules.iter().zip(res) {
            out[k] = r;
        }
        out
    }

    fn cell(&mut self, cell: &CellRef, l: &Layout, src: &dyn LayoutSource, depth: u32) -> Option<Arc<CellDrc>> {
        if let Some(c) = self.cells.get(cell) {
            return Some(c.clone());
        }
        let c = Arc::new(self.check_layout(l, src, depth)?);
        self.cells.insert(cell.clone(), c.clone());
        Some(c)
    }

    /// The checked cells placed by `l`. `None` when the hierarchy is cyclic or deeper than
    /// flattening goes.
    fn children<'l>(&mut self, l: &'l Layout, src: &dyn LayoutSource, depth: u32) -> Option<Vec<(&'l CellInst, Arc<CellDrc>)>> {
        let mut out = Vec::new();
        for inst in &l.instances {
            let Some(cl) = src.layout(&inst.cell) else { continue };
            if depth >= MAX_DEPTH {
                return None;
            }
            let c = self.cell(&inst.cell, cl, src, depth + 1)?;
            if depth + 1 + c.height > MAX_DEPTH {
                return None;
            }
            out.push((inst, c));
        }
        Some(out)
    }

    /// Shapes and skipped shapes of `l` flattened.
    fn counts(&mut self, l: &Layout, src: &dyn LayoutSource) -> Option<(usize, usize)> {
        let (mut shapes, mut skipped, _) = own_counts(l, self.n);
        for (inst, c) in self.children(l, src, 0)? {
            shapes += c.shapes * inst.count() as usize;
            skipped += c.skipped * inst.count() as usize;
        }
        Some((shapes, skipped))
    }

    fn check_layout(&mut self, l: &Layout, src: &dyn LayoutSource, depth: u32) -> Option<CellDrc> {
        let children = self.children(l, src, depth)?;
        let height = children.iter().map(|c| c.1.height + 1).max().unwrap_or(0);
        let rules = self.rules(|s| s != Scope::Flat);
        let used = used_layers(&self.deck, self.n, rules.iter().copied());
        let own: Vec<FlatShape> = l.shapes.iter().map(|s| FlatShape { layer: s.layer, geom: s.geom.clone(), id: None }).collect();
        let own_regions = self.checks(&layers_of(&self.deck, self.n, &own, &used, true), &rules, true);
        let (mut shapes, mut skipped, mut bbox) = own_counts(l, self.n);
        let mut items: Vec<(IRect, usize)> = l
            .shapes
            .iter()
            .filter(|s| s.layer < self.n && !matches!(s.geom, Geometry::Label { .. }))
            .map(|s| (s.bbox(), 0))
            .collect();
        let mut elems: Vec<(Trans, usize)> = Vec::new();
        for (ci, (inst, c)) in children.iter().enumerate() {
            shapes += c.shapes * inst.count() as usize;
            skipped += c.skipped * inst.count() as usize;
            let Some(cb) = c.bbox else { continue };
            let b = inst.bbox(cb);
            bbox = Some(bbox.map_or(b, |x| x.union(&b)));
            let (cols, rows) = inst.dims();
            for j in 0..rows {
                for i in 0..cols {
                    let t = inst.element_trans(i, j);
                    elems.push((t, ci));
                    items.push((t.apply_rect(cb), elems.len()));
                }
            }
        }
        let done = |regions: Vec<Region>| Some(CellDrc { regions, bbox, shapes, skipped, height });
        let Some(extent) = bbox.filter(|_| !elems.is_empty()) else { return done(own_regions) };
        let z = interactions(&items, self.halo);
        if z.area() * 2 > rect_area(&grown(extent, self.halo)) {
            let layers = layers_of(&self.deck, self.n, &flatten(l, src, MAX_DEPTH - depth), &used, true);
            return done(self.checks(&layers, &rules, true));
        }
        #[cfg(test)]
        {
            self.zoned += 1;
        }
        let zone = self.zone(l, src, &z, extent, depth);
        let mut regions = vec![Region::default(); self.deck.rules.len()];
        for &k in &rules {
            let mut rects = own_regions[k].rects().to_vec();
            for (t, ci) in &elems {
                rects.extend(children[*ci].1.regions[k].rects().iter().map(|r| t.apply_rect(*r)));
            }
            regions[k] = zone.mask[k].apply(&Region::from_rects(rects)).boolean(&zone.res[k], BoolOp::Or);
        }
        done(regions)
    }

    /// The non-flat rules checked exactly inside zone `z` of `top` (whose shapes lie in `extent`).
    fn zone(&self, top: &Layout, src: &dyn LayoutSource, z: &Region, extent: IRect, depth: u32) -> Zone {
        let nr = self.deck.rules.len();
        let h = self.halo;
        let max_depth = MAX_DEPTH.saturating_sub(depth);
        let full = grown(extent, h + 1);
        let rules = self.rules(|s| s != Scope::Flat);
        let mut piece_layers: Vec<usize> =
            rules.iter().filter_map(|&k| if let Scope::Piece(l) = self.scope[k] { Some(l) } else { None }).collect();
        piece_layers.sort_unstable();
        piece_layers.dedup();
        let used = used_layers(&self.deck, self.n, rules.iter().copied());
        let (mut res, mut found) = (vec![Vec::new(); nr], HashMap::<usize, Vec<IRect>>::new());
        let mut collect = |outs: Vec<Found>, pending: &mut Vec<(usize, Region)>| {
            for o in outs {
                for (k, r) in o.res {
                    res[k].extend_from_slice(r.rects());
                }
                for (l, r) in o.pieces {
                    found.entry(l).or_default().extend_from_slice(r.rects());
                }
                pending.extend(o.pending);
            }
        };

        let boxes = z.rects();
        let groups: Vec<(Region, IRect)> = group_boxes(boxes, 2 * h)
            .iter()
            .map(|g| {
                let target = Region::from_rects(g.iter().map(|&i| boxes[i]));
                let exact = grown(target.bbox().unwrap_or_default(), h);
                (target, exact)
            })
            .collect();
        let outs = par_windows(
            top,
            src,
            max_depth,
            &groups,
            |w| grown(w.1, h),
            |(target, exact), flat| {
                let layers = layers_of(&self.deck, self.n, flat, &used, false);
                let mut out = Found::default();
                let empty = Region::default();
                let layer = |l: usize| layers.regions.get(l).unwrap_or(&empty);
                for &k in rules.iter().filter(|&&k| self.scope[k] == Scope::Local) {
                    out.res.push((k, check(&self.deck.rules[k].rule, &layer, None).boolean(target, BoolOp::And)));
                }
                for &pl in &piece_layers {
                    self.pieces(&layers, pl, target, *exact, full, &mut out);
                }
                out
            },
        );
        let mut pending = Vec::new();
        collect(outs, &mut pending);

        let mut g = 2 * h;
        while !pending.is_empty() {
            let mut next = Vec::new();
            for &pl in &piece_layers {
                let seeds: Vec<&Region> = pending.iter().filter(|p| p.0 == pl).map(|p| &p.1).collect();
                if seeds.is_empty() {
                    continue;
                }
                let boxes: Vec<IRect> = seeds.iter().map(|s| s.bbox().unwrap_or_default()).collect();
                let used = used_layers(&self.deck, self.n, rules.iter().copied().filter(|&k| self.scope[k] == Scope::Piece(pl)));
                let groups: Vec<(Region, IRect)> = group_boxes(&boxes, g)
                    .iter()
                    .map(|grp| {
                        let target = Region::from_rects(grp.iter().flat_map(|&i| seeds[i].rects().iter().copied()));
                        let exact = grown(target.bbox().unwrap_or_default(), g);
                        (target, exact)
                    })
                    .collect();
                let outs = par_windows(
                    top,
                    src,
                    max_depth,
                    &groups,
                    |w| grown(w.1, h),
                    |(target, exact), flat| {
                        let layers = layers_of(&self.deck, self.n, flat, &used, false);
                        let mut out = Found::default();
                        self.pieces(&layers, pl, target, *exact, full, &mut out);
                        out
                    },
                );
                collect(outs, &mut next);
            }
            pending = next;
            g *= 2;
        }

        let mask = (0..nr)
            .map(|k| match self.scope[k] {
                Scope::Piece(l) if self.area_only(l).is_some() => Mask::Pieces(z.clone()),
                Scope::Piece(l) => {
                    Mask::Cut(z.boolean(&Region::from_rects(found.get(&l).into_iter().flatten().copied()), BoolOp::Or))
                }
                _ => Mask::Cut(z.clone()),
            })
            .collect();
        Zone { mask, res: res.into_iter().map(Region::from_rects).collect() }
    }

    /// The largest minimum area when every piece rule on layer `pl` is an `area` rule. Then a
    /// piece is settled as soon as a known part of it is that large.
    fn area_only(&self, pl: usize) -> Option<i128> {
        let mut min: Option<i128> = None;
        for (k, s) in self.scope.iter().enumerate() {
            if *s == Scope::Piece(pl) {
                let Rule::Area { min: m, .. } = self.deck.rules[k].rule else { return None };
                min = Some(min.map_or(m, |x| x.max(m)));
            }
        }
        min
    }

    /// Pieces of layer `pl` overlapping `target`, computed in a window whose layers are exact
    /// within `exact`: complete ones are checked by the piece rules on `pl`, the others are
    /// left `pending` (their known part) for a wider window.
    fn pieces(&self, layers: &Layers, pl: usize, target: &Region, exact: IRect, full: IRect, out: &mut Found) {
        let Some(l) = layers.regions.get(pl) else { return };
        let (hit, _) = split_overlapping(l, target);
        if hit.is_empty() {
            return;
        }
        let all_known = covers(&exact, &full);
        let inner = grown(exact, -1);
        let (mut done, mut open) = (Vec::new(), Vec::new());
        for p in hit.parts() {
            let complete = all_known || p.bbox().is_some_and(|b| covers(&inner, &b));
            if complete { &mut done } else { &mut open }.extend_from_slice(p.rects());
        }
        if !open.is_empty() {
            let known = Region::from_rects(open).boolean(&Region::from_rects([exact]), BoolOp::And);
            let (mut seed, _) = split_overlapping(&known, target);
            if let Some(min) = self.area_only(pl) {
                seed = Region::from_rects(seed.parts().into_iter().filter(|p| p.area() < min).flat_map(|p| p.rects().to_vec()));
            }
            if !seed.is_empty() {
                out.pending.push((pl, seed));
            }
        }
        if done.is_empty() {
            return;
        }
        let done = Region::from_rects(done);
        let empty = Region::default();
        let layer = |l: usize| layers.regions.get(l).unwrap_or(&empty);
        for k in (0..self.deck.rules.len()).filter(|&k| self.scope[k] == Scope::Piece(pl)) {
            out.res.push((k, check(&self.deck.rules[k].rule, &layer, None).boolean(&done, BoolOp::And)));
        }
        out.pieces.push((pl, done));
    }
}

/// Incremental DRC of a cell: keeps the violation pieces of the last check and, given the
/// areas changed since, re-checks only the halo around them ([`HierDrc`] zones). Placed cells
/// are checked once and cached; build a new session when the deck or a placed cell changes.
pub struct DrcSession {
    hier: HierDrc,
    /// Violation pieces of every rule, in the order [`Region::parts`] gives them.
    parts: Vec<Vec<Region>>,
    report: DrcReport,
}

fn part_key(p: &Region) -> (i64, i64) {
    p.rects().first().map_or((i64::MIN, i64::MIN), |r| (r.y0, r.x0))
}

/// `parts` with the area `mask` replaced by `res`: only the pieces near either are rebuilt.
fn splice(parts: Vec<Region>, mask: &Mask, res: &Region) -> Vec<Region> {
    let near = |p: &Region, a: Option<IRect>| match (p.bbox(), a) {
        (Some(b), Some(a)) => grown(b, 1).intersects(&a),
        _ => false,
    };
    let (m, r) = (mask.bbox(), res.bbox());
    let (hit, mut keep): (Vec<Region>, Vec<Region>) = parts.into_iter().partition(|p| near(p, m) || near(p, r));
    if hit.is_empty() && res.is_empty() {
        return keep;
    }
    let local = Region::from_rects(hit.iter().flat_map(|p| p.rects().iter().copied()));
    keep.extend(mask.apply(&local).boolean(res, BoolOp::Or).parts());
    keep.sort_by_key(part_key);
    keep
}

impl DrcSession {
    /// Check `top` in full (hierarchically).
    pub fn new(deck: &Deck, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Self {
        let start = Instant::now();
        let mut hier = HierDrc::new(deck, tech);
        let (regions, shapes, skipped) = hier.regions(top, src);
        let mut s = Self { hier, parts: par_map(&regions, Region::parts), report: DrcReport::default() };
        s.publish(tech, shapes, skipped, start);
        s
    }

    pub fn deck(&self) -> &Deck {
        self.hier.deck()
    }

    pub fn report(&self) -> &DrcReport {
        &self.report
    }

    /// Re-check after edits of `top`. `dirty` must cover every changed shape and placement,
    /// before and after the change.
    pub fn update(&mut self, top: &Layout, src: &dyn LayoutSource, tech: &Tech, dirty: &[IRect]) -> &DrcReport {
        if dirty.is_empty() {
            return &self.report;
        }
        let start = Instant::now();
        let h = self.hier.halo;
        let z = Region::from_rects(dirty.iter().map(|d| grown(*d, h)));
        let extent = flat_bbox(top, src, MAX_DEPTH, &mut CellBoxes::new());
        let (shapes, skipped) = match (self.hier.counts(top, src), extent) {
            (Some((shapes, skipped)), Some(extent)) if z.area() * 2 <= rect_area(&grown(extent, h)) => {
                let zone = self.hier.zone(top, src, &z, extent, 0);
                let local = self.hier.rules(|s| s != Scope::Flat);
                for k in local {
                    let old = std::mem::take(&mut self.parts[k]);
                    self.parts[k] = splice(old, &zone.mask[k], &zone.res[k]);
                }
                let flat = self.hier.rules(|s| s == Scope::Flat);
                if !flat.is_empty() {
                    let regions = self.hier.fill_flat(vec![Region::default(); self.parts.len()], top, src);
                    for k in flat {
                        self.parts[k] = regions[k].parts();
                    }
                }
                (shapes, skipped)
            }
            _ => {
                let (regions, shapes, skipped) = self.hier.regions(top, src);
                self.parts = par_map(&regions, Region::parts);
                (shapes, skipped)
            }
        };
        self.publish(tech, shapes, skipped, start);
        &self.report
    }

    fn publish(&mut self, tech: &Tech, shapes: usize, skipped: usize, start: Instant) {
        self.report = report_parts(&self.hier.deck, tech, &self.parts, None);
        (self.report.shapes, self.report.skipped) = (shapes, skipped);
        self.report.millis = start.elapsed().as_secs_f64() * 1000.0;
    }
}

fn run_shapes(deck: &Deck, flat: Vec<FlatShape>, tech: &Tech, window: Option<IRect>) -> DrcReport {
    let start = Instant::now();
    let n = tech.layers.len();
    let used = used_layers(deck, n, 0..deck.rules.len());
    let layers = layers_of(deck, n, &flat, &used, true);
    let empty = Region::default();
    let layer = |l: usize| layers.regions.get(l).unwrap_or(&empty);
    let results = par_map(&deck.rules, |nr| check(&nr.rule, &layer, layers.extent));
    let mut report = report_of(deck, tech, &results, window);
    (report.shapes, report.skipped) = (layers.shapes, layers.skipped);
    report.millis = start.elapsed().as_secs_f64() * 1000.0;
    report
}

/// Which layers (technology, then derived) the rules `rules` read, directly or through derived layers.
fn used_layers(deck: &Deck, n: usize, rules: impl IntoIterator<Item = usize>) -> Vec<bool> {
    let total = n + deck.derived.len();
    let mut used = vec![false; total];
    for l in rules.into_iter().flat_map(|k| deck.rules[k].rule.layers()) {
        if l < total {
            used[l] = true;
        }
    }
    for (k, d) in deck.derived.iter().enumerate().rev() {
        if used[n + k] {
            for l in d.expr.inputs().into_iter().filter(|&l| l < total) {
                used[l] = true;
            }
        }
    }
    used
}

/// Merged layers of flattened shapes (unused ones empty) and the shape counts.
struct Layers {
    regions: Vec<Region>,
    shapes: usize,
    skipped: usize,
    extent: Option<IRect>,
}

fn layers_of(deck: &Deck, n: usize, flat: &[FlatShape], used: &[bool], parallel: bool) -> Layers {
    let mut rects: Vec<Vec<IRect>> = vec![Vec::new(); n];
    let (mut shapes, mut skipped) = (0, 0);
    let mut extent: Option<IRect> = None;
    for f in flat {
        if f.layer >= n || matches!(f.geom, Geometry::Label { .. }) {
            continue;
        }
        shapes += 1;
        let b = f.geom.bbox();
        extent = Some(extent.map_or(b, |e| e.union(&b)));
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
    let merge = |r: &Vec<IRect>| Region::from_rects(r.iter().copied());
    let mut regions = if parallel { par_map(&rects, merge) } else { rects.iter().map(merge).collect() };
    for (k, d) in deck.derived.iter().enumerate() {
        let r = if used[n + k] { d.expr.eval(&regions, extent) } else { Region::default() };
        regions.push(r);
    }
    Layers { regions, shapes, skipped, extent }
}

/// Violations of the rule regions `results` (one per deck rule), only those touching `window`.
fn report_of(deck: &Deck, tech: &Tech, results: &[Region], window: Option<IRect>) -> DrcReport {
    report_parts(deck, tech, &par_map(results, Region::parts), window)
}

/// [`report_of`] from the pieces of every rule's region.
fn report_parts(deck: &Deck, tech: &Tech, results: &[Vec<Region>], window: Option<IRect>) -> DrcReport {
    let n = tech.layers.len();
    let names = deck.layer_names(tech);
    let mut report = DrcReport { rules_run: deck.rules.len(), ..Default::default() };
    for (nr, parts) in deck.rules.iter().zip(results) {
        if parts.len() > MAX_VIOLATIONS_PER_RULE {
            report.capped.push((nr.name.clone(), parts.len()));
        }
        let description = nr.comment.clone().unwrap_or_else(|| nr.rule.description(tech, &names));
        let checked = nr.rule.checked_layer();
        for p in parts.iter().take(MAX_VIOLATIONS_PER_RULE) {
            if let Some(marker) = p.bbox() {
                if window.is_some_and(|w| !(marker.x0 <= w.x1 && w.x0 <= marker.x1 && marker.y0 <= w.y1 && w.y0 <= marker.y1)) {
                    continue;
                }
                report.violations.push(Violation {
                    rule: nr.name.clone(),
                    description: description.clone(),
                    layer: (checked < n).then_some(checked),
                    marker,
                });
            }
        }
    }
    report
}

/// `f` on every group and its flattened window (flattened in chunks on this thread).
fn par_windows<G: Sync, R: Send>(
    top: &Layout,
    src: &dyn LayoutSource,
    max_depth: u32,
    groups: &[G],
    window: impl Fn(&G) -> IRect,
    f: impl Fn(&G, &[FlatShape]) -> R + Sync,
) -> Vec<R> {
    let mut out = Vec::with_capacity(groups.len());
    let mut boxes = CellBoxes::new();
    for chunk in groups.chunks(256) {
        let items: Vec<(&G, Vec<FlatShape>)> =
            chunk.iter().map(|g| (g, flatten_in_cached(top, src, max_depth, window(g), &mut boxes))).collect();
        out.extend(par_map(&items, |(g, flat)| f(g, flat)));
    }
    out
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
fn check<'a>(rule: &Rule, layer: &impl Fn(usize) -> &'a Region, extent: Option<IRect>) -> Region {
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
            euclidean(a.sized(min).boolean(&free, BoolOp::And), a, min)
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
        Rule::Antenna { layer: m, ratio, poly, active, ref vias } => antenna(layer, m, ratio, poly, active, vias),
        Rule::Density { layer: l, min, max, window, step } => {
            extent.map_or_else(Region::default, |e| density(layer(l), e, (min, max), window, step))
        }
        Rule::Flag { layer: l } => layer(l).clone(),
    }
}

/// Pieces of `a` selected by `how` against `b` (the others when `not`).
fn select(a: &Region, b: &Region, how: Select, not: bool) -> Region {
    let tree = RTree::bulk_load(b.rects().iter().map(|&r| Boxed(r)).collect());
    let overlaps =
        |p: &Region| p.rects().iter().any(|r| tree.locate_in_envelope_intersecting(aabb(*r)).any(|q| q.0.intersects(r)));
    let touches = |p: &Region| {
        p.rects().iter().any(|r| {
            let g = IRect::new(r.x0 - 1, r.y0 - 1, r.x1 + 1, r.y1 + 1);
            tree.locate_in_envelope_intersecting(aabb(g)).any(|q| q.0.intersects(&g))
        })
    };
    let b_parts = if how == Select::Enclose { b.parts() } else { Vec::new() };
    let keep = |p: &Region| match how {
        Select::Interact => overlaps(p) || touches(p),
        Select::Inside => !p.is_empty() && p.boolean(b, BoolOp::Not).is_empty(),
        Select::Outside => !overlaps(p),
        Select::Touch => !overlaps(p) && touches(p),
        Select::Enclose => {
            let bb = p.bbox();
            b_parts.iter().any(|q| match (bb, q.bbox()) {
                (Some(o), Some(i)) if o.x0 <= i.x0 && o.y0 <= i.y0 && o.x1 >= i.x1 && o.y1 >= i.y1 => {
                    q.boolean(p, BoolOp::Not).is_empty()
                }
                _ => false,
            })
        }
    };
    Region::from_rects(a.parts().into_iter().filter(|p| keep(p) != not).flat_map(|p| p.rects().to_vec()))
}

/// The holes of every piece of `a`: enclosed empty areas.
fn holes(a: &Region) -> Region {
    let mut out = Vec::new();
    for p in a.parts() {
        let Some(bb) = p.bbox() else { continue };
        let frame = Region::from_rects([IRect::new(bb.x0 - 1, bb.y0 - 1, bb.x1 + 1, bb.y1 + 1)]).boolean(&p, BoolOp::Not);
        for gap in frame.parts() {
            if gap.bbox().is_some_and(|g| g.x0 >= bb.x0 && g.y0 >= bb.y0 && g.x1 <= bb.x1 && g.y1 <= bb.y1) {
                out.extend_from_slice(gap.rects());
            }
        }
    }
    Region::from_rects(out)
}

fn dist2(a: &IRect, b: &IRect) -> i128 {
    let dx = (b.x0 - a.x1).max(a.x0 - b.x1).max(0) as i128;
    let dy = (b.y0 - a.y1).max(a.y0 - b.y1).max(0) as i128;
    dx * dx + dy * dy
}

/// The pieces of `v` closer than `min` to `a` in the Euclidean metric.
fn euclidean(v: Region, a: &Region, min: i64) -> Region {
    if v.is_empty() {
        return v;
    }
    let tree = RTree::bulk_load(a.rects().iter().map(|&r| Boxed(r)).collect());
    let limit = min as i128 * min as i128;
    let near = |p: &Region| {
        p.rects().iter().any(|r| {
            let q = IRect::new(r.x0 - min, r.y0 - min, r.x1 + min, r.y1 + min);
            tree.locate_in_envelope_intersecting(aabb(q)).any(|b| dist2(r, &b.0) < limit)
        })
    };
    Region::from_rects(v.parts().into_iter().filter(near).flat_map(|p| p.rects().to_vec()))
}

struct Tagged(IRect, usize);

impl RTreeObject for Tagged {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        aabb(self.0)
    }
}

fn find(uf: &mut [usize], mut i: usize) -> usize {
    while uf[i] != i {
        uf[i] = uf[uf[i]];
        i = uf[i];
    }
    i
}

/// Metal `m` pieces of the gate nets (up to `m`) that are not tied to diffusion and whose metal
/// to gate area ratio exceeds `ratio`.
fn antenna<'a>(
    layer: &impl Fn(usize) -> &'a Region,
    m: usize,
    ratio: f64,
    poly: usize,
    active: usize,
    vias: &[(usize, usize, usize)],
) -> Region {
    let (p, a) = (layer(poly), layer(active));
    let gate = p.boolean(a, BoolOp::And);
    if gate.is_empty() {
        return Region::default();
    }
    let mut conds: Vec<(usize, Region)> = vec![(poly, p.clone()), (active, a.boolean(p, BoolOp::Not))];
    for l in vias.iter().flat_map(|&(b, _, t)| [b, t]) {
        if !conds.iter().any(|c| c.0 == l) {
            conds.push((l, layer(l).clone()));
        }
    }
    let mut pieces: Vec<(usize, Region)> = Vec::new();
    let mut trees: Vec<RTree<Tagged>> = Vec::new();
    for (ci, (_, r)) in conds.iter().enumerate() {
        let mut items = Vec::new();
        for part in r.parts() {
            items.extend(part.rects().iter().map(|&q| Tagged(q, pieces.len())));
            pieces.push((ci, part));
        }
        trees.push(RTree::bulk_load(items));
    }
    let ci_of = |l: usize| conds.iter().position(|c| c.0 == l);
    let hits = |ci: usize, r: &IRect| -> Vec<usize> {
        trees[ci].locate_in_envelope_intersecting(aabb(*r)).filter(|t| t.0.intersects(r)).map(|t| t.1).collect()
    };
    let mut uf: Vec<usize> = (0..pieces.len()).collect();
    for &(b, c, t) in vias {
        let (Some(bi), Some(ti)) = (ci_of(b), ci_of(t)) else { continue };
        let cut = if b == poly { layer(c).boolean(a, BoolOp::Not) } else { layer(c).clone() };
        for part in cut.parts() {
            let (mut lo, mut hi) = (Vec::new(), Vec::new());
            for r in part.rects() {
                lo.extend(hits(bi, r));
                hi.extend(hits(ti, r));
            }
            if let (Some(&first), false) = (lo.first(), hi.is_empty()) {
                for &o in lo.iter().chain(&hi) {
                    let (x, y) = (find(&mut uf, first), find(&mut uf, o));
                    uf[x.max(y)] = x.min(y);
                }
            }
        }
    }
    let n = pieces.len();
    let (mut gate_area, mut metal_area, mut tied) = (vec![0i128; n], vec![0i128; n], vec![false; n]);
    for g in gate.parts() {
        if let Some(pi) = g.rects().first().and_then(|r| hits(0, r).first().copied()) {
            let root = find(&mut uf, pi);
            gate_area[root] += g.area();
        }
    }
    let mi = ci_of(m);
    for (i, (ci, piece)) in pieces.iter().enumerate() {
        let root = find(&mut uf, i);
        match *ci {
            1 => tied[root] = true,
            ci if Some(ci) == mi => metal_area[root] += piece.area(),
            _ => {}
        }
    }
    let bad = |root: usize| !tied[root] && gate_area[root] > 0 && metal_area[root] as f64 > ratio * gate_area[root] as f64;
    let mut out = Vec::new();
    for (i, (ci, piece)) in pieces.iter().enumerate() {
        if Some(*ci) == mi && bad(find(&mut uf, i)) {
            out.extend_from_slice(piece.rects());
        }
    }
    Region::from_rects(out)
}

/// Windows of `extent` (clipped to it) where `r` covers less than `min` or more than `max`
/// percent.
fn density(r: &Region, extent: IRect, (min, max): (f64, f64), window: i64, step: i64) -> Region {
    let spans = |lo: i64, hi: i64| {
        let w = window.min(hi - lo);
        let mut v = Vec::new();
        if w <= 0 {
            return v;
        }
        let mut s = lo;
        loop {
            let s0 = s.min(hi - w);
            v.push((s0, s0 + w));
            if s0 + w >= hi {
                break;
            }
            s += step;
        }
        v
    };
    let tree = RTree::bulk_load(r.rects().iter().map(|&q| Boxed(q)).collect());
    let mut out = Vec::new();
    for &(x0, x1) in &spans(extent.x0, extent.x1) {
        for &(y0, y1) in &spans(extent.y0, extent.y1) {
            let w = IRect::new(x0, y0, x1, y1);
            let covered: i128 = tree
                .locate_in_envelope_intersecting(aabb(w))
                .filter(|b| b.0.intersects(&w))
                .map(|b| {
                    let o = IRect::new(b.0.x0.max(x0), b.0.y0.max(y0), b.0.x1.min(x1), b.0.y1.min(y1));
                    o.width() as i128 * o.height() as i128
                })
                .sum();
            let pct = 100.0 * covered as f64 / (w.width() as f64 * w.height() as f64);
            if pct < min || pct > max {
                out.push(w);
            }
        }
    }
    Region::from_rects(out)
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
                let dx = (if sx > 0 { c.x0 - cx } else { cx - c.x1 }) as i128;
                let dy = (if sy > 0 { c.y0 - cy } else { cy - c.y1 }) as i128;
                dx * dx + dy * dy
            };
            let nearest = tree
                .locate_in_envelope_intersecting(aabb(q))
                .filter(|b| b.0.intersects(&q))
                .map(|b| IRect::new(b.0.x0.max(q.x0), b.0.y0.max(q.y0), b.0.x1.min(q.x1), b.0.y1.min(q.y1)))
                .min_by_key(reach);
            let Some(c) = nearest.filter(|c| reach(c) < min as i128 * min as i128) else { continue };
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
    use super::*;
    use crate::hier::NoCells;
    use crate::layout::ArraySpec;
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
    fn window_matches_full_run_locally() {
        let t = tech();
        let deck = parse_deck("width metal1 0.23\nspace metal1 0.23\narea metal1 0.2\ndensity metal1 99 100 10", &t).unwrap();
        // A long bar crossing the window must not look narrow or small where the halo cuts it.
        let mut lay = layout(&[
            ("metal1", (0, 0, 100_000, 1000)),
            ("metal1", (2000, 2000, 2200, 4000)),
            ("metal1", (2400, 2000, 3000, 4000)),
            ("metal1", (90_000, 5000, 90_100, 9000)),
        ]);
        lay.add(li("metal1"), Geometry::Rect(r(50_000, 5000, 50_100, 5100)));
        let full: Vec<IRect> =
            run(&deck, &lay, &NoCells, &t).violations.into_iter().filter(|v| !v.rule.contains("DEN")).map(|v| v.marker).collect();
        assert_eq!(full.len(), 5, "{full:?}");
        let w = r(1000, 1000, 5000, 5000);
        let local = run_window(&deck, &lay, &NoCells, &t, w);
        assert!(local.violations.iter().all(|v| !v.rule.contains("DEN")), "density is global");
        let mut got: Vec<IRect> = local.violations.into_iter().map(|v| v.marker).collect();
        got.sort_by_key(|m| (m.x0, m.y0));
        let mut want: Vec<IRect> =
            full.into_iter().filter(|m| m.x0 <= w.x1 && w.x0 <= m.x1 && m.y0 <= w.y1 && w.y0 <= m.y1).collect();
        want.sort_by_key(|m| (m.x0, m.y0));
        assert_eq!(got, want);
        assert_eq!(got.len(), 2, "narrow bar and its gap");
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
    fn derived_layers() {
        let t = tech();
        let tee = layout(&[("active", (0, 0, 1500, 1000)), ("poly", (600, -220, 780, 1220))]);
        let rep = run(&parse_deck("gate = poly and active\nwidth gate 0.3", &t).unwrap(), &tee, &NoCells, &t);
        assert_eq!(rep.violations.len(), 1);
        let v = &rep.violations[0];
        assert_eq!((v.rule.as_str(), v.description.as_str()), ("gate.W", "gate width < 0.3 µm"));
        assert_eq!((v.marker, v.layer), (r(600, 0, 780, 1000), None));

        assert_eq!(markers("sd = ACTIVE NOT poly\narea sd 0.7", &tee), [r(0, 0, 600, 1000)]);
        assert_eq!(markers("x = poly xor active\narea x 0.05", &tee), [r(600, -220, 780, 0), r(600, 1000, 780, 1220)]);

        let pair = layout(&[("metal1", (0, 0, 500, 1000)), ("metal1", (900, 0, 1500, 1000))]);
        assert!(markers("space metal1 0.3", &pair).is_empty());
        assert_eq!(markers("big = metal1 size 0.1\nspace big 0.3", &pair), [r(600, -100, 800, 1100)]);
        let shrunk = "core = metal1 size -0.2\narea core 0.1";
        assert_eq!(markers(shrunk, &pair), [r(200, 200, 300, 800)]);
        let chain = "g = poly and active\ngg = g or metal1\nwide = gg size 0.1\nwidth wide 0.35";
        assert!(markers(chain, &tee).is_empty());
        assert_eq!(markers(chain, &layout(&[("metal1", (0, 0, 100, 5000))])), [r(-100, -100, 200, 5100)]);

        let deck = parse_deck("gate = poly and active\nwide = gate size -0.05\nwidth wide 1", &t).unwrap();
        let names = deck.layer_names(&t);
        let text: String = deck
            .derived
            .iter()
            .map(|d| d.deck_line(&t, &names))
            .chain(deck.rules.iter().map(|r| r.rule.deck_line(&t, &names)))
            .map(|l| l + "\n")
            .collect();
        assert_eq!(text, "gate = poly and active\nwide = gate size -0.05\nwidth wide 1\n");
        assert_eq!(parse_deck(&text, &t).unwrap(), deck);
        assert_eq!(deck.rules[0].rule, Rule::Width { layer: t.layers.len() + 1, min: 1000 });

        let bad = "width g 1\ng = poly and active\npoly = metal1 or metal2\nx = poly nand active\ny = poly size abc\nz = poly and\na b = poly and active\nw = nope or poly\nG = metal1 and metal2";
        let errs = parse_deck(bad, &t).unwrap_err();
        let got: Vec<(usize, &str)> = errs.iter().map(|e| (e.line, e.message.as_str())).collect();
        assert_eq!(
            got,
            [
                (1, "unknown layer 'g'"),
                (3, "layer 'poly' is already defined"),
                (
                    4,
                    "unknown operator 'nand' (expected and, or, not, xor, interact, inside, outside, enclose, touch, size or wide)"
                ),
                (5, "bad distance 'abc' (expected a value in µm)"),
                (
                    6,
                    "expected 'z = LAYER and|or|not|xor LAYER', 'z = LAYER [not] interact|inside|outside|enclose|touch LAYER', 'z = LAYER size|wide DIST', 'z = LAYER area MIN MAX' or 'z = holes|extents LAYER'"
                ),
                (7, "invalid layer name 'a b'"),
                (8, "unknown layer 'nope'"),
                (9, "layer 'G' is already defined"),
            ]
        );
    }

    #[test]
    fn derived_rules_on_demo_cells() {
        let p = crate::examples::demo_project();
        let deck = "gate = poly and active\nwidth gate 0.18\npdiff = active and pplus\nenclosure pdiff nwell 0.43\n\
                    ndiff = active and nplus\nseparation nwell ndiff 0.43\nsd = active not poly\narea sd 0.1";
        let d = parse_deck(deck, &p.tech).unwrap();
        for cell in ["cmos_inverter", "nand2", "nor2", "ring_osc5", "current_mirror", "ota5t"] {
            let lay = p.layout(&CellRef::new("work", cell)).unwrap();
            let rep = run(&d, lay, &p, &p.tech);
            let found: Vec<String> = rep.violations.iter().map(|v| format!("{} {:?}", v.rule, v.marker)).collect();
            assert!(found.is_empty(), "{cell}: {found:#?}");
        }
    }

    #[test]
    fn euclidean_corners() {
        let space = "space metal1 0.23";
        let diag = |dx: i64, dy: i64| layout(&[("metal1", (0, 0, 500, 500)), ("metal1", (500 + dx, 500 + dy, 1500, 1500))]);
        assert!(markers(space, &diag(170, 170)).is_empty(), "0.24 µm apart diagonally");
        assert_eq!(markers(space, &diag(160, 160)), [r(500, 500, 660, 660)]);
        let sep = "separation poly contact 0.16";
        let lay = |d: i64| layout(&[("poly", (0, 0, 180, 2000)), ("contact", (180 + d, 2000 + d, 400 + d, 2220 + d))]);
        assert!(markers(sep, &lay(120)).is_empty(), "0.17 µm apart diagonally");
        assert_eq!(markers(sep, &lay(100)).len(), 1);
    }

    #[test]
    fn wide_metal_spacing() {
        let deck = "m1w = metal1 wide 2\nM1.WS: separation m1w metal1 0.5";
        let lay = layout(&[
            ("metal1", (0, 0, 3000, 3000)),
            ("metal1", (0, 3000, 300, 5000)),
            ("metal1", (3400, 0, 3700, 3000)),
            ("metal1", (5000, 0, 5300, 3000)),
            ("metal1", (5600, 0, 5900, 3000)),
        ]);
        assert_eq!(markers(deck, &lay), [r(3400, 0, 3500, 3000)]);
        let t = tech();
        let d = parse_deck(deck, &t).unwrap();
        assert_eq!(d.derived[0].expr, LayerExpr::Wide { a: li("metal1"), min: 2000 });
        assert_eq!(d.derived[0].deck_line(&t, &d.layer_names(&t)), "m1w = metal1 wide 2");
        assert!(parse_deck("x = metal1 wide 0", &t).is_err());
    }

    /// A 0.18 x 1 µm gate with its poly pad contacted to a metal1 plate `plate` µm wide.
    fn antenna_cell(plate: i64) -> Layout {
        let mut l = layout(&[
            ("active", (0, 0, 1000, 1000)),
            ("poly", (400, -300, 580, 1300)),
            ("poly", (300, 1300, 700, 1700)),
            ("contact", (390, 1390, 610, 1610)),
        ]);
        l.add_rect(li("metal1"), 300, 1300, 300 + plate * 1000, 1300 + plate * 1000);
        l
    }

    #[test]
    fn antenna() {
        assert!(markers("antenna metal1 10", &antenna_cell(1)).is_empty(), "1 µm² / 0.18 µm² < 10");
        assert_eq!(markers("antenna metal1 10", &antenna_cell(2)), [r(300, 1300, 2300, 3300)]);
        let mut tied = antenna_cell(2);
        tied.add_rect(li("contact"), 100, 400, 320, 620);
        tied.add_rect(li("metal1"), 0, 300, 420, 1300);
        assert!(markers("antenna metal1 10", &tied).is_empty(), "a diffusion connection drains the charge");

        let mut m2 = antenna_cell(1);
        m2.add_rect(li("via1"), 370, 1370, 630, 1630);
        m2.add_rect(li("metal2"), 0, 1300, 3000, 4300);
        assert!(markers("antenna metal1 10", &m2).is_empty());
        assert_eq!(markers("antenna metal2 10", &m2), [r(0, 1300, 3000, 4300)]);

        let t = tech();
        let d = parse_deck("antenna metal2 400", &t).unwrap();
        assert_eq!(d.rules[0].name, "metal2.ANT");
        assert_eq!(d.rules[0].rule.description(&t, &d.layer_names(&t)), "metal2 antenna ratio > 400");
        let Rule::Antenna { ref vias, .. } = d.rules[0].rule else { panic!() };
        assert_eq!(vias.len(), 3, "contact, polycon and via1, not via2");
        for bad in ["antenna poly 10", "antenna text 10", "antenna metal1"] {
            assert!(parse_deck(bad, &t).is_err(), "{bad}");
        }
    }

    #[test]
    fn density() {
        let mut stripes = layout(&[("boundary", (0, 0, 20_000, 10_000))]);
        for i in 0..10 {
            stripes.add_rect(li("metal1"), i * 2000, 0, i * 2000 + 1000, 10_000);
        }
        assert!(markers("density metal1 40 60 10", &stripes).is_empty());
        assert_eq!(markers("density metal1 60 80 10", &stripes), [r(0, 0, 20_000, 10_000)]);
        assert!(markers("density metal2 0 10 10", &stripes).is_empty());
        assert_eq!(markers("density metal1 40 60 3 3", &stripes).len(), 1, "3 µm windows see 1/3 or 2/3 coverage");

        let t = tech();
        let d = parse_deck("density metal1 20 80 10", &t).unwrap();
        let names = d.layer_names(&t);
        assert_eq!(d.rules[0].rule.deck_line(&t, &names), "density metal1 20 80 10 5");
        assert_eq!(d.rules[0].name, "metal1.DEN");
        for bad in ["density metal1 80 20 10", "density metal1 20 80 0", "density metal1 20", "density metal1 20 120 10"] {
            assert!(parse_deck(bad, &t).is_err(), "{bad}");
        }
    }

    #[test]
    fn deck_parsing() {
        let t = tech();
        let d =
            parse_deck("M1.W: width metal1 0.23\n  # comment\nSPACE Metal1 0.25 # trailing\nenclosure contact active 0.1", &t)
                .unwrap();
        assert_eq!(d.rules.len(), 3);
        assert_eq!(
            d.rules[0],
            NamedRule { name: "M1.W".into(), rule: Rule::Width { layer: li("metal1"), min: 230 }, comment: None }
        );
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
        for n in ["metal1.ANT", "metal2.ANT", "metal3.ANT"] {
            assert!(names.contains(&n), "{n} missing in {names:?}");
        }
        assert!(deck.rules.contains(&NamedRule {
            name: "poly.EX.active".into(),
            rule: Rule::Extension { a: li("poly"), b: li("active"), min: 220 },
            comment: None
        }));
        let names = deck.layer_names(&t);
        let lines: String = deck.rules.iter().map(|r| r.rule.deck_line(&t, &names) + "\n").collect();
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

    const HIER_DECK: &str = "width metal1 0.23\nspace metal1 0.23\narea metal1 0.2\nenclosure contact metal1 0.05\n\
                             separation poly contact 0.16\nextension poly active 0.22\nm1w = metal1 wide 0.5\n\
                             separation m1w metal1 0.5\nbig = metal1 size 0.1\nspace big 0.3\n\
                             loose = contact not inside metal1\nflag loose\ndensity metal1 0 30 5";

    /// A 20 x 20 µm cell whose shapes only violate (or stop violating) once it is placed next
    /// to copies of itself: slivers and gaps at its left and right edges, a pad touching the top
    /// edge that joins the bottom rail of the cell above.
    fn leaf() -> Layout {
        layout(&[
            ("metal1", (0, 0, 20_000, 600)),
            ("metal1", (19_950, 6000, 20_000, 6300)),
            ("metal1", (0, 6000, 80, 6300)),
            ("metal1", (5000, 15_000, 5200, 15_100)),
            ("contact", (5050, 15_020, 5150, 15_080)),
            ("poly", (10_000, 4200, 10_180, 5400)),
            ("active", (9800, 4300, 10_500, 4700)),
            ("metal1", (12_000, 19_800, 12_200, 20_000)),
            ("metal1", (19_800, 12_000, 19_990, 12_200)),
            ("metal1", (100, 12_000, 300, 12_200)),
            ("metal1", (3000, 3000, 4000, 4000)),
            ("metal1", (4300, 3000, 4500, 4000)),
        ])
    }

    fn hier_design() -> (Layout, HashMap<CellRef, Layout>) {
        let (leaf_ref, mid_ref) = (CellRef::new("w", "leaf"), CellRef::new("w", "mid"));
        let mut mid = layout(&[("metal1", (0, 20_000, 40_100, 20_100))]);
        mid.add_instance(leaf_ref.clone(), Trans::IDENTITY, None);
        mid.add_instance(leaf_ref.clone(), Trans::new(20_100, 0), None);
        let mut src = HashMap::new();
        src.insert(leaf_ref.clone(), leaf());
        src.insert(mid_ref.clone(), mid);

        let mut top = layout(&[
            ("metal1", (85_000, -5000, 85_100, 65_000)),
            ("contact", (25_150, 35_020, 25_260, 35_080)),
            ("poly", (24_800, 34_900, 24_900, 35_300)),
            ("metal1", (140_000, 0, 140_100, 600)),
        ]);
        let array = ArraySpec { cols: 4, rows: 3, col: (20_000, 0), row: (0, 20_000) };
        top.add_instance(leaf_ref.clone(), Trans::IDENTITY, Some(array));
        top.add_instance(leaf_ref.clone(), Trans::with(90_000, 10_000, 1, false), None);
        top.add_instance(leaf_ref.clone(), Trans::with(160_000, 0, 0, true), None);
        top.add_instance(leaf_ref.clone(), Trans::with(160_000, 40_000, 2, false), None);
        top.add_instance(mid_ref.clone(), Trans::new(0, 100_000), None);
        let pair = ArraySpec { cols: 2, rows: 1, col: (45_000, 0), row: (0, 0) };
        top.add_instance(mid_ref, Trans::new(0, 200_000), Some(pair));
        (top, src)
    }

    fn assert_hier_matches(deck: &Deck, top: &Layout, src: &dyn LayoutSource, t: &Tech) -> (DrcReport, usize) {
        let flat = run(deck, top, src, t);
        let mut h = HierDrc::new(deck, t);
        let hier = h.run(top, src, t);
        assert_eq!(hier.violations, flat.violations);
        assert_eq!(
            (hier.shapes, hier.skipped, hier.rules_run, &hier.capped),
            (flat.shapes, flat.skipped, flat.rules_run, &flat.capped)
        );
        (flat, h.zoned)
    }

    #[test]
    fn hierarchical_equals_flat() {
        let t = tech();
        let deck = parse_deck(HIER_DECK, &t).unwrap();
        let (top, src) = hier_design();
        let (rep, zoned) = assert_hier_matches(&deck, &top, &src, &t);
        assert_eq!(zoned, 2, "mid and top are checked by their interaction zones");
        let has = |rule: &str, m: IRect| rep.violations.iter().any(|v| v.rule == rule && v.marker == m);
        assert!(has("metal1.W", r(19_950, 6000, 20_080, 6300)), "slivers of neighbours merge across the boundary");
        assert!(has("metal1.S", r(19_990, 12_000, 20_100, 12_200)), "a gap that exists only between placements");
        assert!(!has("metal1.A", r(12_000, 19_800, 12_200, 20_000)), "the pad joins the rail of the cell above");
        assert!(has("metal1.A", r(12_000, 59_800, 12_200, 60_000)), "the top row's pad stays small");
        assert!(has("poly.SEP.contact", r(25_050, 35_020, 25_060, 35_080)), "parent poly near a placed contact");
        assert!(rep.violations.iter().any(|v| v.rule == "loose.FLAG"));
        assert!(rep.violations.iter().any(|v| v.rule == "m1w.SEP.metal1"));

        let mut empty = Layout::default();
        assert_hier_matches(&deck, &empty, &src, &t);
        empty.add_instance(CellRef::new("w", "missing"), Trans::IDENTITY, None);
        assert_hier_matches(&deck, &empty, &src, &t);
        let mut tiny = HashMap::new();
        tiny.insert(CellRef::new("w", "leaf"), layout(&[("metal1", (0, 0, 100, 100))]));
        let mut stacked = Layout::default();
        for k in 0..3 {
            stacked.add_instance(CellRef::new("w", "leaf"), Trans::new(k * 50, 0), None);
        }
        assert_hier_matches(&deck, &stacked, &tiny, &t);
    }

    #[test]
    fn hierarchical_equals_flat_on_demo_cells() {
        let p = crate::examples::demo_project();
        let text = format!("{}\nspace metal1 0.6\nwidth poly 0.3\narea metal1 0.6\nspace nwell 3", p.tech.deck_text());
        let deck = parse_any(&text, &p.tech).unwrap().deck;
        let mut checked = 0;
        for lib in &p.libraries {
            for cell in lib.cells.iter().filter(|c| c.layout.as_ref().is_some_and(|l| !l.instances.is_empty())) {
                let (rep, _) = assert_hier_matches(&deck, cell.layout.as_ref().unwrap(), &p, &p.tech);
                checked += usize::from(!rep.violations.is_empty());
            }
        }
        assert!(checked > 0, "the strict deck finds violations in placed cells");
    }

    #[test]
    fn hierarchy_cycle_falls_back_to_flat() {
        let t = tech();
        let deck = parse_deck("width metal1 0.23", &t).unwrap();
        let a = CellRef::new("w", "a");
        let mut l = layout(&[("metal1", (0, 0, 100, 100))]);
        l.add_instance(a.clone(), Trans::new(10, 0), None);
        let mut src = HashMap::new();
        src.insert(a, l.clone());
        assert_hier_matches(&deck, &l, &src, &t);
    }

    #[test]
    fn incremental_equals_full() {
        let t = tech();
        let deck = parse_deck(HIER_DECK, &t).unwrap();
        let (mut top, src) = hier_design();
        let mut session = DrcSession::new(&deck, &top, &src, &t);
        assert_eq!(session.report().violations, run(&deck, &top, &src, &t).violations);
        let leaf_box = leaf().shapes_bbox().unwrap();
        let mut step = |top: &Layout, dirty: &[IRect], what: &str| {
            let full = run(&deck, top, &src, &t);
            let inc = session.update(top, &src, &t, dirty);
            assert_eq!(inc.violations, full.violations, "{what}");
            assert_eq!(inc.shapes, full.shapes, "{what}");
        };

        let bar = top.add_rect(li("metal1"), 20_050, 30_000, 20_150, 30_300);
        step(&top, &[top.shape(bar).unwrap().bbox()], "narrow bar between two placements");
        let before = top.shape(bar).unwrap().bbox();
        top.translate_item(bar, 15_000, 25_000);
        step(&top, &[before, top.shape(bar).unwrap().bbox()], "bar moved");
        let rail = top.shapes.iter().find(|s| s.bbox() == r(85_000, -5000, 85_100, 65_000)).unwrap().clone();
        top.remove(rail.id);
        step(&top, &[rail.bbox()], "wire across placements removed");
        let bridge = top.add_rect(li("metal1"), 0, 60_000, 80_000, 60_100);
        step(&top, &[top.shape(bridge).unwrap().bbox()], "top pads joined by a bridge");
        let inst = top.add_instance(CellRef::new("w", "leaf"), Trans::with(30_000, 90_000, 3, true), None);
        let ib = top.instance(inst).unwrap().bbox(leaf_box);
        step(&top, &[ib], "overlapping placement added");
        top.remove(inst);
        step(&top, &[ib], "placement removed");
        top.remove(bridge);
        step(&top, &[r(0, 60_000, 80_000, 60_100)], "bridge removed");
        step(&top, &[], "no change");
        step(&top, &[r(-500_000, -500_000, 500_000, 500_000)], "everything dirty");
    }

    #[test]
    #[ignore]
    fn bench_hier_and_incremental() {
        let t = tech();
        let leaf_ref = CellRef::new("w", "leaf");
        let mut src = HashMap::new();
        let mut cell = leaf();
        for k in 0..20 {
            cell.add_rect(li("metal2"), 1000 + k * 900, 4000, 1300 + k * 900, 16_000);
        }
        src.insert(leaf_ref.clone(), cell);
        let mut base = Layout::default();
        let array = ArraySpec { cols: 60, rows: 60, col: (20_000, 0), row: (0, 20_000) };
        base.add_instance(leaf_ref, Trans::IDENTITY, Some(array));
        let time = |f: &mut dyn FnMut() -> DrcReport| {
            let s = Instant::now();
            let rep = f();
            (rep, s.elapsed().as_secs_f64() * 1000.0)
        };
        let typical = "width metal1 0.23\nspace metal1 0.23\narea metal1 0.2\nenclosure contact metal1 0.05\n\
                       separation poly contact 0.16\nextension poly active 0.22\nwidth metal2 0.28\nspace metal2 0.28";
        for (name, text) in [("typical", typical), ("derived", HIER_DECK.split("loose").next().unwrap())] {
            let deck = parse_deck(text, &t).unwrap();
            let mut top = base.clone();
            let (flat, flat_ms) = time(&mut || run(&deck, &top, &src, &t));
            let (hier, hier_ms) = time(&mut || run_hier(&deck, &top, &src, &t));
            assert_eq!(hier.violations, flat.violations);
            let mut session = DrcSession::new(&deck, &top, &src, &t);
            let bar = top.add_rect(li("metal1"), 610_050, 610_000, 610_150, 614_000);
            let dirty = [top.shape(bar).unwrap().bbox()];
            let (inc, inc_ms) = time(&mut || session.update(&top, &src, &t, &dirty).clone());
            let (full, full_ms) = time(&mut || run(&deck, &top, &src, &t));
            assert_eq!(inc.violations, full.violations);
            println!(
                "{name}: {} shapes, {} violations: flat {flat_ms:.0} ms, hierarchical {hier_ms:.0} ms ({:.1}x), incremental edit {inc_ms:.1} ms vs flat {full_ms:.0} ms ({:.0}x)",
                flat.shapes,
                flat.violations.len(),
                flat_ms / hier_ms,
                full_ms / inc_ms
            );
        }
    }
    #[test]
    fn selections_holes_extents_area_and_flag() {
        // metal1: A overlaps poly, B touches it, C is inside poly, D is far away; poly has a
        // ring with a hole around E (on metal2).
        let lay = layout(&[
            ("poly", (0, 0, 1000, 1000)),
            ("metal1", (900, 100, 1500, 300)),
            ("metal1", (1000, 500, 1500, 700)),
            ("metal1", (200, 200, 400, 400)),
            ("metal1", (5000, 0, 5100, 100)),
            ("poly", (3000, 0, 4000, 200)),
            ("poly", (3000, 800, 4000, 1000)),
            ("poly", (3000, 200, 3200, 800)),
            ("poly", (3800, 200, 4000, 800)),
            ("metal2", (3400, 400, 3600, 600)),
        ]);
        let sel = |how: &str| {
            let mut m = markers(
                &format!(
                    "x = metal1 {how} poly
flag x"
                ),
                &lay,
            );
            m.sort_by_key(|r| (r.x0, r.y0));
            m
        };
        assert_eq!(sel("interact"), [r(200, 200, 400, 400), r(900, 100, 1500, 300), r(1000, 500, 1500, 700)]);
        assert_eq!(sel("not interact"), [r(5000, 0, 5100, 100)]);
        assert_eq!(sel("inside"), [r(200, 200, 400, 400)]);
        assert_eq!(sel("outside"), [r(1000, 500, 1500, 700), r(5000, 0, 5100, 100)]);
        assert_eq!(sel("touch"), [r(1000, 500, 1500, 700)]);
        assert_eq!(
            markers(
                "x = poly enclose metal1
flag x",
                &lay
            ),
            [r(0, 0, 1000, 1000)]
        );
        assert_eq!(
            markers(
                "h = holes poly
flag h",
                &lay
            ),
            [r(3200, 200, 3800, 800)]
        );
        assert_eq!(
            markers(
                "h = holes poly
x = h enclose metal2
flag x",
                &lay
            ),
            [r(3200, 200, 3800, 800)]
        );
        assert_eq!(
            markers(
                "e = extents metal1
flag e",
                &lay
            )
            .len(),
            4
        );
        assert_eq!(
            markers(
                "s = metal1 area 0 0.02
flag s",
                &lay
            ),
            [r(5000, 0, 5100, 100)]
        );
        assert_eq!(
            markers(
                "s = metal1 area 0.1 inf
flag s",
                &lay
            )
            .len(),
            2
        );
        let t = tech();
        let d = parse_deck(
            "x = metal1 not inside poly
h = holes poly
flag x",
            &t,
        )
        .unwrap();
        let names = d.layer_names(&t);
        assert_eq!(d.derived[0].deck_line(&t, &names), "x = metal1 not inside poly");
        assert_eq!(d.derived[1].deck_line(&t, &names), "h = holes poly");
        assert_eq!(d.rules[0].rule.deck_line(&t, &names), "flag x");
        assert_eq!(d.rules[0].name, "x.FLAG");
    }
}
