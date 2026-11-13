//! Helpers for designing rule decks visually: writing decks, linting, self-test patterns and
//! derived-layer previews.
//!
//! [`deck_text`] writes a [`Deck`] back as native deck text (`#@` descriptions included), so a
//! deck edited in a form survives a round trip through [`drc::parse_deck`]. [`lint`] reports rules
//! that never fire, duplicates, off-grid values and gaps against the technology. [`patterns`]
//! builds a tiny layout exactly at a rule's limit and one a grid step inside it, which
//! [`run_patterns`] checks with the DRC engine. [`derived_preview`] shows what a derived-layer
//! expression does on sample geometry.

use crate::drc::{self, Deck, LayerExpr, NamedRule, Rule};
use crate::geom::IRect;
use crate::hier::NoCells;
use crate::layout::Layout;
use crate::region::{BoolOp, Region};
use crate::tech::Tech;

fn um(v: i64, tech: &Tech) -> String {
    format!("{}", v as f64 / tech.dbu_per_um.max(1) as f64)
}

fn um2(v: i128, tech: &Tech) -> String {
    let d = tech.dbu_per_um.max(1) as f64;
    format!("{}", v as f64 / (d * d))
}

/// Native deck text for `deck`.
pub fn deck_text(deck: &Deck, tech: &Tech) -> String {
    let names = deck.layer_names(tech);
    let mut out = String::from("# DRC deck written by the IC Studio rule designer (lengths in µm)\n");
    for d in &deck.derived {
        out.push_str(&d.deck_line(tech, &names));
        out.push('\n');
    }
    for r in &deck.rules {
        if let Some(c) = &r.comment {
            out.push_str(&format!("#@ {c}\n"));
        }
        if r.name != r.rule.auto_name(&names) {
            out.push_str(&format!("{}: ", r.name));
        }
        out.push_str(&r.rule.deck_line(tech, &names));
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What a [`Finding`] is about; indices into [`Deck::rules`] / [`Deck::derived`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Deck,
    Rule(usize),
    Derived(usize),
}

/// A one-click repair for a [`Finding`].
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// Replace the subject rule with this one (snapped or technology value).
    Replace(Rule),
    /// Rename the subject rule.
    Rename(String),
    /// Remove the subject rule.
    RemoveRule,
    /// Remove the subject derived layer.
    RemoveDerived,
    /// Add this rule to the deck.
    AddRule(Rule),
}

impl Fix {
    /// Button text for the fix.
    pub fn label(&self, tech: &Tech, names: &[String]) -> String {
        match self {
            Fix::Replace(r) => match length(r) {
                Some(v) => format!("Set to {} µm", um(v, tech)),
                None => format!("Change to {}", r.deck_line(tech, names)),
            },
            Fix::Rename(n) => format!("Rename to {n}"),
            Fix::RemoveRule => "Remove this rule".into(),
            Fix::RemoveDerived => "Remove this layer".into(),
            Fix::AddRule(r) => format!("Add {}", r.auto_name(names)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub subject: Subject,
    pub message: String,
    pub fix: Option<Fix>,
}

/// `rule` with its length value replaced by `v`.
fn with_length(rule: &Rule, v: i64) -> Rule {
    let mut r = rule.clone();
    if let Rule::Width { min, .. }
    | Rule::Space { min, .. }
    | Rule::Separation { min, .. }
    | Rule::Enclosure { min, .. }
    | Rule::Extension { min, .. } = &mut r
    {
        *min = v;
    }
    r
}

/// Every layer index a rule reads, mutably.
pub fn rule_layers_mut(r: &mut Rule) -> Vec<&mut usize> {
    match r {
        Rule::Width { layer, .. }
        | Rule::Space { layer, .. }
        | Rule::Area { layer, .. }
        | Rule::Density { layer, .. }
        | Rule::Flag { layer } => vec![layer],
        Rule::Separation { a, b, .. } | Rule::Extension { a, b, .. } => vec![a, b],
        Rule::Enclosure { inner, outer, .. } => vec![inner, outer],
        Rule::Antenna { layer, poly, active, vias, .. } => {
            let mut v = vec![layer, poly, active];
            for (b, c, t) in vias.iter_mut() {
                v.extend([b, c, t]);
            }
            v
        }
    }
}

/// Every layer index a derived-layer expression reads, mutably.
pub fn expr_layers_mut(e: &mut LayerExpr) -> Vec<&mut usize> {
    match e {
        LayerExpr::Bool { a, b, .. } | LayerExpr::Select { a, b, .. } => vec![a, b],
        LayerExpr::Size { a, .. }
        | LayerExpr::Wide { a, .. }
        | LayerExpr::Holes { a }
        | LayerExpr::Extents { a }
        | LayerExpr::AreaRange { a, .. } => vec![a],
        LayerExpr::Union(v) => v.iter_mut().collect(),
        LayerExpr::Check(r) => rule_layers_mut(r),
    }
}

/// Remove derived layer `k`, renumbering the layers after it. The caller makes sure nothing
/// reads it.
pub fn remove_derived(deck: &mut Deck, k: usize, tech: &Tech) {
    let me = tech.layers.len() + k;
    deck.derived.remove(k);
    let shift = |l: &mut usize| {
        if *l > me {
            *l -= 1;
        }
    };
    for r in &mut deck.rules {
        rule_layers_mut(&mut r.rule).into_iter().for_each(shift);
    }
    for d in &mut deck.derived {
        expr_layers_mut(&mut d.expr).into_iter().for_each(shift);
    }
}

/// Carry out `f.fix` on `deck`; false when the finding has no fix or no longer applies.
pub fn apply_fix(deck: &mut Deck, f: &Finding, tech: &Tech) -> bool {
    let Some(fix) = &f.fix else { return false };
    let names = deck.layer_names(tech);
    match (fix, f.subject) {
        (Fix::Replace(r), Subject::Rule(i)) if i < deck.rules.len() => {
            let nr = &mut deck.rules[i];
            if nr.name == nr.rule.auto_name(&names) {
                nr.name = r.auto_name(&names);
            }
            nr.rule = r.clone();
        }
        (Fix::Rename(n), Subject::Rule(i)) if i < deck.rules.len() => deck.rules[i].name = n.clone(),
        (Fix::RemoveRule, Subject::Rule(i)) if i < deck.rules.len() => {
            deck.rules.remove(i);
        }
        (Fix::RemoveDerived, Subject::Derived(k)) if k < deck.derived.len() => remove_derived(deck, k, tech),
        (Fix::AddRule(r), _) => deck.rules.push(NamedRule { name: r.auto_name(&names), rule: r.clone(), comment: None }),
        _ => return false,
    }
    true
}

/// Whether [`fix_all`] applies this finding's fix: every fix of an error or warning, and the
/// missing rules among the notes.
pub fn auto_fixable(f: &Finding) -> bool {
    f.fix.is_some() && (f.severity != Severity::Info || matches!(f.fix, Some(Fix::AddRule(_))))
}

/// Apply every [`auto_fixable`] fix until none is left; returns how many were applied.
pub fn fix_all(deck: &mut Deck, tech: &Tech) -> usize {
    let mut n = 0;
    for _ in 0..1000 {
        match lint(deck, tech).into_iter().find(auto_fixable) {
            Some(f) if apply_fix(deck, &f, tech) => n += 1,
            _ => break,
        }
    }
    n
}

fn unique_name(base: &str, deck: &Deck) -> String {
    (2..).map(|k| format!("{base}_{k}")).find(|n| deck.rules.iter().all(|r| r.name != *n)).expect("unbounded")
}

/// The length a rule is measured against (dbu), for the rules that have one.
fn length(rule: &Rule) -> Option<i64> {
    match *rule {
        Rule::Width { min, .. }
        | Rule::Space { min, .. }
        | Rule::Separation { min, .. }
        | Rule::Enclosure { min, .. }
        | Rule::Extension { min, .. } => Some(min),
        _ => None,
    }
}

/// Layers that identify the check for duplicate detection.
fn check_key(rule: &Rule) -> (std::mem::Discriminant<Rule>, Vec<usize>) {
    let layers = match rule {
        Rule::Antenna { layer, .. } | Rule::Density { layer, .. } => vec![*layer],
        r => r.layers(),
    };
    (std::mem::discriminant(rule), layers)
}

/// Which of two duplicate rules is stricter: `Some(true)` when `later` is, `None` when equal
/// or not comparable, with the two values as text.
fn stricter(earlier: &Rule, later: &Rule, tech: &Tech) -> Option<(bool, String, String)> {
    match (earlier, later) {
        (Rule::Area { min: a, .. }, Rule::Area { min: b, .. }) if a != b => {
            Some((b > a, format!("{} µm²", um2(*a, tech)), format!("{} µm²", um2(*b, tech))))
        }
        (Rule::Antenna { ratio: a, .. }, Rule::Antenna { ratio: b, .. }) if a != b => Some((b < a, a.to_string(), b.to_string())),
        _ => match (length(earlier), length(later)) {
            (Some(a), Some(b)) if a != b => Some((b > a, format!("{} µm", um(a, tech)), format!("{} µm", um(b, tech)))),
            _ => None,
        },
    }
}

/// Problems and suggestions for `deck`, most severe first (stable within a severity).
pub fn lint(deck: &Deck, tech: &Tech) -> Vec<Finding> {
    let names = deck.layer_names(tech);
    let n = tech.layers.len();
    let name = |l: usize| names.get(l).cloned().unwrap_or_else(|| format!("#{l}"));
    let mut out = Vec::new();
    let mut push = |severity, subject, message: String, fix: Option<Fix>| out.push(Finding { severity, subject, message, fix });

    if deck.rules.is_empty() {
        push(Severity::Info, Subject::Deck, "The deck has no rules".into(), None);
    }
    for (i, nr) in deck.rules.iter().enumerate() {
        let s = Subject::Rule(i);
        let NamedRule { name: rname, rule, .. } = nr;
        let tech_value = match *rule {
            Rule::Width { layer, .. } if layer < n => Some(tech.layers[layer].min_width).filter(|&v| v > 0),
            Rule::Space { layer, .. } if layer < n => Some(tech.layers[layer].min_spacing).filter(|&v| v > 0),
            _ => None,
        };
        if rname.is_empty() || rname.chars().any(|c| c.is_whitespace() || ":#=".contains(c)) {
            let clean: String =
                rname.chars().filter(|c| !":#=".contains(*c)).map(|c| if c.is_whitespace() { '_' } else { c }).collect();
            let clean = if clean.is_empty() { rule.auto_name(&names) } else { clean };
            let clean = if deck.rules.iter().any(|r| r.name == clean) { unique_name(&clean, deck) } else { clean };
            let msg = format!("Invalid rule name '{rname}' (no spaces, ':', '#' or '=')");
            push(Severity::Error, s, msg, Some(Fix::Rename(clean)));
        }
        if let Some(j) = deck.rules[..i].iter().position(|o| o.name == *rname) {
            let fix = Fix::Rename(unique_name(rname, deck));
            push(Severity::Error, s, format!("Duplicate rule name '{rname}' (also rule {})", j + 1), Some(fix));
        }
        let zero = length(rule) == Some(0) || matches!(rule, Rule::Area { min: 0, .. });
        if zero {
            let fix = tech_value.map(|v| Fix::Replace(with_length(rule, v)));
            push(Severity::Error, s, "The value is 0, so the rule never fires".into(), fix);
        }
        match *rule {
            Rule::Enclosure { inner, outer, .. } if inner == outer => {
                push(Severity::Error, s, format!("Enclosure of {} by itself never fires", name(inner)), None);
            }
            Rule::Extension { a, b, .. } if a == b => {
                push(Severity::Error, s, format!("Extension of {} past itself never fires", name(a)), None);
            }
            Rule::Separation { a, b, min } if a == b => {
                let fix = Fix::Replace(Rule::Space { layer: a, min });
                let msg = format!("Separation of {} from itself never fires; use space", name(a));
                push(Severity::Warning, s, msg, Some(fix));
            }
            _ => {}
        }
        if let Some(v) = length(rule).filter(|&v| tech.grid > 1 && v > 0 && v % tech.grid != 0) {
            let lo = v / tech.grid * tech.grid;
            let hi = lo + tech.grid;
            let suggest =
                if lo > 0 { format!("{} or {} µm", um(lo, tech), um(hi, tech)) } else { format!("{} µm", um(hi, tech)) };
            let msg = format!("{} µm is off the {} µm grid (use {suggest})", um(v, tech), um(tech.grid, tech));
            push(Severity::Warning, s, msg, Some(Fix::Replace(with_length(rule, hi))));
        }
        let key = check_key(rule);
        if let Some(e) = deck.rules[..i].iter().find(|o| check_key(&o.rule) == key) {
            let (msg, fix) = match stricter(&e.rule, rule, tech) {
                Some((true, a, b)) => (format!("Same check as {}; this rule is stricter ({b} vs {a})", e.name), None),
                Some((false, a, b)) => {
                    (format!("Same check as {}, which is stricter ({a} vs {b})", e.name), Some(Fix::RemoveRule))
                }
                None if e.rule == *rule => (format!("Same check as {} with the same value", e.name), Some(Fix::RemoveRule)),
                None => (format!("Same check as {}", e.name), None),
            };
            push(Severity::Warning, s, msg, fix);
        }
        let tech_min = match *rule {
            Rule::Width { layer, min } if layer < n => Some((min, tech.layers[layer].min_width, "width")),
            Rule::Space { layer, min } if layer < n => Some((min, tech.layers[layer].min_spacing, "spacing")),
            _ => None,
        };
        if let Some((v, t, what)) = tech_min.filter(|&(v, t, _)| t > 0 && v != t && v > 0) {
            push(
                Severity::Info,
                s,
                format!(
                    "{} µm differs from the technology's minimum {what}; PCells and the router use {} µm",
                    um(v, tech),
                    um(t, tech)
                ),
                Some(Fix::Replace(with_length(rule, t))),
            );
        }
    }

    for (k, d) in deck.derived.iter().enumerate() {
        let s = Subject::Derived(k);
        match d.expr {
            LayerExpr::Union(_) => {
                push(Severity::Error, s, format!("'{}' is a union, which has no native deck syntax", d.name), None);
            }
            LayerExpr::Check(_) => {
                push(Severity::Error, s, format!("'{}' is a rule check, which has no native deck syntax", d.name), None);
            }
            _ => {}
        }
        let idx = n + k;
        let used = deck.rules.iter().any(|r| r.rule.layers().contains(&idx))
            || deck.derived[k + 1..].iter().any(|o| o.expr.inputs().contains(&idx));
        if !used {
            let msg = format!("Derived layer '{}' is not used by any rule", d.name);
            push(Severity::Warning, s, msg, Some(Fix::RemoveDerived));
        }
    }

    let has_enc = |cut: usize, outer: usize| {
        deck.rules.iter().any(|r| matches!(r.rule, Rule::Enclosure { inner, outer: o, .. } if inner == cut && o == outer))
    };
    let mut seen = Vec::new();
    for v in &tech.vias {
        let (Some(cut), Some(bottom), Some(top)) =
            (tech.layer_index(&v.cut), tech.layer_index(&v.bottom), tech.layer_index(&v.top))
        else {
            continue;
        };
        for (outer, enc) in [(bottom, v.enc_bottom), (top, v.enc_top)] {
            if !has_enc(cut, outer) && !seen.contains(&(cut, outer)) {
                seen.push((cut, outer));
                push(
                    Severity::Warning,
                    Subject::Deck,
                    format!("Via {}: no enclosure rule for {} by {}", v.name, name(cut), name(outer)),
                    (enc > 0).then_some(Fix::AddRule(Rule::Enclosure { inner: cut, outer, min: enc })),
                );
            }
        }
    }

    for (l, def) in tech.layers.iter().enumerate().filter(|(_, d)| d.routing) {
        let has = |f: fn(&Rule, usize) -> bool| deck.rules.iter().any(|r| f(&r.rule, l));
        if !has(|r, l| matches!(*r, Rule::Width { layer, .. } if layer == l)) {
            let fix = (def.min_width > 0).then_some(Fix::AddRule(Rule::Width { layer: l, min: def.min_width }));
            push(Severity::Info, Subject::Deck, format!("Routing layer {} has no width rule", def.name), fix);
        }
        if !has(|r, l| matches!(*r, Rule::Space { layer, .. } if layer == l)) {
            let fix = (def.min_spacing > 0).then_some(Fix::AddRule(Rule::Space { layer: l, min: def.min_spacing }));
            push(Severity::Info, Subject::Deck, format!("Routing layer {} has no space rule", def.name), fix);
        }
    }

    out.sort_by_key(|f| f.severity);
    out
}

/// A dimension annotation (dbu), drawn from `from` to `to` with `label`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dim {
    pub from: (i64, i64),
    pub to: (i64, i64),
    pub label: String,
}

/// A tiny layout that a rule should accept (`expect_fail == false`) or flag.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub title: String,
    pub expect_fail: bool,
    pub shapes: Vec<(usize, IRect)>,
    pub dims: Vec<Dim>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternRun {
    pub pattern: Pattern,
    pub markers: Vec<IRect>,
    pub as_expected: bool,
}

fn ceil_sqrt(v: i128) -> i128 {
    let mut r = (v.max(0) as f64).sqrt() as i128;
    while r * r < v {
        r += 1;
    }
    while r > 0 && (r - 1) * (r - 1) >= v {
        r -= 1;
    }
    r
}

/// A passing pattern exactly at the rule's limit and a failing one a grid step inside it,
/// or `None` for rules that cannot be illustrated with technology layers alone.
pub fn patterns(rule: &Rule, tech: &Tech) -> Option<Vec<Pattern>> {
    let n = tech.layers.len();
    if rule.layers().iter().any(|&l| l >= n) {
        return None;
    }
    let g = tech.grid.max(1);
    let minw = |l: usize| tech.layers[l].min_width.max(g);
    let rect = IRect::new;
    let dim = |from, to, v: i64| Dim { from, to, label: format!("{} µm", um(v, tech)) };
    let pattern = |fail: bool, v: i64, shapes: Vec<(usize, IRect)>, dims: Vec<Dim>| Pattern {
        title: format!("{} at {} µm", if fail { "Fail" } else { "Pass" }, um(v, tech)),
        expect_fail: fail,
        shapes,
        dims,
    };
    if let Rule::Area { layer, min } = *rule {
        if min <= 0 {
            return None;
        }
        let gg = g as i128;
        let s = (ceil_sqrt(min) + gg - 1) / gg * gg;
        let square = |fail: bool, s: i128| {
            let s = s as i64;
            let a = um2(s as i128 * s as i128, tech);
            Pattern {
                title: format!("{} at {a} µm²", if fail { "Fail" } else { "Pass" }),
                expect_fail: fail,
                shapes: vec![(layer, rect(0, 0, s, s))],
                dims: vec![Dim { from: (0, s / 2), to: (s, s / 2), label: format!("{} µm ({a} µm²)", um(s, tech)) }],
            }
        };
        let mut v = vec![square(false, s)];
        if s - gg > 0 {
            v.push(square(true, s - gg));
        }
        return Some(v);
    }
    let m = length(rule)?;
    if m <= g {
        return None;
    }
    let both = |make: &dyn Fn(i64, bool) -> Pattern| Some(vec![make(m, false), make(m - g, true)]);
    match *rule {
        Rule::Width { layer, .. } => both(&|w, fail| {
            let len = (4 * m).max(10 * g);
            pattern(fail, w, vec![(layer, rect(0, 0, w, len))], vec![dim((0, len / 2), (w, len / 2), w)])
        }),
        Rule::Space { layer, .. } => {
            let w = m.max(minw(layer));
            let len = 3 * w;
            both(&|gap, fail| {
                let shapes = vec![(layer, rect(0, 0, w, len)), (layer, rect(w + gap, 0, 2 * w + gap, len))];
                pattern(fail, gap, shapes, vec![dim((w, len / 2), (w + gap, len / 2), gap)])
            })
        }
        Rule::Separation { a, b, .. } if a != b => {
            let (wa, wb) = (minw(a), minw(b));
            let len = 3 * wa.max(wb).max(m);
            both(&|gap, fail| {
                let shapes = vec![(a, rect(0, 0, wa, len)), (b, rect(wa + gap, 0, wa + gap + wb, len))];
                pattern(fail, gap, shapes, vec![dim((wa, len / 2), (wa + gap, len / 2), gap)])
            })
        }
        Rule::Enclosure { inner, outer, .. } if inner != outer => {
            let s = minw(inner).max(m);
            both(&|e, fail| {
                let shapes = vec![(outer, rect(-m, -m, s + e, s + m)), (inner, rect(0, 0, s, s))];
                pattern(fail, e, shapes, vec![dim((s, s / 2), (s + e, s / 2), e)])
            })
        }
        Rule::Extension { a, b, .. } if a != b => {
            let wa = minw(a);
            let side = (2 * m).max(minw(b));
            let h = (2 * wa).max(2 * m).max(minw(b));
            both(&|e, fail| {
                let shapes = vec![(b, rect(0, 0, 2 * side + wa, h)), (a, rect(side, -e, side + wa, h + e))];
                let x = side + wa / 2;
                pattern(fail, e, shapes, vec![dim((x, h), (x, h + e), e)])
            })
        }
        _ => None,
    }
}

/// Run each pattern through [`drc::run`] with just this rule.
pub fn run_patterns(rule: &Rule, tech: &Tech) -> Vec<PatternRun> {
    let deck = Deck { derived: vec![], rules: vec![NamedRule { name: "rule".into(), rule: rule.clone(), comment: None }] };
    patterns(rule, tech)
        .unwrap_or_default()
        .into_iter()
        .map(|pattern| {
            let mut lay = Layout::default();
            for &(l, r) in &pattern.shapes {
                lay.add_rect(l, r.x0, r.y0, r.x1, r.y1);
            }
            let markers: Vec<IRect> = drc::run(&deck, &lay, &NoCells, tech).violations.into_iter().map(|v| v.marker).collect();
            let as_expected = markers.is_empty() != pattern.expect_fail;
            PatternRun { pattern, markers, as_expected }
        })
        .collect()
}

/// Sample inputs of a derived layer and the result of its expression on them.
#[derive(Debug, Clone)]
pub struct Preview {
    pub inputs: Vec<(usize, Region)>,
    pub result: Region,
}

fn region(rects: &[(i64, i64, i64, i64)]) -> Region {
    Region::from_rects(rects.iter().map(|&(x0, y0, x1, y1)| IRect::new(x0, y0, x1, y1)))
}

fn l_shape(u: i64) -> Region {
    region(&[(0, 0, 3 * u, u), (0, 0, u, 3 * u)])
}

/// Squares with areas below, inside and above `min <= area < max` (dbu²), side by side.
fn area_samples(min: Option<i128>, max: Option<i128>) -> Region {
    let lo = min.unwrap_or(0);
    let mut sides: Vec<i128> = Vec::new();
    if lo > 1 {
        sides.push(ceil_sqrt(lo) - 1);
    }
    let target = match max {
        Some(hi) if lo > 0 => lo + (hi - lo) / 2,
        Some(hi) => hi / 2,
        None if lo > 0 => lo * 2,
        None => 1_000_000,
    };
    let inside = [ceil_sqrt(target), ceil_sqrt(lo)].into_iter().find(|&s| s * s >= lo && max.is_none_or(|hi| s * s < hi));
    sides.extend(inside);
    if let Some(hi) = max {
        sides.push(ceil_sqrt(hi));
    }
    if lo == 0 && max.is_none() {
        sides = vec![500, 1000, 1500];
    }
    let sides: Vec<i64> = sides.into_iter().filter(|&s| s > 0).map(|s| s.min(i64::MAX as i128 / 8) as i64).collect();
    let gap = sides.iter().copied().max().unwrap_or(1) / 2 + 1;
    let mut x = 0;
    let mut rects = Vec::new();
    for s in sides {
        rects.push(IRect::new(x, 0, x + s, s));
        x += s + gap;
    }
    Region::from_rects(rects)
}

/// `None` for `Check` expressions.
pub fn derived_preview(expr: &LayerExpr) -> Option<Preview> {
    let pair = |a: usize, ra: Region, b: usize, rb: Region| {
        if a == b { vec![(a, ra.boolean(&rb, BoolOp::Or))] } else { vec![(a, ra), (b, rb)] }
    };
    let inputs: Vec<(usize, Region)> = match *expr {
        LayerExpr::Bool { a, b, .. } => pair(a, region(&[(0, 0, 2000, 1400)]), b, region(&[(1200, 700, 3200, 2100)])),
        LayerExpr::Select { a, b, .. } => {
            let pieces = region(&[
                (2500, 800, 3500, 1300),
                (600, 600, 1400, 1400),
                (-800, 300, 0, 900),
                (1000, 2600, 2000, 3200),
                (4600, 200, 6200, 1400),
            ]);
            pair(a, pieces, b, region(&[(0, 0, 3000, 2000), (5000, 500, 5800, 1100)]))
        }
        LayerExpr::Size { a, by } => vec![(a, l_shape((4 * by.abs()).max(400)))],
        LayerExpr::Wide { a, min } => {
            let s = (2 * min).max(400);
            let w = (min / 2).max(1);
            let neck = s / 2;
            vec![(a, region(&[(0, 0, s, s), (s, (s - w) / 2, s + neck, (s + w) / 2), (s + neck, 0, 2 * s + neck, s)]))]
        }
        LayerExpr::Holes { a } => {
            let ring = region(&[(0, 0, 2000, 2000)]).boolean(&region(&[(600, 600, 1400, 1400)]), BoolOp::Not);
            vec![(a, ring.boolean(&region(&[(2600, 0, 3600, 1000)]), BoolOp::Or))]
        }
        LayerExpr::Extents { a } => vec![(a, l_shape(1000).boolean(&region(&[(3600, 2000, 4400, 2600)]), BoolOp::Or))],
        LayerExpr::AreaRange { a, min, max } => vec![(a, area_samples(min, max))],
        LayerExpr::Union(ref v) => {
            let mut out: Vec<(usize, Region)> = Vec::new();
            for (k, &l) in v.iter().enumerate() {
                if out.iter().any(|(i, _)| *i == l) {
                    continue;
                }
                let k = k as i64;
                out.push((l, region(&[(k * 800, k * 500, k * 800 + 1500, k * 500 + 1000)])));
            }
            out
        }
        LayerExpr::Check(_) => return None,
    };
    let len = inputs.iter().map(|(l, _)| l + 1).max().unwrap_or(0);
    let mut layers = vec![Region::default(); len];
    for (l, r) in &inputs {
        layers[*l] = r.clone();
    }
    let result = expr.evaluate(&layers);
    Some(Preview { inputs, result })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::{DerivedLayer, Select, default_deck_text, parse_deck};

    fn tech() -> Tech {
        Tech::generic()
    }

    fn select_count(how: Select, not: bool) -> usize {
        derived_preview(&LayerExpr::Select { a: 0, how, not, b: 1 }).unwrap().result.parts().len()
    }

    #[test]
    fn round_trip_default_deck() {
        let t = tech();
        let d = parse_deck(&default_deck_text(&t), &t).unwrap();
        let text = deck_text(&d, &t);
        assert!(text.starts_with("# DRC deck written by the IC Studio rule designer"));
        assert_eq!(parse_deck(&text, &t), Ok(d));
    }

    #[test]
    fn round_trip_hand_written_deck() {
        let t = tech();
        let src = "gate = poly and active\n\
                   big = metal1 size 0.1\n\
                   m1w = metal1 wide 10\n\
                   sel = contact not inside gate\n\
                   h = holes metal1\n\
                   e = extents big\n\
                   ar = metal1 area 0.05 inf\n\
                   #@ Gates must be wide\n\
                   GATE.1: width gate 0.18\n\
                   space big 0.3\n\
                   separation m1w metal1 0.6\n\
                   #@ Contacts in active\n\
                   enclosure contact active 0.1\n\
                   extension poly active 0.22\n\
                   area ar 0.2\n\
                   antenna metal2 400\n\
                   M1.DEN: density metal1 20 80 100 50\n\
                   flag sel\n\
                   flag h\n\
                   flag e";
        let d = parse_deck(src, &t).unwrap();
        assert_eq!(d.rules[0].comment.as_deref(), Some("Gates must be wide"));
        assert_eq!(d.rules[1].comment, None);
        assert_eq!(d.rules[3].comment.as_deref(), Some("Contacts in active"));
        let text = deck_text(&d, &t);
        assert!(text.contains("#@ Gates must be wide\nGATE.1: width gate 0.18\n"));
        assert!(text.contains("\nspace big 0.3\n"));
        assert_eq!(parse_deck(&text, &t), Ok(d));
    }

    #[test]
    fn comment_waits_for_a_rule() {
        let t = tech();
        let d = parse_deck("#@ note\ng = poly and active\nwidth g 0.2\nwidth poly 0.2", &t).unwrap();
        assert_eq!(d.rules[0].comment.as_deref(), Some("note"));
        assert_eq!(d.rules[1].comment, None);
    }

    #[test]
    fn default_deck_is_clean() {
        let t = tech();
        let d = parse_deck(&default_deck_text(&t), &t).unwrap();
        let f = lint(&d, &t);
        assert!(f.iter().all(|f| f.severity == Severity::Info), "{f:?}");
    }

    #[test]
    fn lint_findings() {
        let t = tech();
        let src = "width metal1 0\n\
                   X: width metal1 0.225\n\
                   X: space metal1 0.3\n\
                   width metal1 0.25\n\
                   separation metal1 metal1 0.3\n\
                   enclosure contact contact 0.1\n\
                   unused = poly and active";
        let d = parse_deck(src, &t).unwrap();
        let f = lint(&d, &t);
        let has = |sev: Severity, subj: Subject, text: &str| {
            f.iter().any(|x| x.severity == sev && x.subject == subj && x.message.contains(text))
        };
        assert!(has(Severity::Error, Subject::Rule(0), "never fires"));
        assert!(has(Severity::Error, Subject::Rule(2), "Duplicate rule name 'X'"));
        assert!(has(Severity::Error, Subject::Rule(5), "by itself"));
        assert!(has(Severity::Warning, Subject::Rule(1), "0.22 or 0.23 µm"));
        assert!(has(Severity::Warning, Subject::Rule(1), "Same check as metal1.W"));
        assert!(
            has(Severity::Warning, Subject::Rule(3), "X, which is stricter")
                || has(Severity::Warning, Subject::Rule(3), "this rule is stricter")
        );
        assert!(has(Severity::Warning, Subject::Rule(4), "use space"));
        assert!(has(Severity::Warning, Subject::Derived(0), "not used"));
        assert!(has(Severity::Warning, Subject::Deck, "no enclosure rule for via1 by metal1"));
        assert!(has(Severity::Info, Subject::Deck, "Routing layer poly has no width rule"));
        assert!(has(Severity::Info, Subject::Rule(3), "PCells and the router use 0.23 µm"));
        assert!(f.windows(2).all(|w| w[0].severity <= w[1].severity));
        assert!(lint(&Deck::default(), &t).iter().any(|f| f.message == "The deck has no rules"));

        let mut bad = d.clone();
        bad.rules[0].name = "a b".into();
        bad.derived.push(DerivedLayer { name: "u".into(), expr: LayerExpr::Union(vec![0, 1]) });
        let f = lint(&bad, &t);
        assert!(f.iter().any(|x| x.subject == Subject::Rule(0) && x.message.starts_with("Invalid rule name")));
        assert!(f.iter().any(|x| x.subject == Subject::Derived(1) && x.severity == Severity::Error));
    }

    #[test]
    fn fixes_repair_the_deck() {
        let t = tech();
        let src = "width metal1 0\n\
                   X: width metal1 0.225\n\
                   X: space metal1 0.3\n\
                   separation metal2 metal2 0.3\n\
                   unused = poly and active\n\
                   big = metal1 size 0.1\n\
                   flag big";
        let mut d = parse_deck(src, &t).unwrap();
        assert!(lint(&d, &t).iter().filter(|f| f.severity == Severity::Error).all(|f| f.fix.is_some()));
        assert!(fix_all(&mut d, &t) >= 5);
        let f = lint(&d, &t);
        assert!(f.iter().all(|f| !auto_fixable(f)), "{f:?}");
        assert!(f.iter().all(|f| f.severity == Severity::Info), "{f:?}");
        assert_eq!(d.derived.len(), 1);
        assert!(d.rules.iter().any(|r| r.rule == Rule::Flag { layer: t.layers.len() }));
        let m1 = t.layer_index("metal1").unwrap();
        assert_eq!(d.rules.iter().filter(|r| matches!(r.rule, Rule::Width { layer, .. } if layer == m1)).count(), 1);
        assert!(d.rules.iter().any(|r| r.rule == Rule::Space { layer: t.layer_index("metal2").unwrap(), min: 300 }));
        assert!(d.rules.iter().any(|r| r.name == "X_2"));
        assert!(parse_deck(&deck_text(&d, &t), &t).is_ok());
    }

    #[test]
    fn default_deck_patterns_behave() {
        let t = tech();
        let d = parse_deck(&default_deck_text(&t), &t).unwrap();
        let mut covered = 0;
        for nr in &d.rules {
            let runs = run_patterns(&nr.rule, &t);
            if patterns(&nr.rule, &t).is_some() {
                assert!(!runs.is_empty(), "{}", nr.name);
                covered += 1;
            }
            for r in &runs {
                assert!(r.as_expected, "{}: {} gave {:?}", nr.name, r.pattern.title, r.markers);
            }
        }
        assert!(covered >= 20, "{covered}");
    }

    #[test]
    fn hand_written_patterns_behave() {
        let t = tech();
        let d = parse_deck(
            "separation poly contact 0.16\nseparation nwell active 0.43\narea metal1 0.1\nwidth metal1 0.231\nspace metal2 0.285",
            &t,
        )
        .unwrap();
        for nr in &d.rules {
            let runs = run_patterns(&nr.rule, &t);
            assert_eq!(runs.len(), 2, "{}", nr.name);
            for r in &runs {
                assert!(r.as_expected, "{}: {} gave {:?}", nr.name, r.pattern.title, r.markers);
            }
        }
        let area = patterns(&d.rules[2].rule, &t).unwrap();
        assert_eq!(area[0].title, "Pass at 0.1024 µm²");
        assert_eq!(area[1].title, "Fail at 0.0961 µm²");
        assert!(patterns(&Rule::Flag { layer: 0 }, &t).is_none());
        assert!(patterns(&Rule::Width { layer: t.layers.len(), min: 300 }, &t).is_none());
        assert_eq!(patterns(&Rule::Width { layer: 0, min: 230 }, &t).unwrap()[1].title, "Fail at 0.22 µm");
    }

    #[test]
    fn previews() {
        assert_eq!(select_count(Select::Inside, false), 1);
        assert_eq!(select_count(Select::Interact, false), 4);
        assert_eq!(select_count(Select::Interact, true), 1);
        assert_eq!(select_count(Select::Outside, false), 2);
        assert_eq!(select_count(Select::Touch, false), 1);
        assert_eq!(select_count(Select::Enclose, false), 1);
        let outside = derived_preview(&LayerExpr::Select { a: 0, how: Select::Interact, not: true, b: 1 }).unwrap();
        assert_eq!(outside.result.bbox(), Some(IRect::new(1000, 2600, 2000, 3200)));

        for op in [BoolOp::And, BoolOp::Or, BoolOp::Not, BoolOp::Xor] {
            let p = derived_preview(&LayerExpr::Bool { a: 2, op, b: 5 }).unwrap();
            assert_eq!(p.inputs.len(), 2);
            assert!(!p.result.is_empty(), "{op:?}");
        }
        let same = derived_preview(&LayerExpr::Bool { a: 3, op: BoolOp::Or, b: 3 }).unwrap();
        assert_eq!(same.inputs.len(), 1);

        let input_area = |p: &Preview| p.inputs[0].1.area();
        let grow = derived_preview(&LayerExpr::Size { a: 0, by: 200 }).unwrap();
        assert!(grow.result.area() > input_area(&grow));
        let shrink = derived_preview(&LayerExpr::Size { a: 0, by: -300 }).unwrap();
        assert!(!shrink.result.is_empty() && shrink.result.area() < input_area(&shrink));

        let wide = derived_preview(&LayerExpr::Wide { a: 0, min: 1000 }).unwrap();
        assert_eq!(wide.result.parts().len(), 2);
        assert!(wide.result.area() < input_area(&wide));

        let holes = derived_preview(&LayerExpr::Holes { a: 0 }).unwrap();
        assert_eq!(holes.result, region(&[(600, 600, 1400, 1400)]));
        let ext = derived_preview(&LayerExpr::Extents { a: 0 }).unwrap();
        assert_eq!(ext.result, region(&[(0, 0, 3000, 3000), (3600, 2000, 4400, 2600)]));

        for (min, max) in [(Some(100_000), Some(400_000)), (Some(0), Some(50_000)), (Some(2_000_000), None), (None, None)] {
            let p = derived_preview(&LayerExpr::AreaRange { a: 0, min, max }).unwrap();
            let want = if min.unwrap_or(0) == 0 && max.is_none() { 3 } else { 1 };
            assert_eq!(p.result.parts().len(), want, "{min:?} {max:?}");
            assert!(p.inputs[0].1.parts().len() >= 2 || want == 3);
        }

        let u = derived_preview(&LayerExpr::Union(vec![1, 4, 1])).unwrap();
        assert_eq!(u.inputs.len(), 2);
        assert_eq!(u.result.parts().len(), 1);
        assert!(derived_preview(&LayerExpr::Check(Box::new(Rule::Flag { layer: 0 }))).is_none());
    }
}
