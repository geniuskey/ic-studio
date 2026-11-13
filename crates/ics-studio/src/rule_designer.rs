//! DRC Rule Designer: a sub-tool window to build a rule deck visually. Rules and derived
//! layers are edited as forms, every rule is self-tested on generated pass / fail patterns
//! with the real DRC engine, a coverage matrix shows which checks each layer has, and the
//! deck can be linted, checked on the active cell and written back to the technology.
//! Without a project it edits deck files against a generic or PDK technology.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use ics_core::drc::{self, Deck, DerivedLayer, LayerExpr, NamedRule, Rule, Select};
use ics_core::region::{BoolOp, Region};
use ics_core::rule_design::{self, Finding, PatternRun, Severity, Subject};
use ics_core::{CellRef, IRect, Project, Tech};

use crate::layout_view::layer_color;
use crate::theme::{self, Palette};

/// What the designer asks the app to do.
pub enum DesignerAction {
    /// Store this deck text in the technology.
    Apply(String),
    /// Store it and run DRC on the active cell.
    ApplyAndRun(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sel {
    None,
    Rule(usize),
    Derived(usize),
    /// The gallery for adding a rule.
    Add,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Guide,
    Design,
    Coverage,
    Text,
    Issues,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Width,
    Space,
    Separation,
    Enclosure,
    Extension,
    Area,
    Antenna,
    Density,
    Flag,
}

const KINDS: [Kind; 9] = [
    Kind::Width,
    Kind::Space,
    Kind::Area,
    Kind::Enclosure,
    Kind::Separation,
    Kind::Extension,
    Kind::Density,
    Kind::Antenna,
    Kind::Flag,
];

impl Kind {
    fn of(r: &Rule) -> Kind {
        match r {
            Rule::Width { .. } => Kind::Width,
            Rule::Space { .. } => Kind::Space,
            Rule::Separation { .. } => Kind::Separation,
            Rule::Enclosure { .. } => Kind::Enclosure,
            Rule::Extension { .. } => Kind::Extension,
            Rule::Area { .. } => Kind::Area,
            Rule::Antenna { .. } => Kind::Antenna,
            Rule::Density { .. } => Kind::Density,
            Rule::Flag { .. } => Kind::Flag,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Width => "Width",
            Kind::Space => "Space",
            Kind::Separation => "Separation",
            Kind::Enclosure => "Enclosure",
            Kind::Extension => "Extension",
            Kind::Area => "Area",
            Kind::Antenna => "Antenna",
            Kind::Density => "Density",
            Kind::Flag => "Flag",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Kind::Width => "W",
            Kind::Space => "S",
            Kind::Separation => "SEP",
            Kind::Enclosure => "EN",
            Kind::Extension => "EX",
            Kind::Area => "A",
            Kind::Antenna => "ANT",
            Kind::Density => "DEN",
            Kind::Flag => "FLAG",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Width => "Minimum width",
            Kind::Space => "Minimum spacing",
            Kind::Separation => "Spacing to another layer",
            Kind::Enclosure => "Enclosure",
            Kind::Extension => "Extension",
            Kind::Area => "Minimum area",
            Kind::Antenna => "Antenna ratio",
            Kind::Density => "Density",
            Kind::Flag => "Flag every shape",
        }
    }

    /// The question the check answers, in plain words.
    fn question(self) -> &'static str {
        match self {
            Kind::Width => "How narrow may a shape be?",
            Kind::Space => "How close may two shapes of the same layer be?",
            Kind::Separation => "How close may shapes of two different layers be?",
            Kind::Enclosure => "How far must one layer reach around another, e.g. metal around a via?",
            Kind::Extension => "How far must one layer stick out past another, e.g. poly past active?",
            Kind::Area => "How small may a single shape be?",
            Kind::Antenna => "Can a long wire damage a transistor gate during manufacturing?",
            Kind::Density => "How much of each region must the layer fill?",
            Kind::Flag => "Report every shape of a layer, e.g. a forbidden layer or a computed error layer.",
        }
    }

    fn explain(self) -> &'static str {
        match self {
            Kind::Width => "Every part of the layer must fit a MIN x MIN square. Narrow bars, necks and slivers are flagged.",
            Kind::Space => {
                "Facing edges of the layer (also within one shape: notches, holes) must be at least MIN apart; corner to corner gaps are measured diagonally."
            }
            Kind::Separation => {
                "Shapes of the second layer that do not overlap the first must stay MIN away from it. Overlapping pieces count as joined and are not checked."
            }
            Kind::Enclosure => {
                "Inner shapes that overlap the outer layer must lie inside it with a margin of at least MIN on every side."
            }
            Kind::Extension => {
                "Where the first layer crosses out of the second (e.g. a poly gate leaving active), it must continue for at least MIN beyond the edge."
            }
            Kind::Area => "Every connected piece of the layer must have an area of at least MIN.",
            Kind::Antenna => {
                "For nets not tied to diffusion, the metal area up to this layer divided by the gate area must not exceed RATIO."
            }
            Kind::Density => "The layer must cover between MIN % and MAX % of every WINDOW x WINDOW window, stepped by STEP.",
            Kind::Flag => "Every piece of the layer is reported. Combine with a derived layer to flag any condition.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Bool(BoolOp),
    Select(Select, bool),
    Size,
    Wide,
    Area,
    Holes,
    Extents,
}

const OPS: [Op; 19] = [
    Op::Bool(BoolOp::And),
    Op::Bool(BoolOp::Or),
    Op::Bool(BoolOp::Not),
    Op::Bool(BoolOp::Xor),
    Op::Select(Select::Interact, false),
    Op::Select(Select::Inside, false),
    Op::Select(Select::Outside, false),
    Op::Select(Select::Enclose, false),
    Op::Select(Select::Touch, false),
    Op::Select(Select::Interact, true),
    Op::Select(Select::Inside, true),
    Op::Select(Select::Outside, true),
    Op::Select(Select::Enclose, true),
    Op::Select(Select::Touch, true),
    Op::Size,
    Op::Wide,
    Op::Area,
    Op::Holes,
    Op::Extents,
];

impl Op {
    fn of(e: &LayerExpr) -> Option<Op> {
        Some(match e {
            LayerExpr::Bool { op, .. } => Op::Bool(*op),
            LayerExpr::Select { how, not, .. } => Op::Select(*how, *not),
            LayerExpr::Size { .. } => Op::Size,
            LayerExpr::Wide { .. } => Op::Wide,
            LayerExpr::AreaRange { .. } => Op::Area,
            LayerExpr::Holes { .. } => Op::Holes,
            LayerExpr::Extents { .. } => Op::Extents,
            LayerExpr::Union(_) | LayerExpr::Check(_) => return None,
        })
    }

    fn label(self) -> String {
        match self {
            Op::Bool(BoolOp::And) => "A and B (overlap)".into(),
            Op::Bool(BoolOp::Or) => "A or B (merge)".into(),
            Op::Bool(BoolOp::Not) => "A not B (A minus B)".into(),
            Op::Bool(BoolOp::Xor) => "A xor B (either, not both)".into(),
            Op::Select(how, not) => {
                let what = match how {
                    Select::Interact => "interacting with",
                    Select::Inside => "inside",
                    Select::Outside => "outside",
                    Select::Enclose => "enclosing",
                    Select::Touch => "touching",
                };
                format!("pieces of A {}{what} B", if not { "not " } else { "" })
            }
            Op::Size => "A grown / shrunk by D".into(),
            Op::Wide => "parts of A at least D wide".into(),
            Op::Area => "pieces of A with MIN <= area < MAX".into(),
            Op::Holes => "holes of A".into(),
            Op::Extents => "bounding box of each piece of A".into(),
        }
    }

    fn binary(self) -> bool {
        matches!(self, Op::Bool(_) | Op::Select(..))
    }

    /// `e` turned into this operation, keeping its inputs.
    fn apply(self, e: &LayerExpr, tech: &Tech) -> LayerExpr {
        let ins = e.inputs();
        let a = ins.first().copied().unwrap_or(0);
        let b = ins.get(1).copied().unwrap_or(a);
        let g = tech.grid.max(1);
        match self {
            Op::Bool(op) => LayerExpr::Bool { a, op, b },
            Op::Select(how, not) => LayerExpr::Select { a, how, not, b },
            Op::Size => LayerExpr::Size { a, by: 20 * g },
            Op::Wide => LayerExpr::Wide { a, min: tech.dbu_per_um.max(g) },
            Op::Area => LayerExpr::AreaRange { a, min: Some(0), max: Some((tech.dbu_per_um as i128).pow(2)) },
            Op::Holes => LayerExpr::Holes { a },
            Op::Extents => LayerExpr::Extents { a },
        }
    }
}

struct CellCheck {
    gen_: u64,
    cell: CellRef,
    counts: Vec<usize>,
    total: usize,
    millis: f64,
}

struct CheckJob {
    gen_: u64,
    cell: CellRef,
    rules: Vec<String>,
    rx: Receiver<drc::DrcReport>,
}

pub struct RuleDesigner {
    pub open: bool,
    deck: Deck,
    /// Deck text when loaded or last applied, for the unsaved marker.
    baseline: String,
    /// Technology the deck was loaded for (name, layer count).
    tech_sig: (String, usize),
    notes: Vec<String>,
    sel: Sel,
    tab: Tab,
    filter: String,
    gen_: u64,
    lint_gen: u64,
    findings: Vec<Finding>,
    dirty: bool,
    text: String,
    text_gen: u64,
    text_edited: bool,
    text_errors: Vec<String>,
    runs: Option<(Rule, Vec<PatternRun>)>,
    check: Option<CellCheck>,
    job: Option<CheckJob>,
    status: Option<(bool, String)>,
    /// Layer picked in the add-rule gallery.
    add_layer: usize,
    /// Technology used when no project is open.
    own_tech: Option<Tech>,
    /// Deck file the draft was opened from or saved to.
    file: Option<PathBuf>,
}

/// A designer action that the toolbar and the guide share.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Reload,
    Defaults,
    FillGaps,
    FixAll,
    Check,
    Apply,
    ApplyRun,
    OpenDeck,
    SaveDeck,
    SaveDeckAs,
    LoadPdk,
    GenericTech,
}

impl Default for RuleDesigner {
    fn default() -> Self {
        RuleDesigner {
            open: false,
            deck: Deck::default(),
            baseline: String::new(),
            tech_sig: (String::new(), 0),
            notes: vec![],
            sel: Sel::None,
            tab: Tab::Guide,
            filter: String::new(),
            gen_: 1,
            lint_gen: 0,
            findings: vec![],
            dirty: false,
            text: String::new(),
            text_gen: 0,
            text_edited: false,
            text_errors: vec![],
            runs: None,
            check: None,
            job: None,
            status: None,
            add_layer: 0,
            own_tech: None,
            file: None,
        }
    }
}

struct Names<'a> {
    names: &'a [String],
    tech: &'a Tech,
    dark: bool,
}

fn sig(tech: &Tech) -> (String, usize) {
    (tech.name.clone(), tech.layers.len())
}

fn um(v: i64, tech: &Tech) -> String {
    crate::layout_view::um(v, tech)
}

fn um2(v: i128, tech: &Tech) -> String {
    let d = tech.dbu_per_um.max(1) as f64;
    let s = format!("{:.4}", v as f64 / (d * d));
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Display colour of layer `l` (technology layers use their own; derived layers get a
/// generated one).
fn color_of(l: usize, tech: &Tech, dark: bool) -> Color32 {
    match tech.layer(l) {
        Some(d) => layer_color(d, dark),
        None => {
            let [r, g, b, _] = ics_core::tech::auto_color(l * 3 + 1);
            Color32::from_rgb(r, g, b)
        }
    }
}

fn swatch(ui: &mut egui::Ui, c: Color32) {
    let (r, p) = ui.allocate_painter(Vec2::splat(10.0), Sense::hover());
    p.rect_filled(r.rect, 2.0, c);
}

fn chip(ui: &mut egui::Ui, text: &str, fill: Color32) -> egui::Response {
    let font = FontId::monospace(10.5);
    let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x + 8.0;
    let (r, p) = ui.allocate_painter(Vec2::new(w.max(26.0), 16.0), Sense::hover());
    p.rect_filled(r.rect, 3.0, fill);
    p.text(r.rect.center(), Align2::CENTER_CENTER, text, font, theme::on_fill(fill));
    r
}

/// The value the technology's layer and via definitions give for a rule's length, if any.
fn tech_value(r: &Rule, tech: &Tech) -> Option<i64> {
    let n = tech.layers.len();
    let name = |l: usize| tech.layer(l).map_or("", |d| d.name.as_str());
    let cut = |l: usize| tech.vias.iter().find(|v| !name(l).is_empty() && v.cut == name(l));
    let v = match *r {
        Rule::Width { layer, .. } if layer < n => tech.layers[layer].min_width.max(cut(layer).map_or(0, |c| c.cut_size)),
        Rule::Space { layer, .. } if layer < n => tech.layers[layer].min_spacing.max(cut(layer).map_or(0, |c| c.cut_spacing)),
        Rule::Enclosure { inner, outer, .. } if inner < n && outer < n => tech
            .vias
            .iter()
            .filter(|v| v.cut == name(inner))
            .find_map(|v| if v.bottom == name(outer) { Some(v.enc_bottom) } else { (v.top == name(outer)).then_some(v.enc_top) })
            .unwrap_or(0),
        Rule::Extension { a, b, .. } if Some(a) == tech.poly() && Some(b) == tech.diffusion() => tech.rules.poly_endcap,
        _ => 0,
    };
    (v > 0).then_some(v)
}

/// The checks a layer usually needs.
fn recommended(l: usize, tech: &Tech) -> Vec<Kind> {
    let Some(def) = tech.layer(l) else { return vec![] };
    let mut v = vec![Kind::Width, Kind::Space];
    if def.routing {
        v.push(Kind::Area);
    }
    if tech.vias.iter().any(|x| x.cut == def.name) {
        v.push(Kind::Enclosure);
    }
    if Some(l) == tech.poly() {
        v.push(Kind::Extension);
    }
    v
}

fn unique_layer_name(base: &str, names: &[String]) -> String {
    let free = |n: &str| !names.iter().any(|x| x.eq_ignore_ascii_case(n));
    if free(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base}{i}")).find(|n| free(n)).expect("unbounded")
}

/// The main value of a rule as a length (dbu), when it has one.
fn rule_len(r: &Rule) -> Option<i64> {
    match *r {
        Rule::Width { min, .. }
        | Rule::Space { min, .. }
        | Rule::Separation { min, .. }
        | Rule::Enclosure { min, .. }
        | Rule::Extension { min, .. } => Some(min),
        _ => None,
    }
}

fn rule_value(r: &Rule, tech: &Tech) -> String {
    match *r {
        Rule::Area { min, .. } => format!("{} µm²", um2(min, tech)),
        Rule::Antenna { ratio, .. } => format!("{ratio}"),
        Rule::Density { min, max, .. } => format!("{min}-{max} %"),
        Rule::Flag { .. } => String::new(),
        _ => format!("{} µm", um(rule_len(r).unwrap_or(0), tech)),
    }
}

fn other_layer(l: usize, tech: &Tech) -> usize {
    let n = tech.layers.len().max(1);
    if l + 1 < n { l + 1 } else { l.saturating_sub(1) }
}

/// A new rule of `kind` checking `layer`, with values taken from the technology.
fn new_rule(kind: Kind, layer: usize, tech: &Tech) -> Result<Rule, String> {
    let g = tech.grid.max(1);
    let def = tech.layer(layer);
    let name = def.map(|d| d.name.as_str()).unwrap_or_default();
    let cut = tech.vias.iter().find(|v| v.cut == name);
    let fallback = 10 * g;
    let pick = |v: i64| if v > 0 { v } else { fallback };
    Ok(match kind {
        Kind::Width => Rule::Width { layer, min: pick(def.map_or(0, |d| d.min_width).max(cut.map_or(0, |c| c.cut_size))) },
        Kind::Space => Rule::Space { layer, min: pick(def.map_or(0, |d| d.min_spacing).max(cut.map_or(0, |c| c.cut_spacing))) },
        Kind::Area => {
            let w = pick(def.map_or(0, |d| d.min_width)) as i128;
            Rule::Area { layer, min: def.map(|d| d.min_area as i128).filter(|a| *a > 0).unwrap_or(w * w * 2) }
        }
        Kind::Separation => {
            let a = other_layer(layer, tech);
            let s = tech.layer(a).map_or(0, |d| d.min_spacing).max(def.map_or(0, |d| d.min_spacing));
            Rule::Separation { a, b: layer, min: pick(s) }
        }
        Kind::Enclosure => match cut.and_then(|c| tech.layer_index(&c.bottom).map(|o| (o, c.enc_bottom))) {
            Some((outer, enc)) => Rule::Enclosure { inner: layer, outer, min: pick(enc) },
            None => Rule::Enclosure { inner: layer, outer: other_layer(layer, tech), min: fallback },
        },
        Kind::Extension => match (tech.poly(), tech.diffusion()) {
            (Some(p), Some(d)) if layer == p || layer == d => Rule::Extension { a: p, b: d, min: pick(tech.rules.poly_endcap) },
            _ => Rule::Extension { a: layer, b: other_layer(layer, tech), min: fallback },
        },
        Kind::Density => {
            let w = 100 * tech.dbu_per_um.max(1);
            Rule::Density { layer, min: 20.0, max: 80.0, window: w, step: w / 2 }
        }
        Kind::Antenna => {
            let metal = if tech.vias.iter().any(|v| v.top == name) {
                name.to_string()
            } else {
                tech.vias.iter().rev().map(|v| v.top.clone()).next().ok_or("antenna rules need a via stack")?
            };
            let d = drc::parse_deck(&format!("antenna {metal} 400"), tech).map_err(|e| e[0].message.clone())?;
            d.rules.into_iter().next().map(|r| r.rule).ok_or("antenna rule not created")?
        }
        Kind::Flag => Rule::Flag { layer },
    })
}

fn valid_layer_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

impl RuleDesigner {
    /// Open the window on the technology's current deck.
    pub fn show(&mut self, tech: &Tech) {
        if !self.open || self.tech_sig != sig(tech) {
            self.load(tech, &tech.deck_text());
        }
        self.open = true;
    }

    /// Open the window without a project, on the last standalone technology or the generic one.
    pub fn show_standalone(&mut self) {
        let tech = self.own_tech.take().unwrap_or_else(Tech::generic);
        if !self.open || self.tech_sig != sig(&tech) {
            match self.file.clone() {
                Some(path) => self.open_file(&tech, &path, true),
                None => self.load(&tech, &tech.deck_text()),
            }
        }
        self.own_tech = Some(tech);
        self.open = true;
    }

    /// Path of the deck file the draft was opened from or saved to.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn is_checking(&self) -> bool {
        self.job.is_some()
    }

    /// The deck being designed, as deck text.
    pub fn deck_text(&self, tech: &Tech) -> String {
        rule_design::deck_text(&self.deck, tech)
    }

    fn load(&mut self, tech: &Tech, text: &str) {
        let (deck, notes) = match drc::parse_any(text, tech) {
            Ok(p) => (p.deck, p.notes),
            Err(errs) => {
                let mut notes =
                    vec!["The technology deck has errors; started from the rules derived from the layers:".to_string()];
                notes.extend(errs.iter().map(|e| e.to_string()));
                (drc::parse_deck(&drc::default_deck_text(tech), tech).unwrap_or_default(), notes)
            }
        };
        self.set_deck(tech, deck, notes);
        self.baseline = rule_design::deck_text(&self.deck, tech);
    }

    fn set_deck(&mut self, tech: &Tech, deck: Deck, notes: Vec<String>) {
        self.deck = deck;
        self.notes = notes;
        self.tech_sig = sig(tech);
        self.sel = if self.deck.rules.is_empty() { Sel::None } else { Sel::Rule(0) };
        self.check = None;
        self.text_edited = false;
        self.text_errors.clear();
        self.touch();
    }

    /// Load deck file `path`. An SVRF deck is translated and not kept as the save target.
    fn open_file(&mut self, tech: &Tech, path: &Path, standalone: bool) {
        match drc::load_deck(path, tech) {
            Ok(p) => {
                let svrf = p.notes.iter().any(|n| n.starts_with("SVRF"));
                self.set_deck(tech, p.deck, p.notes);
                if standalone {
                    self.baseline = rule_design::deck_text(&self.deck, tech);
                }
                self.file = (!svrf).then(|| path.to_path_buf());
                let how = if svrf { " (translated from SVRF; Save as writes an IC Studio deck)" } else { "" };
                self.status = Some((true, format!("Opened {}{how}", path.display())));
            }
            Err(e) => self.status = Some((false, format!("Cannot open deck: {e}"))),
        }
    }

    /// Write the draft to deck file `path`.
    pub fn save_file(&mut self, tech: &Tech, path: &Path, standalone: bool) -> Result<(), String> {
        let text = rule_design::deck_text(&self.deck, tech);
        if let Err(errs) = drc::parse_deck(&text, tech) {
            self.tab = Tab::Issues;
            return Err(format!("Not saved: {}", errs[0]));
        }
        std::fs::write(path, &text).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
        if standalone {
            self.baseline = text;
            self.touch();
        }
        self.file = Some(path.to_path_buf());
        Ok(())
    }

    fn save_as(&mut self, tech: &Tech, standalone: bool) {
        let name =
            self.file.as_ref().and_then(|f| f.file_name()).map_or("rules.deck".into(), |f| f.to_string_lossy().into_owned());
        if let Some(path) = rfd::FileDialog::new().add_filter("DRC deck", &["deck", "drc"]).set_file_name(name).save_file() {
            self.save(tech, &path, standalone);
        }
    }

    fn save(&mut self, tech: &Tech, path: &Path, standalone: bool) {
        self.status = Some(match self.save_file(tech, path, standalone) {
            Ok(()) => (true, format!("Saved {}", path.display())),
            Err(e) => (false, e),
        });
    }

    /// Use `tech` for the standalone designer, starting from its deck.
    fn use_tech(&mut self, tech: Tech, file: Option<PathBuf>, notes: Vec<String>) {
        self.load(&tech, &tech.deck_text());
        self.notes.extend(notes);
        self.file = file.filter(|_| !self.notes.iter().any(|n| n.starts_with("SVRF")));
        self.status = Some((true, format!("Technology {} ({} layers)", tech.name, tech.layers.len())));
        self.own_tech = Some(tech);
    }

    fn touch(&mut self) {
        self.gen_ += 1;
    }

    fn refresh(&mut self, tech: &Tech) {
        if self.lint_gen != self.gen_ {
            self.findings = rule_design::lint(&self.deck, tech);
            self.dirty = rule_design::deck_text(&self.deck, tech) != self.baseline;
            self.lint_gen = self.gen_;
        }
        if !self.text_edited && self.text_gen != self.gen_ {
            self.text = rule_design::deck_text(&self.deck, tech);
            self.text_gen = self.gen_;
        }
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let Ok(rep) = job.rx.try_recv() else { return };
        let job = self.job.take().expect("checked");
        let mut counts = vec![0; job.rules.len()];
        let idx = |name: &str| job.rules.iter().position(|r| r == name);
        for v in &rep.violations {
            if let Some(i) = idx(&v.rule) {
                counts[i] += 1;
            }
        }
        for (name, full) in &rep.capped {
            if let Some(i) = idx(name) {
                counts[i] = *full;
            }
        }
        let total = counts.iter().sum();
        self.status = Some((true, format!("{}: {total} violation(s) from the draft deck", job.cell)));
        self.check = Some(CellCheck { gen_: job.gen_, cell: job.cell, counts, total, millis: rep.millis });
    }

    fn start_check(&mut self, ctx: &egui::Context, project: &Project, cell: &CellRef) {
        if project.cell(cell).and_then(|c| c.layout.as_ref()).is_none() {
            self.status = Some((false, format!("{cell} has no layout")));
            return;
        }
        let (tx, rx) = channel();
        let (p, r, deck, ctx) = (project.clone(), cell.clone(), self.deck.clone(), ctx.clone());
        std::thread::spawn(move || {
            if let Some(top) = p.cell(&r).and_then(|c| c.layout.as_ref()) {
                let _ = tx.send(drc::run(&deck, top, &p, &p.tech));
            }
            ctx.request_repaint();
        });
        let rules = self.deck.rules.iter().map(|r| r.name.clone()).collect();
        self.job = Some(CheckJob { gen_: self.gen_, cell: cell.clone(), rules, rx });
    }

    /// Validated deck text for applying, or the problems.
    fn apply_text(&mut self, tech: &Tech) -> Option<String> {
        let text = rule_design::deck_text(&self.deck, tech);
        match drc::parse_deck(&text, tech) {
            Ok(_) => {
                self.baseline = text.clone();
                self.notes.retain(|n| !n.starts_with("SVRF"));
                self.touch();
                Some(text)
            }
            Err(errs) => {
                self.status = Some((false, format!("Not applied: {}", errs[0])));
                self.tab = Tab::Issues;
                None
            }
        }
    }

    /// The designer window (a native window where the platform supports it).
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>, active: Option<&CellRef>) -> Option<DesignerAction> {
        if !self.open {
            return None;
        }
        let own = if project.is_none() { self.own_tech.take() } else { None };
        let Some(tech) = project.map(|p| &p.tech).or(own.as_ref()) else {
            self.open = false;
            return None;
        };
        if self.tech_sig != sig(tech) {
            let draft = self.text.clone();
            self.load(tech, &draft);
            self.baseline.clear();
            self.notes.push(format!("Re-read the draft for technology {}", tech.name));
        }
        self.poll();
        let mut open = true;
        let r = crate::float::tool_window(ctx, "rule_designer", "DRC Rule Designer", [1220.0, 780.0], &mut open, |ui| {
            self.ui(ui, tech, project, active)
        });
        if self.own_tech.is_none() {
            self.own_tech = own;
        }
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        tech: &Tech,
        project: Option<&Project>,
        active: Option<&CellRef>,
    ) -> Option<DesignerAction> {
        let active = active.filter(|_| project.is_some());
        let standalone = project.is_none();
        let pal = theme::palette(ui.ctx());
        if self.job.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.refresh(tech);
        let mut act = None;
        egui::Panel::top("rd_toolbar")
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| act = self.toolbar(ui, active, tech, standalone, &pal));
        egui::Panel::left("rd_list")
            .resizable(true)
            .default_size(290.0)
            .size_range(220.0..=460.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| self.rule_list(ui, tech, &pal));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let issues = self.findings.iter().filter(|f| f.severity != Severity::Info).count();
                for (t, name) in [
                    (Tab::Guide, "Start here".to_string()),
                    (Tab::Design, "Design".to_string()),
                    (Tab::Coverage, "Coverage".into()),
                    (Tab::Text, "Deck text".into()),
                    (Tab::Issues, format!("Issues ({issues})")),
                ] {
                    if ui.selectable_label(self.tab == t, name).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            for n in &self.notes {
                ui.label(RichText::new(n).small().color(pal.warning));
            }
            egui::ScrollArea::vertical().id_salt("rd_main").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Guide => {
                    if let Some(a) = self.guide(ui, tech, active, standalone, &pal) {
                        act = Some(a);
                    }
                }
                Tab::Design => match self.sel {
                    Sel::Rule(i) if i < self.deck.rules.len() => self.rule_editor(ui, i, tech, &pal),
                    Sel::Derived(k) if k < self.deck.derived.len() => self.derived_editor(ui, k, tech, &pal),
                    Sel::Add => self.add_gallery(ui, tech, &pal),
                    _ => {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("Select a rule on the left, or add one.").color(pal.text_weak));
                            ui.add_space(6.0);
                            if ui.button("Add a rule…").clicked() {
                                self.open_gallery(tech);
                            }
                            if ui.button("Open the guide").clicked() {
                                self.tab = Tab::Guide;
                            }
                        });
                    }
                },
                Tab::Coverage => self.coverage(ui, tech, &pal),
                Tab::Text => self.text_tab(ui, tech, &pal),
                Tab::Issues => self.issues(ui, tech, &pal),
            });
        });
        act.and_then(|a| self.run(a, ui.ctx(), tech, project, active))
    }

    fn run(
        &mut self,
        a: Act,
        ctx: &egui::Context,
        tech: &Tech,
        project: Option<&Project>,
        active: Option<&CellRef>,
    ) -> Option<DesignerAction> {
        let standalone = project.is_none();
        match a {
            Act::Reload => match self.file.clone().filter(|_| standalone) {
                Some(path) => self.open_file(tech, &path, true),
                None => self.load(tech, &tech.deck_text()),
            },
            Act::Defaults => {
                self.load(tech, &drc::default_deck_text(tech));
                self.notes.push("Started from the layer defaults (not applied yet)".into());
            }
            Act::FillGaps => {
                let n = self.fill_gaps(tech);
                self.status = Some((true, format!("Added {n} rule(s)")));
            }
            Act::FixAll => {
                let n = rule_design::fix_all(&mut self.deck, tech);
                self.status = Some((true, format!("Applied {n} fix(es)")));
                if n > 0 {
                    self.touch();
                }
            }
            Act::Check => {
                if let (Some(p), Some(c)) = (project, active) {
                    self.start_check(ctx, p, c);
                }
            }
            Act::Apply | Act::ApplyRun if standalone => {
                self.status = Some((false, "Open a project to apply the deck to its technology".into()));
            }
            Act::Apply => return self.apply_text(tech).map(DesignerAction::Apply),
            Act::ApplyRun => return self.apply_text(tech).map(DesignerAction::ApplyAndRun),
            Act::OpenDeck => {
                let pick =
                    rfd::FileDialog::new().add_filter("DRC deck", &["deck", "drc", "svrf", "rul", "cal", "txt"]).pick_file();
                if let Some(path) = pick {
                    self.open_file(tech, &path, standalone);
                }
            }
            Act::SaveDeck => match self.file.clone() {
                Some(path) => self.save(tech, &path, standalone),
                None => self.save_as(tech, standalone),
            },
            Act::SaveDeckAs => self.save_as(tech, standalone),
            Act::LoadPdk => {
                if let Some(path) = rfd::FileDialog::new().add_filter("IC Studio PDK", &["toml"]).pick_file() {
                    match ics_core::pdk::load(&path) {
                        Ok(pdk) => {
                            let notes = pdk.warnings.iter().map(|w| format!("PDK: {w}")).collect();
                            self.use_tech(pdk.tech, pdk.drc_deck, notes);
                        }
                        Err(e) => self.status = Some((false, format!("Cannot load PDK: {e}"))),
                    }
                }
            }
            Act::GenericTech => self.use_tech(Tech::generic(), None, vec![]),
        }
        None
    }

    fn open_gallery(&mut self, tech: &Tech) {
        self.add_layer = match self.sel {
            Sel::Rule(i) => self.deck.rules.get(i).map_or(0, |r| r.rule.checked_layer()),
            Sel::Derived(k) => tech.layers.len() + k,
            Sel::None | Sel::Add => tech.layers.iter().position(|l| l.routing).unwrap_or(0),
        };
        self.sel = Sel::Add;
        self.tab = Tab::Design;
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        active: Option<&CellRef>,
        tech: &Tech,
        standalone: bool,
        pal: &Palette,
    ) -> Option<Act> {
        let mut act = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("DRC Rule Designer").strong().color(pal.text_strong));
            if standalone {
                ui.menu_button(format!("Technology: {}", tech.name), |ui| {
                    if ui.button("Generic technology").clicked() {
                        act = Some(Act::GenericTech);
                        ui.close();
                    }
                    if ui.button("Load PDK…").clicked() {
                        act = Some(Act::LoadPdk);
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Technology the rules are written for (no project is open)");
            } else {
                ui.label(RichText::new(format!("· {}", tech.name)).color(pal.text_weak));
            }
            if let Some(f) = &self.file {
                let name = f.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                ui.label(RichText::new(name).monospace().color(pal.text_weak)).on_hover_text(f.display().to_string());
            }
            if self.dirty {
                let (t, h) = if standalone {
                    ("● unsaved", "The draft differs from the deck file. Save it to keep it.")
                } else {
                    ("● not applied", "The draft differs from the technology deck. Apply it to use it for DRC.")
                };
                ui.label(RichText::new(t).color(pal.warning)).on_hover_text(h);
            }
            ui.separator();
            if ui.button("Open…").on_hover_text("Open a deck file (IC Studio or SVRF)").clicked() {
                act = Some(Act::OpenDeck);
            }
            let save_hover = match &self.file {
                Some(f) => format!("Write the draft to {}", f.display()),
                None => "Write the draft to a deck file".into(),
            };
            if ui.button("Save").on_hover_text(save_hover).clicked() {
                act = Some(Act::SaveDeck);
            }
            if ui.button("Save as…").on_hover_text("Write the draft to a new deck file").clicked() {
                act = Some(Act::SaveDeckAs);
            }
            ui.separator();
            let reload = if standalone && self.file.is_some() {
                "Discard the draft and reopen the deck file"
            } else {
                "Discard the draft and load the technology's deck"
            };
            if ui.button("Reload").on_hover_text(reload).clicked() {
                act = Some(Act::Reload);
            }
            if ui.button("Layer defaults").on_hover_text("Start from the rules derived from the layer and via definitions").clicked() {
                act = Some(Act::Defaults);
            }
            if ui
                .button("Fill gaps")
                .on_hover_text("Add the width / space rules of routing layers and the via enclosures that are missing, with technology values")
                .clicked()
            {
                act = Some(Act::FillGaps);
            }
            if standalone {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.open = false;
                    }
                    if let Some((ok, s)) = &self.status {
                        ui.label(RichText::new(s).small().color(if *ok { pal.text_weak } else { pal.danger }));
                    }
                });
                return;
            }
            ui.separator();
            let busy = self.job.is_some();
            let can_check = active.is_some() && !busy;
            let check = ui.add_enabled(can_check, egui::Button::new(if busy { "Checking…" } else { "Check active cell" }));
            let hover = match active {
                Some(c) => format!("Run the draft deck on {c} and count violations per rule"),
                None => "Open a layout cell first".into(),
            };
            if check.on_hover_text(hover).on_disabled_hover_text("Open a layout cell first").clicked() {
                act = Some(Act::Check);
            }
            if busy {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    self.open = false;
                }
                if ui
                    .add_enabled(active.is_some(), egui::Button::new("Apply & run DRC"))
                    .on_hover_text("Store the deck in the technology and run DRC on the active cell")
                    .clicked()
                {
                    act = Some(Act::ApplyRun);
                }
                let apply = egui::Button::new(RichText::new("Apply to technology").color(theme::on_fill(pal.accent))).fill(pal.accent);
                if ui.add(apply).on_hover_text("Store the deck in the project's technology (save the project to keep it)").clicked() {
                    act = Some(Act::Apply);
                }
                if let Some((ok, s)) = &self.status {
                    ui.label(RichText::new(s).small().color(if *ok { pal.text_weak } else { pal.danger }));
                }
            });
        });
        act
    }

    fn guide(
        &mut self,
        ui: &mut egui::Ui,
        tech: &Tech,
        active: Option<&CellRef>,
        standalone: bool,
        pal: &Palette,
    ) -> Option<Act> {
        let mut act = None;
        ui.set_max_width(ui.available_width().min(780.0));
        ui.label(RichText::new("Designing DRC rules").heading().color(pal.text_strong));
        ui.label(
            RichText::new(
                "Design Rule Checking (DRC) makes sure a layout can be manufactured. The foundry sets limits such as \
                 \"metal1 lines must be at least 0.23 µm wide\". Each rule in the deck checks one such limit and marks every \
                 place in the layout that breaks it. This window builds the deck with forms and pictures, and tests every \
                 rule while you edit it.",
            )
            .color(pal.text),
        );
        ui.add_space(10.0);
        let issues = self.findings.iter().filter(|f| f.severity != Severity::Info).count();
        let fixable = self.findings.iter().filter(|f| rule_design::auto_fixable(f)).count();
        step(ui, 1, "Start from a deck", pal, |ui| {
            ui.label(format!(
                "The deck has {} rule(s) now. To start over, take the rules the technology derives from its layer and via definitions.",
                self.deck.rules.len()
            ));
            ui.horizontal(|ui| {
                if ui.button("Start from layer defaults").clicked() {
                    act = Some(Act::Defaults);
                }
                if ui.button("Reload technology deck").clicked() {
                    act = Some(Act::Reload);
                }
            });
        });
        step(ui, 2, "Fill the gaps", pal, |ui| {
            ui.label(
                "Add the width and space rules of routing layers and the via enclosures that the technology defines but the \
                 deck lacks. Coverage shows which checks each layer has.",
            );
            ui.horizontal(|ui| {
                if ui.button("Fill gaps").clicked() {
                    act = Some(Act::FillGaps);
                }
                if ui.button("Open coverage").clicked() {
                    self.tab = Tab::Coverage;
                }
            });
        });
        step(ui, 3, "Fix problems", pal, |ui| {
            let t = match (issues, fixable) {
                (0, 0) => "No errors or warnings. Notes with suggestions are listed under Issues.".to_string(),
                (n, 0) => format!("{n} error(s) or warning(s); open Issues to see what to change."),
                (n, m) => format!("{n} error(s) or warning(s); {m} can be fixed with one click."),
            };
            ui.label(t);
            ui.horizontal(|ui| {
                let fix = ui.add_enabled(fixable > 0, egui::Button::new(format!("Fix automatically ({fixable})")));
                if fix.on_hover_text("Apply every suggested fix of the errors and warnings, and add the missing rules").clicked()
                {
                    act = Some(Act::FixAll);
                }
                if ui.button("Open issues").clicked() {
                    self.tab = Tab::Issues;
                }
            });
        });
        step(ui, 4, "Add or change rules", pal, |ui| {
            ui.label(
                "Click a rule on the left to edit it. Every rule is tested on generated shapes: one exactly at the limit, \
                 which must pass, and one a grid step inside it, which must be flagged.",
            );
            if ui.button("Add a rule…").clicked() {
                self.open_gallery(tech);
            }
        });
        if standalone {
            step(ui, 5, "Save the deck", pal, |ui| {
                ui.label(
                    "No project is open, so the deck is kept in a file. Point a PDK's drc_deck at it to share it with \
                     every project that loads the PDK, or open it here from a project and apply it.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        act = Some(Act::SaveDeck);
                    }
                    if ui.button("Save as…").clicked() {
                        act = Some(Act::SaveDeckAs);
                    }
                });
            });
        } else {
            step(ui, 5, "Try it on your layout", pal, |ui| {
                ui.label(match active {
                    Some(c) => format!("Count the violations of each rule in {c}. Nothing is changed."),
                    None => "Open a layout cell to try the deck on it.".to_string(),
                });
                if ui.add_enabled(active.is_some() && self.job.is_none(), egui::Button::new("Check active cell")).clicked() {
                    act = Some(Act::Check);
                }
            });
            step(ui, 6, "Apply", pal, |ui| {
                ui.label(
                    "Store the deck as the technology's DRC deck so DRC uses it. Save the project to keep it, or save \
                     the deck to a file to share it.",
                );
                if ui.button("Apply to technology").clicked() {
                    act = Some(Act::Apply);
                }
            });
        }
        ui.add_space(12.0);
        theme::section(ui, "Words you will see");
        let grid = um(tech.grid.max(1), tech);
        egui::Grid::new("rd_terms").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            let terms = [
                ("Layer", "A mask layer such as poly, metal1 or via1. Each rule checks one layer; some compare two.".to_string()),
                ("Rule", "One limit, e.g. a minimum width. Its name (like metal1.W) is shown with every violation.".into()),
                ("Violation", "A place where the layout breaks a rule. DRC marks it in the layout.".into()),
                (
                    "Derived layer",
                    "A layer computed from others, e.g. gate = poly and active, to check only part of a layer.".into(),
                ),
                ("Grid", format!("The manufacturing grid. Every length must be a multiple of it ({grid} µm here).")),
            ];
            for (t, d) in terms {
                ui.label(RichText::new(t).strong().color(pal.text_strong));
                ui.label(RichText::new(d).color(pal.text));
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        theme::section(ui, "Kinds of checks");
        ui.label(RichText::new("Click one to add it.").small().color(pal.text_weak));
        let (a, b) = (color_of(0, tech, pal.dark), color_of(other_layer(0, tech), tech, pal.dark));
        let mut pick = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            for k in KINDS {
                pick |= kind_card(ui, k, a, b, pal, None).clicked();
            }
        });
        if pick {
            self.open_gallery(tech);
        }
        act
    }

    fn add_gallery(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        let names = self.deck.layer_names(tech);
        if self.add_layer >= names.len() {
            self.add_layer = 0;
        }
        ui.label(RichText::new("Add a rule").heading().color(pal.text_strong));
        ui.label(
            RichText::new(
                "Choose the layer, then click the kind of check. The value starts from the technology; you can change it afterwards.",
            )
            .color(pal.text_weak),
        );
        ui.add_space(8.0);
        let cx = Names { names: &names, tech, dark: pal.dark };
        egui::Grid::new("rd_add_layer").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            Self::layer_pick(ui, "rd_add_l", "Layer to check", &mut self.add_layer, names.len(), &cx);
        });
        let l = self.add_layer;
        let rec = recommended(l, tech);
        let (a, b) = (color_of(l, tech, pal.dark), color_of(other_layer(l, tech), tech, pal.dark));
        ui.add_space(8.0);
        let order: Vec<Kind> =
            KINDS.into_iter().filter(|k| rec.contains(k)).chain(KINDS.into_iter().filter(|k| !rec.contains(k))).collect();
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            for k in order {
                let have = self.deck.rules.iter().any(|r| Kind::of(&r.rule) == k && r.rule.checked_layer() == l);
                let note = if have {
                    Some(("Already in the deck: click to edit it".to_string(), pal.text_weak))
                } else if rec.contains(&k) {
                    Some(("Recommended for this layer".to_string(), pal.ok))
                } else {
                    None
                };
                if kind_card(ui, k, a, b, pal, note).clicked() {
                    chosen = Some(k);
                }
            }
        });
        let existing = |k: Kind| self.deck.rules.iter().position(|r| Kind::of(&r.rule) == k && r.rule.checked_layer() == l);
        if let Some(i) = chosen.and_then(existing) {
            self.sel = Sel::Rule(i);
        } else if let Some(k) = chosen {
            match new_rule(k, l, tech) {
                Ok(rule) => {
                    self.deck.rules.push(NamedRule { name: rule.auto_name(&names), rule, comment: None });
                    self.sel = Sel::Rule(self.deck.rules.len() - 1);
                    self.touch();
                }
                Err(e) => self.status = Some((false, e)),
            }
        }
    }

    fn fill_gaps(&mut self, tech: &Tech) -> usize {
        let has = |d: &Deck, k: Kind, l: usize| d.rules.iter().any(|r| Kind::of(&r.rule) == k && r.rule.checked_layer() == l);
        let mut add = vec![];
        for (l, def) in tech.layers.iter().enumerate() {
            if def.routing {
                for k in [Kind::Width, Kind::Space] {
                    if !has(&self.deck, k, l) {
                        add.extend(new_rule(k, l, tech).ok());
                    }
                }
            }
        }
        for v in &tech.vias {
            let (Some(cut), Some(bot), Some(top)) =
                (tech.layer_index(&v.cut), tech.layer_index(&v.bottom), tech.layer_index(&v.top))
            else {
                continue;
            };
            for (outer, enc) in [(bot, v.enc_bottom), (top, v.enc_top)] {
                let exists = self
                    .deck
                    .rules
                    .iter()
                    .any(|r| matches!(r.rule, Rule::Enclosure { inner, outer: o, .. } if inner == cut && o == outer));
                if !exists && enc > 0 {
                    add.push(Rule::Enclosure { inner: cut, outer, min: enc });
                }
            }
        }
        let names = self.deck.layer_names(tech);
        let n = add.len();
        for rule in add {
            self.deck.rules.push(NamedRule { name: rule.auto_name(&names), rule, comment: None });
        }
        if n > 0 {
            self.touch();
        }
        n
    }

    fn findings_for(&self, s: Subject) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.subject == s)
    }

    fn severity_of(&self, s: Subject) -> Option<Severity> {
        self.findings_for(s).map(|f| f.severity).min()
    }

    fn rule_list(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        let names = self.deck.layer_names(tech);
        ui.horizontal(|ui| {
            let fl = ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("name, layer or kind").desired_width(f32::INFINITY))
                .labelled_by(fl.id);
        });
        ui.horizontal(|ui| {
            if ui.button("+ Rule").on_hover_text("Pick a layer and a kind of check from pictures").clicked() {
                self.open_gallery(tech);
            }
            ui.menu_button("+ Derived layer", |ui| {
                ui.label(
                    RichText::new("A layer computed from others, used by rules like any layer").small().color(pal.text_weak),
                );
                let a = tech.poly().unwrap_or(0);
                let b = tech.diffusion().unwrap_or(other_layer(a, tech));
                let mut presets = vec![];
                if let (Some(p), Some(d)) = (tech.poly(), tech.diffusion()) {
                    presets.push((
                        "gate",
                        "Transistor gates: where poly crosses active",
                        LayerExpr::Bool { a: p, op: BoolOp::And, b: d },
                    ));
                    presets.push(("fieldpoly", "Poly wiring outside active", LayerExpr::Bool { a: p, op: BoolOp::Not, b: d }));
                }
                let wide = (0..tech.layers.len()).find(|&i| tech.layers[i].routing && Some(i) != tech.poly());
                let wide_name = wide.and_then(|m| tech.layer(m)).map(|d| format!("wide_{}", d.name));
                if let (Some(m), Some(n)) = (wide, wide_name.as_deref()) {
                    let min = 10 * tech.dbu_per_um.max(1);
                    presets.push((
                        n,
                        "Wide metal (10 µm and wider), for wide-metal spacing rules",
                        LayerExpr::Wide { a: m, min },
                    ));
                }
                presets.push((
                    "derived",
                    "Custom: pick the operation and layers yourself",
                    LayerExpr::Bool { a, op: BoolOp::And, b },
                ));
                for (base, what, expr) in presets {
                    let name = unique_layer_name(base, &names);
                    let line = DerivedLayer { name: name.clone(), expr: expr.clone() }.deck_line(tech, &names);
                    if ui.button(format!("{name}: {what}")).on_hover_text(line).clicked() {
                        self.deck.derived.push(DerivedLayer { name, expr });
                        self.sel = Sel::Derived(self.deck.derived.len() - 1);
                        self.tab = Tab::Design;
                        self.touch();
                        ui.close();
                    }
                }
            });
        });
        ui.label(
            RichText::new(format!("{} rule(s) · {} derived layer(s)", self.deck.rules.len(), self.deck.derived.len()))
                .small()
                .color(pal.text_weak),
        );
        ui.separator();
        let f = self.filter.to_ascii_lowercase();
        let check = self.check.as_ref().filter(|c| c.gen_ == self.gen_);
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt("rd_rules").auto_shrink([false, false]).show(ui, |ui| {
            if !self.deck.derived.is_empty() {
                theme::section(ui, "Derived layers");
                for (k, d) in self.deck.derived.iter().enumerate() {
                    let line = d.deck_line(tech, &names);
                    if !f.is_empty() && !line.to_ascii_lowercase().contains(&f) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        swatch(ui, color_of(tech.layers.len() + k, tech, pal.dark));
                        let r = ui.selectable_label(self.sel == Sel::Derived(k), RichText::new(&d.name).strong());
                        if r.on_hover_text(&line).clicked() {
                            pick = Some(Sel::Derived(k));
                        }
                        let rest = line.split_once('=').map_or("", |x| x.1).trim();
                        ui.label(RichText::new(rest).small().color(pal.text_weak));
                    });
                }
                ui.add_space(6.0);
            }
            let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for (i, r) in self.deck.rules.iter().enumerate() {
                let hay = format!(
                    "{} {} {}",
                    r.name,
                    names.get(r.rule.checked_layer()).cloned().unwrap_or_default(),
                    Kind::of(&r.rule).label()
                )
                .to_ascii_lowercase();
                if f.is_empty() || hay.contains(&f) {
                    groups.entry(r.rule.checked_layer()).or_default().push(i);
                }
            }
            for (layer, idx) in groups {
                let lname = names.get(layer).cloned().unwrap_or_else(|| format!("#{layer}"));
                let c = color_of(layer, tech, pal.dark);
                egui::CollapsingHeader::new(RichText::new(format!("{lname}  ({})", idx.len())).color(pal.text))
                    .id_salt(("rd_layer", layer))
                    .default_open(true)
                    .show(ui, |ui| {
                        for i in idx {
                            let r = &self.deck.rules[i];
                            ui.horizontal(|ui| {
                                let k = Kind::of(&r.rule);
                                chip(ui, k.tag(), c).on_hover_text(format!("{}: {}", k.title(), k.question()));
                                let resp = ui.selectable_label(self.sel == Sel::Rule(i), &r.name);
                                if resp.on_hover_text(r.rule.description(tech, &names)).clicked() {
                                    pick = Some(Sel::Rule(i));
                                }
                                ui.label(RichText::new(rule_value(&r.rule, tech)).small().monospace().color(pal.text_weak));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if let Some(c) = check
                                        && let Some(&n) = c.counts.get(i)
                                    {
                                        let col = if n == 0 { pal.ok } else { pal.danger };
                                        ui.label(RichText::new(n.to_string()).small().strong().color(col))
                                            .on_hover_text(format!("{n} violation(s) in {}", c.cell));
                                    }
                                    if let Some(s) = self.severity_of(Subject::Rule(i)) {
                                        let col = match s {
                                            Severity::Error => pal.danger,
                                            Severity::Warning => pal.warning,
                                            Severity::Info => pal.text_weak,
                                        };
                                        ui.label(RichText::new("●").small().color(col));
                                    }
                                });
                            });
                        }
                    });
            }
        });
        if let Some(s) = pick {
            self.sel = s;
            self.tab = Tab::Design;
        }
    }

    fn layer_pick(ui: &mut egui::Ui, id: &str, label: &str, l: &mut usize, limit: usize, cx: &Names) -> bool {
        let (names, tech, dark) = (cx.names, cx.tech, cx.dark);
        let lb = ui.label(label);
        let before = *l;
        let text = names.get(*l).cloned().unwrap_or_else(|| format!("#{l}"));
        let r = egui::ComboBox::from_id_salt(id)
            .selected_text(RichText::new(text).color(color_of(*l, tech, dark)))
            .width(180.0)
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate().take(limit) {
                    ui.horizontal(|ui| {
                        swatch(ui, color_of(i, tech, dark));
                        ui.selectable_value(l, i, n);
                    });
                }
            });
        let _ = r.response.labelled_by(lb.id);
        ui.end_row();
        *l != before
    }

    /// A length in µm; `hint` is the technology's value, offered when it differs.
    fn len_edit(ui: &mut egui::Ui, label: &str, v: &mut i64, tech: &Tech, pal: &Palette, hint: Option<i64>) -> bool {
        let d = tech.dbu_per_um.max(1) as f64;
        let g = tech.grid.max(1);
        let lb = ui.label(label);
        let mut changed = false;
        ui.horizontal(|ui| {
            let mut x = *v as f64 / d;
            let r = ui
                .add(egui::DragValue::new(&mut x).speed(g as f64 / d).range(0.0..=1.0e4).max_decimals(4).suffix(" µm"))
                .labelled_by(lb.id);
            if r.changed() {
                *v = (x * d).round() as i64;
                changed = true;
            }
            if g > 1 && *v % g != 0 {
                ui.label(RichText::new("off grid").small().color(pal.warning));
                let snapped = ((*v as f64 / g as f64).round() as i64 * g).max(g);
                if ui.small_button(format!("Snap to {}", um(snapped, tech))).clicked() {
                    *v = snapped;
                    changed = true;
                }
            }
            match hint {
                Some(t) if t == *v => {
                    ui.label(RichText::new("technology value").small().color(pal.text_weak));
                }
                Some(t) => {
                    ui.label(RichText::new(format!("technology: {} µm", um(t, tech))).small().color(pal.text_weak));
                    if ui.small_button("Use").on_hover_text("Use the value from the layer and via definitions").clicked() {
                        *v = t;
                        changed = true;
                    }
                }
                None => {}
            }
        });
        ui.end_row();
        changed
    }

    fn num_edit(ui: &mut egui::Ui, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str) -> bool {
        let lb = ui.label(label);
        let c =
            ui.add(egui::DragValue::new(v).speed(0.5).range(range).max_decimals(3).suffix(suffix)).labelled_by(lb.id).changed();
        ui.end_row();
        c
    }

    fn rule_editor(&mut self, ui: &mut egui::Ui, i: usize, tech: &Tech, pal: &Palette) {
        let names = self.deck.layer_names(tech);
        let limit = names.len();
        let mut changed = false;
        let mut delete = false;
        let mut duplicate = false;
        let kind = Kind::of(&self.deck.rules[i].rule);
        let layer = self.deck.rules[i].rule.checked_layer();
        ui.horizontal(|ui| {
            let second = self.deck.rules[i].rule.layers().into_iter().find(|&l| l != layer).unwrap_or(other_layer(layer, tech));
            let (r, p) = ui.allocate_painter(Vec2::new(72.0, 44.0), Sense::hover());
            p.rect_filled(r.rect, 4.0, pal.bg_canvas);
            kind_icon(&p, r.rect.shrink(4.0), kind, color_of(layer, tech, pal.dark), color_of(second, tech, pal.dark), pal);
            ui.vertical(|ui| {
                ui.label(RichText::new(&self.deck.rules[i].name).heading().color(pal.text_strong));
                ui.label(RichText::new(format!("{}: {}", kind.title(), kind.question())).color(pal.text_weak));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Delete").clicked() {
                    delete = true;
                }
                if ui.button("Duplicate").clicked() {
                    duplicate = true;
                }
                if ui.add_enabled(i + 1 < self.deck.rules.len(), egui::Button::new("Down")).clicked() {
                    self.deck.rules.swap(i, i + 1);
                    self.sel = Sel::Rule(i + 1);
                    self.touch();
                }
                if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                    self.deck.rules.swap(i, i - 1);
                    self.sel = Sel::Rule(i - 1);
                    self.touch();
                }
            });
        });
        if !matches!(self.sel, Sel::Rule(j) if j == i) {
            return;
        }
        let nr = &mut self.deck.rules[i];
        let desc = nr.comment.clone().unwrap_or_else(|| nr.rule.description(tech, &names));
        ui.label(RichText::new(desc).color(pal.text));
        ui.label(RichText::new(kind.explain()).small().color(pal.text_weak));
        ui.add_space(8.0);

        let was_auto = nr.name == nr.rule.auto_name(&names);
        egui::Grid::new(("rd_form", i)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            let lb = ui.label("Check");
            let mut k = kind;
            egui::ComboBox::from_id_salt("rd_kind")
                .selected_text(k.title())
                .width(180.0)
                .show_ui(ui, |ui| {
                    for x in KINDS {
                        ui.selectable_value(&mut k, x, x.title()).on_hover_text(format!("{}\n{}", x.question(), x.explain()));
                    }
                })
                .response
                .labelled_by(lb.id);
            ui.end_row();
            if k != kind {
                match new_rule(k, layer, tech) {
                    Ok(r) => {
                        nr.rule = r;
                        changed = true;
                    }
                    Err(e) => self.status = Some((false, e)),
                }
            }
            let cx = Names { names: &names, tech, dark: pal.dark };
            let mut swap = None;
            let hint = tech_value(&nr.rule, tech);
            match &mut nr.rule {
                Rule::Width { layer, min } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Layer", layer, limit, &cx);
                    changed |= Self::len_edit(ui, "Minimum width", min, tech, pal, hint);
                }
                Rule::Space { layer, min } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Layer", layer, limit, &cx);
                    changed |= Self::len_edit(ui, "Minimum spacing", min, tech, pal, hint);
                }
                Rule::Separation { a, b, min } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "From layer", a, limit, &cx);
                    changed |= Self::layer_pick(ui, "rd_l1", "Checked layer", b, limit, &cx);
                    changed |= Self::len_edit(ui, "Minimum separation", min, tech, pal, hint);
                }
                Rule::Enclosure { inner, outer, min } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Inner layer (e.g. via)", inner, limit, &cx);
                    changed |= Self::layer_pick(ui, "rd_l1", "Outer layer (e.g. metal)", outer, limit, &cx);
                    changed |= Self::len_edit(ui, "Minimum margin", min, tech, pal, hint);
                }
                Rule::Extension { a, b, min } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Extending layer (e.g. poly)", a, limit, &cx);
                    changed |= Self::layer_pick(ui, "rd_l1", "Past layer (e.g. active)", b, limit, &cx);
                    changed |= Self::len_edit(ui, "Minimum extension", min, tech, pal, hint);
                }
                Rule::Area { layer, min } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Layer", layer, limit, &cx);
                    let d = tech.dbu_per_um.max(1) as f64;
                    let mut x = *min as f64 / (d * d);
                    if Self::num_edit(ui, "Minimum area", &mut x, 0.0..=1.0e8, " µm²") {
                        *min = (x * d * d).round() as i128;
                        changed = true;
                    }
                }
                Rule::Antenna { layer, ratio, .. } => {
                    let mut l = *layer;
                    if Self::layer_pick(ui, "rd_l0", "Metal", &mut l, tech.layers.len(), &cx) {
                        match new_rule(Kind::Antenna, l, tech) {
                            Ok(Rule::Antenna { layer: nl, poly, active, vias, .. }) if nl == l => {
                                swap = Some(Rule::Antenna { layer: nl, ratio: *ratio, poly, active, vias });
                            }
                            _ => self.status = Some((false, format!("{} is not a metal of the via stack", names[l]))),
                        }
                    }
                    changed |= Self::num_edit(ui, "Maximum ratio", ratio, 0.0..=1.0e6, "");
                }
                Rule::Density { layer, min, max, window, step } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Layer", layer, limit, &cx);
                    changed |= Self::num_edit(ui, "Minimum coverage", min, 0.0..=100.0, " %");
                    changed |= Self::num_edit(ui, "Maximum coverage", max, 0.0..=100.0, " %");
                    if *min > *max {
                        *max = *min;
                    }
                    changed |= Self::len_edit(ui, "Window", window, tech, pal, None);
                    changed |= Self::len_edit(ui, "Step", step, tech, pal, None);
                    *window = (*window).max(1);
                    *step = (*step).max(1);
                }
                Rule::Flag { layer } => {
                    changed |= Self::layer_pick(ui, "rd_l0", "Layer", layer, limit, &cx);
                }
            }
            if let Some(r) = swap {
                nr.rule = r;
                changed = true;
            }
            let lb = ui.label("Name");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut nr.name).desired_width(180.0)).labelled_by(lb.id).changed();
                if !was_auto && ui.small_button("Auto").on_hover_text("Use the generated name").clicked() {
                    nr.name = nr.rule.auto_name(&names);
                    changed = true;
                }
            });
            ui.end_row();
            let lb = ui.label("Description");
            let mut c = nr.comment.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut c).hint_text("reported with violations (optional)").desired_width(320.0))
                .labelled_by(lb.id)
                .changed()
            {
                nr.comment = (!c.trim().is_empty()).then(|| c.trim().to_string());
                changed = true;
            }
            ui.end_row();
        });
        if changed && was_auto {
            nr.name = nr.rule.auto_name(&names);
        }
        if changed {
            self.touch();
        }
        if delete {
            self.deck.rules.remove(i);
            self.sel = if self.deck.rules.is_empty() { Sel::None } else { Sel::Rule(i.min(self.deck.rules.len() - 1)) };
            self.touch();
            return;
        }
        if duplicate {
            let mut c = self.deck.rules[i].clone();
            c.name = format!("{}_copy", c.name);
            self.deck.rules.insert(i + 1, c);
            self.sel = Sel::Rule(i + 1);
            self.touch();
            return;
        }

        let findings: Vec<Finding> = self.findings_for(Subject::Rule(i)).cloned().collect();
        if !findings.is_empty() {
            ui.add_space(8.0);
            for f in &findings {
                if finding_row(ui, f, tech, &names, pal) {
                    rule_design::apply_fix(&mut self.deck, f, tech);
                    self.touch();
                    return;
                }
            }
        }

        ui.add_space(14.0);
        theme::section(ui, "Does the rule work? Self-test on generated shapes (real DRC engine)");
        let rule = self.deck.rules[i].rule.clone();
        if self.runs.as_ref().is_none_or(|(r, _)| *r != rule) {
            let runs = if rule.layers().iter().all(|&l| l < tech.layers.len()) {
                rule_design::run_patterns(&rule, tech)
            } else {
                vec![]
            };
            self.runs = Some((rule.clone(), runs));
        }
        let runs = &self.runs.as_ref().expect("set").1;
        if runs.is_empty() {
            let why = if rule.layers().iter().any(|&l| l >= tech.layers.len()) {
                "This rule reads a derived layer; use Check active cell to see what it flags."
            } else {
                "No generated patterns for this check; the sketch below shows what it measures."
            };
            ui.label(RichText::new(why).small().color(pal.text_weak));
            ui.add_space(6.0);
            sketch(ui, &rule, tech, &names, pal);
        } else {
            let ok = runs.iter().all(|r| r.as_expected);
            let (c, t) = if ok {
                (pal.ok, "✔ The engine accepts the limit and flags a grid step inside it.")
            } else {
                (pal.danger, "✖ The engine does not behave as the rule intends. Check the value and layers.")
            };
            ui.label(RichText::new(t).color(c));
            ui.add_space(4.0);
            ui.columns(runs.len(), |cols| {
                for (ui, r) in cols.iter_mut().zip(runs) {
                    pattern_card(ui, r, tech, pal);
                }
            });
        }
        if let Some(c) = self.check.as_ref().filter(|c| c.gen_ == self.gen_)
            && let Some(&n) = c.counts.get(i)
        {
            ui.add_space(10.0);
            theme::section(ui, "On the active cell");
            let col = if n == 0 { pal.ok } else { pal.danger };
            ui.label(RichText::new(format!("{n} violation(s) in {} ({} total, {:.0} ms)", c.cell, c.total, c.millis)).color(col));
        }
    }

    fn derived_editor(&mut self, ui: &mut egui::Ui, k: usize, tech: &Tech, pal: &Palette) {
        let names = self.deck.layer_names(tech);
        let n = tech.layers.len();
        let me = n + k;
        let users: Vec<String> = self
            .deck
            .rules
            .iter()
            .filter(|r| r.rule.layers().contains(&me))
            .map(|r| r.name.clone())
            .chain(self.deck.derived.iter().filter(|d| d.expr.inputs().contains(&me)).map(|d| d.name.clone()))
            .collect();
        let mut changed = false;
        let mut delete = false;
        ui.horizontal(|ui| {
            chip(ui, "LAYER", color_of(me, tech, pal.dark));
            ui.label(RichText::new(&self.deck.derived[k].name).heading().color(pal.text_strong));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let del = ui.add_enabled(users.is_empty(), egui::Button::new("Delete"));
                if del.on_disabled_hover_text(format!("Used by {}", users.join(", "))).clicked() {
                    delete = true;
                }
            });
        });
        if delete {
            rule_design::remove_derived(&mut self.deck, k, tech);
            self.sel = Sel::None;
            self.touch();
            return;
        }
        ui.label(
            RichText::new(
                "A derived layer is computed from technology layers and earlier derived layers; rules use it like any layer.",
            )
            .small()
            .color(pal.text_weak),
        );
        ui.add_space(8.0);
        let d = &mut self.deck.derived[k];
        egui::Grid::new(("rd_dform", k)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            let lb = ui.label("Name");
            let mut name = d.name.clone();
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut name).desired_width(180.0)).labelled_by(lb.id);
                let clash = names.iter().enumerate().any(|(i, x)| i != me && x.eq_ignore_ascii_case(&name));
                if !valid_layer_name(&name) {
                    ui.label(RichText::new("letters, digits, _ . - only").small().color(pal.danger));
                } else if clash {
                    ui.label(RichText::new("name already used").small().color(pal.danger));
                } else if name != d.name {
                    d.name = name;
                    changed = true;
                }
            });
            ui.end_row();
            match Op::of(&d.expr) {
                None => {
                    ui.label("Expression");
                    ui.label(RichText::new("SVRF-only expression: it cannot be written to a native deck").color(pal.danger));
                    ui.end_row();
                }
                Some(op) => {
                    let lb = ui.label("Operation");
                    let mut o = op;
                    egui::ComboBox::from_id_salt("rd_op")
                        .selected_text(o.label())
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for x in OPS {
                                ui.selectable_value(&mut o, x, x.label());
                            }
                        })
                        .response
                        .labelled_by(lb.id);
                    ui.end_row();
                    if o != op {
                        d.expr = o.apply(&d.expr, tech);
                        changed = true;
                    }
                    let cx = Names { names: &names, tech, dark: pal.dark };
                    match &mut d.expr {
                        LayerExpr::Bool { a, b, .. } | LayerExpr::Select { a, b, .. } => {
                            changed |= Self::layer_pick(ui, "rd_da", "A", a, me, &cx);
                            changed |= Self::layer_pick(ui, "rd_db", "B", b, me, &cx);
                        }
                        LayerExpr::Size { a, by } => {
                            changed |= Self::layer_pick(ui, "rd_da", "A", a, me, &cx);
                            let lb = ui.label("D (negative shrinks)");
                            let dd = tech.dbu_per_um.max(1) as f64;
                            let mut x = *by as f64 / dd;
                            let r = ui.add(
                                egui::DragValue::new(&mut x)
                                    .speed(tech.grid.max(1) as f64 / dd)
                                    .range(-1.0e3..=1.0e3)
                                    .max_decimals(4)
                                    .suffix(" µm"),
                            );
                            if r.labelled_by(lb.id).changed() {
                                *by = (x * dd).round() as i64;
                                changed = true;
                            }
                            ui.end_row();
                        }
                        LayerExpr::Wide { a, min } => {
                            changed |= Self::layer_pick(ui, "rd_da", "A", a, me, &cx);
                            changed |= Self::len_edit(ui, "D (minimum width)", min, tech, pal, None);
                            *min = (*min).max(1);
                        }
                        LayerExpr::AreaRange { a, min, max } => {
                            changed |= Self::layer_pick(ui, "rd_da", "A", a, me, &cx);
                            let dd = (tech.dbu_per_um.max(1) as f64).powi(2);
                            let mut lo = min.unwrap_or(0) as f64 / dd;
                            if Self::num_edit(ui, "MIN", &mut lo, 0.0..=1.0e8, " µm²") {
                                *min = Some((lo * dd).round() as i128);
                                changed = true;
                            }
                            let lb = ui.label("MAX");
                            ui.horizontal(|ui| {
                                let mut inf = max.is_none();
                                if ui.checkbox(&mut inf, "no limit").changed() {
                                    *max = if inf { None } else { Some((lo * dd).round() as i128 * 4 + 1) };
                                    changed = true;
                                }
                                if let Some(m) = max {
                                    let mut hi = *m as f64 / dd;
                                    let r = ui.add(
                                        egui::DragValue::new(&mut hi)
                                            .speed(0.5)
                                            .range(0.0..=1.0e8)
                                            .max_decimals(3)
                                            .suffix(" µm²"),
                                    );
                                    if r.labelled_by(lb.id).changed() {
                                        *m = (hi * dd).round() as i128;
                                        changed = true;
                                    }
                                }
                            });
                            ui.end_row();
                        }
                        LayerExpr::Holes { a } | LayerExpr::Extents { a } => {
                            changed |= Self::layer_pick(ui, "rd_da", "A", a, me, &cx);
                        }
                        LayerExpr::Union(_) | LayerExpr::Check(_) => {}
                    }
                }
            }
        });
        if changed {
            self.touch();
        }
        let d = &self.deck.derived[k];
        ui.add_space(6.0);
        ui.label(RichText::new(d.deck_line(tech, &names)).monospace().color(pal.text));
        if !users.is_empty() {
            ui.label(RichText::new(format!("Used by {}", users.join(", "))).small().color(pal.text_weak));
        }
        let findings: Vec<Finding> = self.findings_for(Subject::Derived(k)).cloned().collect();
        for f in &findings {
            if finding_row(ui, f, tech, &names, pal) {
                rule_design::apply_fix(&mut self.deck, f, tech);
                if matches!(f.fix, Some(rule_design::Fix::RemoveDerived)) {
                    self.sel = Sel::None;
                }
                self.touch();
                return;
            }
        }
        let d = &self.deck.derived[k];
        ui.add_space(14.0);
        theme::section(ui, "Preview on sample shapes");
        if let Some(p) = rule_design::derived_preview(&d.expr) {
            preview_card(ui, &p, &d.name, me, tech, &names, pal);
        }
        if Op::of(&d.expr).is_some_and(Op::binary) {
            ui.label(RichText::new("Outlines: inputs A and B · filled: the result").small().color(pal.text_weak));
        }
    }

    fn coverage(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        let names = self.deck.layer_names(tech);
        let cols = [
            Kind::Width,
            Kind::Space,
            Kind::Area,
            Kind::Enclosure,
            Kind::Separation,
            Kind::Extension,
            Kind::Density,
            Kind::Antenna,
        ];
        ui.label(
            RichText::new("Checks per layer (enclosure, separation and extension are listed on the checked layer). Click a value to edit it, + to add it with the technology value.")
                .small()
                .color(pal.text_weak),
        );
        ui.add_space(6.0);
        let mut pick = None;
        let mut add: Option<(Kind, usize)> = None;
        egui::Grid::new("rd_cov").striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Layer").strong());
            for k in cols {
                ui.label(RichText::new(k.label()).strong()).on_hover_text(format!("{}: {}", k.title(), k.question()));
            }
            ui.end_row();
            for (l, lname) in names.iter().enumerate() {
                let routing = tech.layer(l).is_some_and(|d| d.routing);
                ui.horizontal(|ui| {
                    swatch(ui, color_of(l, tech, pal.dark));
                    ui.label(lname);
                    if routing {
                        ui.label(RichText::new("routing").small().color(pal.text_weak));
                    }
                });
                for k in cols {
                    let hits: Vec<usize> = self
                        .deck
                        .rules
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| Kind::of(&r.rule) == k && r.rule.checked_layer() == l)
                        .map(|(i, _)| i)
                        .collect();
                    ui.horizontal(|ui| {
                        for &i in &hits {
                            let r = &self.deck.rules[i].rule;
                            let mut t = rule_value(r, tech);
                            match *r {
                                Rule::Enclosure { outer: o, .. } => t = format!("{t} by {}", names[o]),
                                Rule::Separation { a, .. } => t = format!("{t} to {}", names[a]),
                                Rule::Extension { b, .. } => t = format!("{t} past {}", names[b]),
                                _ => {}
                            }
                            if ui.selectable_label(self.sel == Sel::Rule(i), RichText::new(t).small()).clicked() {
                                pick = Some(i);
                            }
                        }
                        let basic = matches!(k, Kind::Width | Kind::Space | Kind::Area);
                        if hits.is_empty() && l < tech.layers.len() {
                            if routing && matches!(k, Kind::Width | Kind::Space) {
                                ui.label(RichText::new("missing").small().color(pal.warning));
                            }
                            if (basic || routing)
                                && ui.small_button("+").on_hover_text(format!("Add a {} rule", k.label())).clicked()
                            {
                                add = Some((k, l));
                            }
                        }
                    });
                }
                ui.end_row();
            }
        });
        if let Some(i) = pick {
            self.sel = Sel::Rule(i);
            self.tab = Tab::Design;
        }
        if let Some((k, l)) = add {
            match new_rule(k, l, tech) {
                Ok(rule) => {
                    self.deck.rules.push(NamedRule { name: rule.auto_name(&names), rule, comment: None });
                    self.sel = Sel::Rule(self.deck.rules.len() - 1);
                    self.tab = Tab::Design;
                    self.touch();
                }
                Err(e) => self.status = Some((false, e)),
            }
        }
    }

    fn text_tab(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        ui.label(
            RichText::new("The deck the designer writes. Edit it here and press Use text to bring the changes back into the designer; SVRF decks are translated.")
                .small()
                .color(pal.text_weak),
        );
        ui.horizontal(|ui| {
            if ui.add_enabled(self.text_edited, egui::Button::new("Use text")).clicked() {
                match drc::parse_any(&self.text, tech) {
                    Ok(p) => {
                        self.deck = p.deck;
                        self.notes = p.notes;
                        self.text_edited = false;
                        self.text_errors.clear();
                        self.sel = if self.deck.rules.is_empty() { Sel::None } else { Sel::Rule(0) };
                        self.touch();
                    }
                    Err(errs) => self.text_errors = errs.iter().map(|e| e.to_string()).collect(),
                }
            }
            if ui.add_enabled(self.text_edited, egui::Button::new("Discard edits")).clicked() {
                self.text_edited = false;
                self.text_errors.clear();
                self.text_gen = 0;
            }
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(self.text.clone());
            }
        });
        for e in &self.text_errors {
            ui.label(RichText::new(e).small().color(pal.danger));
        }
        let lb = ui.label(RichText::new("Deck").small().color(pal.text_weak));
        let r = ui
            .add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_rows(30).desired_width(f32::INFINITY))
            .labelled_by(lb.id);
        if r.changed() {
            self.text_edited = true;
        }
    }

    fn issues(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        if self.findings.is_empty() {
            ui.label(RichText::new("✔ No issues found").color(pal.ok));
            return;
        }
        let names = self.deck.layer_names(tech);
        ui.label(
            RichText::new(
                "error: the rule cannot work as written · warning: probably a mistake · note: a suggestion. \
                 The buttons apply the suggested change to the draft.",
            )
            .small()
            .color(pal.text_weak),
        );
        let fixable = self.findings.iter().filter(|f| rule_design::auto_fixable(f)).count();
        let all = ui.add_enabled(fixable > 0, egui::Button::new(format!("Fix automatically ({fixable})")));
        let fix_all =
            all.on_hover_text("Apply every suggested fix of the errors and warnings, and add the missing rules").clicked();
        ui.add_space(6.0);
        let mut pick = None;
        let mut fix = None;
        for f in &self.findings {
            ui.horizontal(|ui| {
                if finding_row(ui, f, tech, &names, pal) {
                    fix = Some(f.clone());
                }
                let target = match f.subject {
                    Subject::Rule(i) => self.deck.rules.get(i).map(|r| (Sel::Rule(i), r.name.clone())),
                    Subject::Derived(k) => self.deck.derived.get(k).map(|d| (Sel::Derived(k), d.name.clone())),
                    Subject::Deck => None,
                };
                if let Some((s, name)) = target
                    && ui.small_button(format!("Go to {name}")).clicked()
                {
                    pick = Some(s);
                }
            });
        }
        if let Some(s) = pick {
            self.sel = s;
            self.tab = Tab::Design;
        }
        if fix_all {
            let n = rule_design::fix_all(&mut self.deck, tech);
            self.status = Some((true, format!("Applied {n} fix(es)")));
            self.touch();
        } else if let Some(f) = fix {
            rule_design::apply_fix(&mut self.deck, &f, tech);
            self.touch();
        }
    }
}

/// A finding with its fix button; true when the fix was clicked.
fn finding_row(ui: &mut egui::Ui, f: &Finding, tech: &Tech, names: &[String], pal: &Palette) -> bool {
    let (tag, c) = match f.severity {
        Severity::Error => ("error", pal.danger),
        Severity::Warning => ("warning", pal.warning),
        Severity::Info => ("note", pal.text_weak),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tag).small().strong().color(c));
        ui.label(RichText::new(&f.message).small().color(pal.text));
        f.fix.as_ref().is_some_and(|x| ui.small_button(x.label(tech, names)).clicked())
    })
    .inner
}

/// A numbered step of the guide.
fn step(ui: &mut egui::Ui, n: usize, title: &str, pal: &Palette, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(pal.bg_sidebar).stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(10).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, p) = ui.allocate_painter(Vec2::splat(24.0), Sense::hover());
                p.circle_filled(r.rect.center(), 11.0, pal.accent);
                p.text(
                    r.rect.center(),
                    Align2::CENTER_CENTER,
                    n.to_string(),
                    FontId::proportional(13.0),
                    theme::on_fill(pal.accent),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong().color(pal.text_strong));
                    body(ui);
                });
            });
        },
    );
    ui.add_space(6.0);
}

/// A clickable card with a picture of a kind of check, its plain question and an optional note.
fn kind_card(
    ui: &mut egui::Ui,
    kind: Kind,
    a: Color32,
    b: Color32,
    pal: &Palette,
    note: Option<(String, Color32)>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(232.0, 176.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, kind.title()));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hov = resp.hovered();
        p.rect_filled(rect, 6.0, if hov { pal.bg_hover } else { pal.bg_sidebar });
        let edge = if hov { Stroke::new(1.5, pal.accent) } else { Stroke::new(1.0, pal.border) };
        p.rect_stroke(rect, 6.0, edge, StrokeKind::Inside);
        let inner = rect.shrink(10.0);
        let icon = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 60.0));
        p.rect_filled(icon, 4.0, pal.bg_canvas);
        kind_icon(p, icon.shrink(6.0), kind, a, b, pal);
        let title = p.layout_no_wrap(kind.title().to_string(), FontId::proportional(14.0), pal.text_strong);
        let mut y = icon.bottom() + 6.0;
        let th = title.size().y;
        p.galley(Pos2::new(inner.left(), y), title, pal.text_strong);
        y += th + 2.0;
        let q = p.layout(kind.question().to_string(), FontId::proportional(12.0), pal.text, inner.width());
        p.galley(Pos2::new(inner.left(), y), q, pal.text);
        if let Some((t, c)) = note {
            p.text(inner.left_bottom(), Align2::LEFT_BOTTOM, t, FontId::proportional(11.0), c);
        }
    }
    resp.on_hover_text(kind.explain())
}

/// A double-headed dimension arrow.
fn arrow(p: &egui::Painter, a: Pos2, b: Pos2, col: Color32) {
    let st = Stroke::new(1.2, col);
    p.line_segment([a, b], st);
    let d = (b - a).normalized();
    let n = Vec2::new(-d.y, d.x);
    for (q, dir) in [(a, d), (b, -d)] {
        p.line_segment([q, q + dir * 4.0 + n * 2.5], st);
        p.line_segment([q, q + dir * 4.0 - n * 2.5], st);
    }
}

/// A small picture of what a kind of check measures; `a` is the checked layer, `b` the other.
fn kind_icon(p: &egui::Painter, r: Rect, kind: Kind, a: Color32, b: Color32, pal: &Palette) {
    let close = |x: u8, y: u8| x.abs_diff(y) as u32;
    let b = if close(a.r(), b.r()) + close(a.g(), b.g()) + close(a.b(), b.b()) < 120 { pal.accent } else { b };
    let at = |fx: f32, fy: f32| r.min + Vec2::new(r.width() * fx, r.height() * fy);
    let rr = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let fill = |q: Rect, c: Color32| {
        p.rect_filled(q, 1.0, c.gamma_multiply(0.45));
        p.rect_stroke(q, 1.0, Stroke::new(1.2, c), StrokeKind::Inside);
    };
    let ink = pal.text_strong;
    match kind {
        Kind::Width => {
            fill(rr(0.42, 0.05, 0.58, 0.95), a);
            arrow(p, at(0.42, 0.5), at(0.58, 0.5), ink);
        }
        Kind::Space => {
            fill(rr(0.2, 0.05, 0.4, 0.95), a);
            fill(rr(0.6, 0.05, 0.8, 0.95), a);
            arrow(p, at(0.4, 0.5), at(0.6, 0.5), ink);
        }
        Kind::Separation => {
            fill(rr(0.2, 0.05, 0.4, 0.95), b);
            fill(rr(0.6, 0.05, 0.8, 0.95), a);
            arrow(p, at(0.4, 0.5), at(0.6, 0.5), ink);
        }
        Kind::Enclosure => {
            fill(rr(0.3, 0.05, 0.7, 0.95), b);
            fill(rr(0.42, 0.3, 0.55, 0.7), a);
            arrow(p, at(0.55, 0.5), at(0.7, 0.5), ink);
        }
        Kind::Extension => {
            fill(rr(0.2, 0.4, 0.8, 0.95), b);
            fill(rr(0.45, 0.05, 0.55, 0.95), a);
            arrow(p, at(0.62, 0.4), at(0.62, 0.05), ink);
        }
        Kind::Area => {
            fill(rr(0.4, 0.15, 0.6, 0.85), a);
            p.text(at(0.5, 0.5), Align2::CENTER_CENTER, "A", FontId::proportional(13.0), ink);
            fill(rr(0.68, 0.55, 0.74, 0.75), a);
            p.rect_stroke(rr(0.66, 0.5, 0.76, 0.8), 1.0, Stroke::new(1.5, pal.danger), StrokeKind::Outside);
        }
        Kind::Density => {
            for (k, f) in [0.2f32, 0.6, 0.9, 0.4, 0.7, 0.3].into_iter().enumerate() {
                let (cx, cy) = (0.2 + 0.2 * (k % 3) as f32, 0.05 + 0.45 * (k / 3) as f32);
                let cell = rr(cx, cy, cx + 0.2, cy + 0.45);
                p.rect_filled(
                    Rect::from_min_max(Pos2::new(cell.left(), cell.bottom() - cell.height() * f), cell.max),
                    0.0,
                    a.gamma_multiply(0.5),
                );
                p.rect_stroke(cell, 0.0, Stroke::new(0.8, pal.border), StrokeKind::Inside);
            }
        }
        Kind::Antenna => {
            fill(rr(0.15, 0.1, 0.85, 0.3), a);
            fill(rr(0.2, 0.7, 0.3, 0.95), b);
            p.line_segment([at(0.25, 0.3), at(0.25, 0.7)], Stroke::new(2.0, pal.text_weak));
        }
        Kind::Flag => {
            for (x0, y0) in [(0.25, 0.2), (0.55, 0.45)] {
                let q = rr(x0, y0, x0 + 0.2, y0 + 0.35);
                fill(q, a);
                p.rect_stroke(q.expand(2.0), 0.0, Stroke::new(1.5, pal.danger), StrokeKind::Middle);
            }
        }
    }
}

/// World (dbu, y up) to screen mapping that fits `bb` into `rect`.
struct Fit {
    rect: Rect,
    bb: IRect,
    s: f32,
}

impl Fit {
    fn new(rect: Rect, bb: IRect) -> Fit {
        let w = (bb.x1 - bb.x0).max(1) as f32;
        let h = (bb.y1 - bb.y0).max(1) as f32;
        let s = (rect.width() / w).min(rect.height() / h);
        Fit { rect, bb, s }
    }

    fn pt(&self, x: i64, y: i64) -> Pos2 {
        let c = self.rect.center();
        let mx = (self.bb.x0 + self.bb.x1) as f32 / 2.0;
        let my = (self.bb.y0 + self.bb.y1) as f32 / 2.0;
        Pos2::new(c.x + (x as f32 - mx) * self.s, c.y - (y as f32 - my) * self.s)
    }

    fn r(&self, r: &IRect) -> Rect {
        Rect::from_two_pos(self.pt(r.x0, r.y0), self.pt(r.x1, r.y1))
    }
}

fn grow(bb: IRect, f: f32) -> IRect {
    let m = (((bb.x1 - bb.x0).max(bb.y1 - bb.y0)) as f32 * f) as i64;
    IRect { x0: bb.x0 - m, y0: bb.y0 - m, x1: bb.x1 + m, y1: bb.y1 + m }
}

fn draw_dim(p: &egui::Painter, a: Pos2, b: Pos2, label: &str, pal: &Palette) {
    let col = pal.text_strong;
    let st = Stroke::new(1.2, col);
    p.line_segment([a, b], st);
    let d = (b - a).normalized();
    let n = Vec2::new(-d.y, d.x);
    for (q, dir) in [(a, d), (b, -d)] {
        p.line_segment([q - n * 5.0, q + n * 5.0], st);
        p.line_segment([q, q + dir * 6.0 + n * 3.0], st);
        p.line_segment([q, q + dir * 6.0 - n * 3.0], st);
    }
    let mid = a + (b - a) / 2.0 + n * 12.0;
    let g = p.layout_no_wrap(label.to_string(), FontId::monospace(11.0), col);
    let r = Rect::from_center_size(mid, g.size() + Vec2::new(6.0, 2.0));
    p.rect_filled(r, 3.0, pal.bg_panel.gamma_multiply(0.9));
    p.galley(r.min + Vec2::new(3.0, 1.0), g, col);
}

fn draw_region(p: &egui::Painter, fit: &Fit, reg: &Region, c: Color32, fill: f32, width: f32) {
    for r in reg.rects() {
        p.rect_filled(fit.r(r), 0.0, c.gamma_multiply(fill));
    }
    for poly in reg.to_polygons() {
        let pts: Vec<Pos2> = poly.iter().map(|&(x, y)| fit.pt(x, y)).collect();
        p.add(egui::Shape::closed_line(pts, Stroke::new(width, c)));
    }
}

fn pattern_card(ui: &mut egui::Ui, run: &PatternRun, tech: &Tech, pal: &Palette) {
    let w = ui.available_width().min(520.0);
    egui::Frame::new().fill(pal.bg_canvas).stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
        ui.set_width(w - 16.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&run.pattern.title).strong().color(pal.text_strong));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (t, c) = match (run.markers.len(), run.as_expected) {
                    (0, true) => ("accepted".to_string(), pal.ok),
                    (n, true) => (format!("flagged {n}"), pal.ok),
                    (0, false) => ("missed!".to_string(), pal.danger),
                    (n, false) => (format!("false flag {n}"), pal.danger),
                };
                chip(ui, &t, c);
            });
        });
        let cap = if run.pattern.expect_fail {
            "One grid step past the limit: the rule must flag it."
        } else {
            "Exactly at the limit: the rule must accept it."
        };
        ui.label(RichText::new(cap).small().color(pal.text_weak));
        let (resp, p) = ui.allocate_painter(Vec2::new(w - 16.0, 220.0), Sense::hover());
        let rect = resp.rect.shrink(10.0);
        let all = run.pattern.shapes.iter().map(|s| s.1).chain(run.markers.iter().copied()).reduce(|a, b| a.union(&b));
        let Some(bb) = all else { return };
        let fit = Fit::new(rect, grow(bb, 0.12));
        let mut by_layer: BTreeMap<usize, Vec<IRect>> = BTreeMap::new();
        for (l, r) in &run.pattern.shapes {
            by_layer.entry(*l).or_default().push(*r);
        }
        for (l, rs) in by_layer {
            draw_region(&p, &fit, &Region::from_rects(rs), color_of(l, tech, pal.dark), 0.35, 1.5);
        }
        for m in &run.markers {
            let r = fit.r(m).expand(2.0);
            p.rect_filled(r, 0.0, pal.danger.gamma_multiply(0.25));
            p.rect_stroke(r, 0.0, Stroke::new(2.0, pal.danger), StrokeKind::Middle);
        }
        for d in &run.pattern.dims {
            draw_dim(&p, fit.pt(d.from.0, d.from.1), fit.pt(d.to.0, d.to.1), &d.label, pal);
        }
        let mut seen = vec![];
        let mut x = resp.rect.left() + 4.0;
        for (l, _) in &run.pattern.shapes {
            if seen.contains(l) {
                continue;
            }
            seen.push(*l);
            let name = tech.layer(*l).map_or("?", |d| d.name.as_str());
            let c = color_of(*l, tech, pal.dark);
            p.rect_filled(Rect::from_min_size(Pos2::new(x, resp.rect.bottom() - 13.0), Vec2::splat(9.0)), 2.0, c);
            let g = p.layout_no_wrap(name.to_string(), FontId::proportional(11.0), pal.text_weak);
            let gw = g.size().x;
            p.galley(Pos2::new(x + 12.0, resp.rect.bottom() - 15.0), g, pal.text_weak);
            x += gw + 24.0;
        }
    });
}

fn preview_card(
    ui: &mut egui::Ui,
    pv: &rule_design::Preview,
    name: &str,
    me: usize,
    tech: &Tech,
    names: &[String],
    pal: &Palette,
) {
    let w = ui.available_width().min(720.0);
    egui::Frame::new().fill(pal.bg_canvas).stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
        ui.set_width(w - 16.0);
        let (resp, p) = ui.allocate_painter(Vec2::new(w - 16.0, 260.0), Sense::hover());
        let rect = resp.rect.shrink(12.0);
        let all = pv.inputs.iter().filter_map(|(_, r)| r.bbox()).chain(pv.result.bbox()).reduce(|a, b| a.union(&b));
        let Some(bb) = all else { return };
        let fit = Fit::new(rect, grow(bb, 0.08));
        let rc = color_of(me, tech, pal.dark);
        draw_region(&p, &fit, &pv.result, rc, 0.55, 2.5);
        for (i, (l, reg)) in pv.inputs.iter().enumerate() {
            let c = color_of(*l, tech, pal.dark);
            for poly in reg.to_polygons() {
                let pts: Vec<Pos2> = poly.iter().map(|&(x, y)| fit.pt(x, y)).collect();
                let mut closed = pts.clone();
                if let Some(f) = pts.first() {
                    closed.push(*f);
                }
                p.extend(egui::Shape::dashed_line(&closed, Stroke::new(1.5, c), 6.0, 4.0));
            }
            let tag = if pv.inputs.len() > 1 { ["A", "B", "C", "D"].get(i).copied().unwrap_or("") } else { "A" };
            let label = format!("{tag} = {}", names.get(*l).map_or("?", |s| s.as_str()));
            p.text(
                resp.rect.left_top() + Vec2::new(6.0, 6.0 + 15.0 * i as f32),
                Align2::LEFT_TOP,
                label,
                FontId::proportional(12.0),
                c,
            );
        }
        p.text(
            resp.rect.right_top() + Vec2::new(-6.0, 6.0),
            Align2::RIGHT_TOP,
            format!("result: {name}"),
            FontId::proportional(12.0),
            rc,
        );
    });
}

/// Illustration of checks without generated patterns (antenna, density, flag, derived layers).
fn sketch(ui: &mut egui::Ui, rule: &Rule, tech: &Tech, names: &[String], pal: &Palette) {
    let w = ui.available_width().min(560.0);
    let (resp, p) = ui.allocate_painter(Vec2::new(w, 200.0), Sense::hover());
    p.rect_filled(resp.rect, 6.0, pal.bg_canvas);
    let r = resp.rect.shrink(16.0);
    let font = FontId::proportional(12.0);
    let nm = |l: usize| names.get(l).cloned().unwrap_or_default();
    match *rule {
        Rule::Density { layer, min, max, window, .. } => {
            let c = color_of(layer, tech, pal.dark);
            let cell = (r.height() / 3.0).min(r.width() / 5.0);
            let fills: [f32; 15] = [0.1, 0.45, 0.7, 0.95, 0.3, 0.55, 0.85, 0.15, 0.5, 0.65, 0.4, 0.25, 0.6, 0.75, 0.35];
            for (k, f) in fills.iter().enumerate() {
                let (cx, cy) = ((k % 5) as f32, (k / 5) as f32);
                let q = Rect::from_min_size(r.min + Vec2::new(cx * cell, cy * cell), Vec2::splat(cell));
                let pct = (f * 100.0) as f64;
                let bad = pct < min || pct > max;
                p.rect_filled(Rect::from_min_size(q.min, Vec2::new(cell, cell * f)), 0.0, c.gamma_multiply(0.5));
                p.rect_stroke(
                    q,
                    0.0,
                    Stroke::new(if bad { 2.0 } else { 0.5 }, if bad { pal.danger } else { pal.border }),
                    StrokeKind::Inside,
                );
                p.text(q.center(), Align2::CENTER_CENTER, format!("{pct:.0}%"), font.clone(), pal.text);
            }
            p.text(
                Pos2::new(r.min.x + 5.0 * cell + 16.0, r.center().y),
                Align2::LEFT_CENTER,
                format!("{} µm windows\nallowed {min}-{max} %\nred: failing windows", um(window, tech)),
                font,
                pal.text,
            );
        }
        Rule::Antenna { layer, ratio, poly, .. } => {
            let mc = color_of(layer, tech, pal.dark);
            let pc = color_of(poly, tech, pal.dark);
            let gate = Rect::from_min_size(Pos2::new(r.left() + 20.0, r.bottom() - 40.0), Vec2::new(30.0, 30.0));
            p.rect_filled(gate, 0.0, pc.gamma_multiply(0.6));
            p.text(gate.center_bottom() + Vec2::new(0.0, 4.0), Align2::CENTER_TOP, "gate", font.clone(), pal.text_weak);
            let metal = Rect::from_min_max(Pos2::new(gate.left(), r.top() + 20.0), Pos2::new(r.right() - 160.0, r.top() + 50.0));
            p.rect_filled(metal, 0.0, mc.gamma_multiply(0.5));
            p.line_segment([gate.center_top(), Pos2::new(gate.center().x, metal.bottom())], Stroke::new(3.0, pal.text_weak));
            p.text(
                Pos2::new(r.right() - 150.0, r.center().y),
                Align2::LEFT_CENTER,
                format!("area({}) / area(gate)\nmust stay <= {ratio}\nfor nets without diffusion", nm(layer)),
                font,
                pal.text,
            );
        }
        _ => {
            let l = rule.checked_layer();
            let c = color_of(l, tech, pal.dark);
            for (k, q) in [(0.0, 0.3), (0.35, 0.55), (0.7, 0.25)] {
                let x = Rect::from_min_size(r.min + Vec2::new(r.width() * k * 0.6, r.height() * q * 0.5), Vec2::new(70.0, 50.0));
                p.rect_filled(x, 0.0, c.gamma_multiply(0.45));
                p.rect_stroke(x.expand(2.0), 0.0, Stroke::new(2.0, pal.danger), StrokeKind::Middle);
            }
            p.text(r.right_center(), Align2::RIGHT_CENTER, format!("every piece of {}\nis reported", nm(l)), font, pal.text);
        }
    }
}
