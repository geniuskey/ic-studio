//! Layout compaction: slide the cell's top-level items toward one side until the design rules
//! stop them, keeping connected geometry together.
//!
//! One-dimensional constraint compaction. Items (top-level shapes and instances, flattened) are
//! grouped into rigid clusters: geometry joined on a layer (touching on the same layer, or
//! overlapping across layers) moves as one, and so do the members of a symmetry or align
//! constraint, so compaction never changes their relative placement. Clusters are then moved
//! one by one, nearest the target side first, as far as the spacing rules against every other
//! cluster allow (never past the starting extent, in steps of the grid). Every move is legal
//! against the layout as it stands at that moment, so each prefix of the move log is a valid
//! layout too: when a DRC run afterwards finds violations the spacing model missed, the latest
//! moves are undone until it is clean again.

use std::collections::{BTreeMap, BTreeSet};

use crate::constraints::{self, Axis, Constraint};
use crate::drc::{self, Deck, NamedRule, Rule};
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::tech::Tech;

const MAX_PASSES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Down,
    Up,
}

impl Dir {
    /// `r` in the frame where this direction points toward -x.
    fn canon(self, r: IRect) -> IRect {
        match self {
            Dir::Left => r,
            Dir::Right => IRect::new(-r.x1, r.y0, -r.x0, r.y1),
            Dir::Down => IRect::new(r.y0, r.x0, r.y1, r.x1),
            Dir::Up => IRect::new(-r.y1, r.x0, -r.y0, r.x1),
        }
    }

    /// The real translation of a move by `t` toward this side.
    fn delta(self, t: i64) -> (i64, i64) {
        match self {
            Dir::Left => (-t, 0),
            Dir::Right => (t, 0),
            Dir::Down => (0, -t),
            Dir::Up => (0, t),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompactOptions {
    /// Items to move (`None`: every top-level item). Others are obstacles.
    pub only: Option<BTreeSet<u64>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompactReport {
    /// Rigid groups that moved.
    pub moved: usize,
    /// Rigid groups found (connected geometry and constrained items form one group).
    pub groups: usize,
    /// Group moves undone because the DRC run found new violations.
    pub reverted: usize,
    /// Extent of the items taking part (`only`, or every item) before and after (dbu).
    pub before: Option<IRect>,
    pub after: Option<IRect>,
}

/// Required separation between layers, from the technology and the deck.
struct Rules {
    n: usize,
    sep: Vec<i64>,
    /// Layers no rule reads (text, boundary): they neither join nor block geometry.
    passive: Vec<bool>,
    max: i64,
}

impl Rules {
    fn new(tech: &Tech, deck: &Deck) -> Rules {
        let n = tech.layers.len();
        let mut sep = vec![0; n * n];
        let mut used = vec![false; n];
        for (i, l) in tech.layers.iter().enumerate() {
            sep[i * n + i] = l.min_spacing.max(0);
            used[i] = l.min_spacing > 0;
        }
        let mut stack: Vec<usize> = deck.rules.iter().flat_map(|r| r.rule.layers()).collect();
        let mut seen = BTreeSet::new();
        while let Some(k) = stack.pop() {
            if !seen.insert(k) {
                continue;
            }
            if k < n {
                used[k] = true;
            } else if let Some(d) = deck.derived.get(k - n) {
                stack.extend(d.expr.inputs());
            }
        }
        for r in &deck.rules {
            match r.rule {
                Rule::Space { layer, min } if layer < n => sep[layer * n + layer] = sep[layer * n + layer].max(min),
                Rule::Separation { a, b, min } if a < n && b < n => {
                    sep[a * n + b] = sep[a * n + b].max(min);
                    sep[b * n + a] = sep[b * n + a].max(min);
                }
                _ => {}
            }
        }
        let max = sep.iter().copied().max().unwrap_or(0);
        Rules { n, sep, passive: used.iter().map(|u| !u).collect(), max }
    }

    fn passive(&self, l: usize) -> bool {
        l < self.n && self.passive[l]
    }

    fn s(&self, a: usize, b: usize) -> i64 {
        if a < self.n && b < self.n { self.sep[a * self.n + b] } else { 0 }
    }
}

/// The technology's own deck, or its spacing rules when that does not parse.
fn tech_deck(tech: &Tech) -> Deck {
    drc::parse_any(&tech.deck_text(), tech).map(|p| p.deck).unwrap_or_else(|_| Deck {
        derived: vec![],
        rules: tech
            .layers
            .iter()
            .enumerate()
            .filter(|(_, d)| d.min_spacing > 0)
            .map(|(i, d)| NamedRule {
                name: format!("{}.S", d.name),
                rule: Rule::Space { layer: i, min: d.min_spacing },
                comment: None,
            })
            .collect(),
    })
}

/// Manhattan pieces of a geometry; non-Manhattan geometry by its bounding box.
fn pieces(g: &Geometry) -> Vec<IRect> {
    match g {
        Geometry::Rect(r) => vec![*r],
        Geometry::Label { .. } => vec![],
        Geometry::Path { points, .. } => {
            let r = g.rects();
            if !r.is_empty() && r.len() + 1 == points.len() { r } else { vec![g.bbox()] }
        }
        Geometry::Polygon { .. } => vec![g.bbox()],
    }
}

struct Item {
    id: u64,
    /// (layer, rect) in top-cell coordinates.
    rects: Vec<(usize, IRect)>,
}

struct Cluster {
    items: Vec<usize>,
    labels: Vec<u64>,
    /// Non-passive geometry in the canonical frame.
    rects: Vec<(usize, IRect)>,
    bbox: IRect,
    movable: bool,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// A label: its id and anchor.
type Label = (u64, (i64, i64));

fn items_of(l: &Layout, src: &dyn LayoutSource) -> (Vec<Item>, Vec<Label>) {
    let mut items = vec![];
    let mut labels = vec![];
    for s in &l.shapes {
        match &s.geom {
            Geometry::Label { at, .. } => labels.push((s.id, *at)),
            g => items.push(Item { id: s.id, rects: pieces(g).into_iter().map(|r| (s.layer, r)).collect() }),
        }
    }
    for inst in &l.instances {
        let one = Layout { instances: vec![inst.clone()], ..Default::default() };
        let rects: Vec<(usize, IRect)> = flatten(&one, src, 64)
            .iter()
            .flat_map(|f| {
                let layer = f.layer;
                pieces(&f.geom).into_iter().map(move |r| (layer, r))
            })
            .collect();
        if !rects.is_empty() {
            items.push(Item { id: inst.id, rects });
        }
    }
    (items, labels)
}

/// How far cluster `j` can slide toward -x (canonical frame) from where it is now.
fn room(j: usize, cl: &[Cluster], shift: &[i64], rules: &Rules, bound: i64) -> i64 {
    let bj = cl[j].bbox.translate(-shift[j], 0);
    let mut limit = bj.x0 - bound;
    if limit <= 0 {
        return 0;
    }
    let m = rules.max;
    let swept = IRect::new(bj.x0 - limit - m, bj.y0 - m, bj.x1 + m, bj.y1 + m);
    for (i, ci) in cl.iter().enumerate() {
        if i == j || ci.rects.is_empty() || !ci.bbox.translate(-shift[i], 0).intersects(&swept) {
            continue;
        }
        for &(la, a) in &ci.rects {
            let a = a.translate(-shift[i], 0);
            if !a.intersects(&swept) {
                continue;
            }
            for &(lb, b) in &cl[j].rects {
                let b = b.translate(-shift[j], 0);
                let s = rules.s(la, lb);
                if a.y0 >= b.y1 + s || b.y0 >= a.y1 + s {
                    continue;
                }
                if a.x1 + s <= b.x0 {
                    limit = limit.min(b.x0 - s - a.x1);
                } else if a.x0 <= b.x0 {
                    // Already closer than the rule allows with `a` behind: moving would make it worse.
                    return 0;
                }
                if limit <= 0 {
                    return 0;
                }
            }
        }
    }
    limit
}

fn violation_counts(deck: &Deck, l: &Layout, src: &dyn LayoutSource, tech: &Tech) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for v in drc::run(deck, l, src, tech).violations {
        *out.entry(v.rule).or_insert(0) += 1;
    }
    out
}

/// Slide the movable items of `l` toward `dir` as far as the design rules allow. `deck` defaults
/// to the technology's deck; the technology's same-layer spacings always apply.
pub fn compact(
    l: &mut Layout,
    src: &dyn LayoutSource,
    tech: &Tech,
    deck: Option<&Deck>,
    dir: Dir,
    opts: &CompactOptions,
) -> CompactReport {
    let owned;
    let deck = match deck {
        Some(d) => d,
        None => {
            owned = tech_deck(tech);
            &owned
        }
    };
    let rules = Rules::new(tech, deck);
    let (items, labels) = items_of(l, src);
    let index: BTreeMap<u64, usize> = items.iter().enumerate().map(|(k, it)| (it.id, k)).collect();

    let mut dsu = Dsu((0..items.len()).collect());
    let mut all: Vec<(usize, usize, IRect)> = items
        .iter()
        .enumerate()
        .flat_map(|(k, it)| it.rects.iter().filter(|(layer, _)| !rules.passive(*layer)).map(move |&(layer, r)| (k, layer, r)))
        .collect();
    all.sort_by_key(|e| e.2.x0);
    for i in 0..all.len() {
        let (ki, li, a) = all[i];
        for &(kk, lk, b) in &all[i + 1..] {
            if b.x0 > a.x1 {
                break;
            }
            let joined = if li == lk { b.y0 <= a.y1 && a.y0 <= b.y1 } else { a.intersects(&b) };
            if joined && ki != kk {
                dsu.union(ki, kk);
            }
        }
    }
    for c in &l.constraints {
        if matches!(c, Constraint::Symmetry { .. } | Constraint::Align { .. }) {
            let members: Vec<usize> = c.items().iter().filter_map(|id| index.get(id).copied()).collect();
            for w in members.windows(2) {
                dsu.union(w[0], w[1]);
            }
        }
    }
    let fixed = constraints::fixed_items(&l.constraints);
    let pinned = |id: u64| fixed.contains(&id) || opts.only.as_ref().is_some_and(|o| !o.contains(&id));

    let mut root_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut clusters: Vec<Cluster> = vec![];
    let mut cluster_of = vec![0; items.len()];
    for (k, it) in items.iter().enumerate() {
        let r = dsu.find(k);
        let c = *root_of.entry(r).or_insert_with(|| {
            clusters.push(Cluster { items: vec![], labels: vec![], rects: vec![], bbox: IRect::default(), movable: true });
            clusters.len() - 1
        });
        cluster_of[k] = c;
        let cl = &mut clusters[c];
        cl.items.push(k);
        cl.movable &= !pinned(it.id);
        cl.rects.extend(it.rects.iter().filter(|(layer, _)| !rules.passive(*layer)).map(|&(layer, r)| (layer, dir.canon(r))));
    }
    for cl in &mut clusters {
        match cl.rects.iter().map(|(_, r)| *r).reduce(|a, b| a.union(&b)) {
            Some(b) => cl.bbox = b,
            None => cl.movable = false,
        }
    }
    for &(id, (x, y)) in &labels {
        if fixed.contains(&id) {
            continue;
        }
        let host = items.iter().enumerate().find(|(_, it)| it.rects.iter().any(|(_, r)| r.contains(x, y)));
        if let Some((k, _)) = host {
            clusters[cluster_of[k]].labels.push(id);
        }
    }

    let taking_part = |it: &Item| opts.only.as_ref().is_none_or(|o| o.contains(&it.id));
    let extent = |shift: &[i64]| {
        items
            .iter()
            .enumerate()
            .filter(|(_, it)| taking_part(it))
            .flat_map(|(k, it)| {
                let (dx, dy) = dir.delta(shift[cluster_of[k]]);
                it.rects.iter().map(move |(_, r)| r.translate(dx, dy))
            })
            .reduce(|a, b| a.union(&b))
    };
    let groups = clusters.iter().filter(|c| !c.rects.is_empty()).count();
    let zero = vec![0; clusters.len()];
    let before = extent(&zero);

    let bound = clusters.iter().filter(|c| !c.rects.is_empty()).map(|c| c.bbox.x0).min().unwrap_or(0);
    let grid = tech.grid.max(1);
    let mut shift = zero.clone();
    let mut log: Vec<(usize, i64)> = vec![];
    for _ in 0..MAX_PASSES {
        let mut order: Vec<usize> = (0..clusters.len()).filter(|&c| clusters[c].movable).collect();
        order.sort_by_key(|&c| (clusters[c].bbox.x0 - shift[c], c));
        let mut any = false;
        for j in order {
            let t = room(j, &clusters, &shift, &rules, bound).div_euclid(grid) * grid;
            if t > 0 {
                shift[j] += t;
                log.push((j, t));
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    if log.is_empty() {
        return CompactReport { moved: 0, groups, reverted: 0, before, after: before };
    }

    let orig = l.clone();
    let apply = |k: usize| -> (Layout, Vec<i64>) {
        let mut s = zero.clone();
        for &(c, t) in &log[..k] {
            s[c] += t;
        }
        let mut out = orig.clone();
        for (c, cl) in clusters.iter().enumerate() {
            if s[c] == 0 {
                continue;
            }
            let (dx, dy) = dir.delta(s[c]);
            for &it in &cl.items {
                out.translate_item(items[it].id, dx, dy);
            }
            for &id in &cl.labels {
                out.translate_item(id, dx, dy);
            }
        }
        // A symmetry group moves rigidly, so its axis moves with it.
        for c in &mut out.constraints {
            let lead = c.items().iter().find_map(|id| index.get(id).copied());
            if let (Constraint::Symmetry { axis, at, .. }, Some(k)) = (c, lead) {
                let (dx, dy) = dir.delta(s[cluster_of[k]]);
                *at += if *axis == Axis::Vertical { dx } else { dy };
            }
        }
        (out, s)
    };
    let base_drc = violation_counts(deck, &orig, src, tech);
    let base_err = constraints::errors(&orig, src);
    let clean = |cand: &Layout| {
        let counts = violation_counts(deck, cand, src, tech);
        let worse = counts.iter().any(|(rule, n)| *n > base_drc.get(rule).copied().unwrap_or(0));
        !worse && constraints::errors(cand, src).iter().zip(&base_err).all(|(a, b)| a <= b)
    };
    let (mut lo, mut hi) = (0, log.len());
    let mut best = apply(hi);
    if !clean(&best.0) {
        best = apply(0);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            let cand = apply(mid);
            if clean(&cand.0) {
                lo = mid;
                best = cand;
            } else {
                hi = mid;
            }
        }
        hi = lo;
    }
    let (out, s) = best;
    *l = out;
    CompactReport { moved: s.iter().filter(|&&t| t > 0).count(), groups, reverted: log.len() - hi, before, after: extent(&s) }
}

/// [`compact`] toward `dirs[0]`, then `dirs[1]` (e.g. `[Dir::Left, Dir::Down]`). The report
/// adds up the moves and spans the first run's extent before to the second run's after.
pub fn compact_xy(
    l: &mut Layout,
    src: &dyn LayoutSource,
    tech: &Tech,
    deck: Option<&Deck>,
    dirs: [Dir; 2],
    opts: &CompactOptions,
) -> CompactReport {
    let a = compact(l, src, tech, deck, dirs[0], opts);
    let b = compact(l, src, tech, deck, dirs[1], opts);
    CompactReport {
        moved: a.moved + b.moved,
        groups: a.groups.max(b.groups),
        reverted: a.reverted + b.reverted,
        before: a.before,
        after: b.after,
    }
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

    fn deck(text: &str) -> Deck {
        drc::parse_deck(text, &tech()).unwrap()
    }

    fn x0(l: &Layout, id: u64) -> i64 {
        l.shape(id).unwrap().bbox().x0
    }

    fn y0(l: &Layout, id: u64) -> i64 {
        l.shape(id).unwrap().bbox().y0
    }

    fn three() -> (Layout, [u64; 3]) {
        let mut l = Layout::default();
        let m = li("metal1");
        let ids = [l.add_rect(m, 0, 0, 500, 1000), l.add_rect(m, 2000, 0, 2500, 1000), l.add_rect(m, 5000, 0, 5500, 1000)];
        (l, ids)
    }

    fn run(l: &mut Layout, dir: Dir) -> CompactReport {
        compact(l, &NoCells, &tech(), None, dir, &CompactOptions::default())
    }

    #[test]
    fn separated_rects_compact_to_min_spacing() {
        let (mut l, [a, b, c]) = three();
        let rep = run(&mut l, Dir::Left);
        assert_eq!((x0(&l, a), x0(&l, b), x0(&l, c)), (0, 730, 1460));
        assert_eq!((rep.moved, rep.groups, rep.reverted), (2, 3, 0));
        assert_eq!(rep.before, Some(IRect::new(0, 0, 5500, 1000)));
        assert_eq!(rep.after, Some(IRect::new(0, 0, 1960, 1000)));
        assert!(drc::run_tech_deck(&l, &NoCells, &tech()).unwrap().violations.is_empty());
        let again = run(&mut l, Dir::Left);
        assert_eq!(again.moved, 0, "already compact");
    }

    #[test]
    fn other_directions() {
        let (mut l, [a, b, c]) = three();
        run(&mut l, Dir::Right);
        assert_eq!((x0(&l, a), x0(&l, b), x0(&l, c)), (3540, 4270, 5000));

        let mut l = Layout::default();
        let m = li("metal1");
        let ids = [l.add_rect(m, 0, 0, 1000, 500), l.add_rect(m, 0, 2000, 1000, 2500), l.add_rect(m, 0, 5000, 1000, 5500)];
        let mut down = l.clone();
        run(&mut down, Dir::Down);
        assert_eq!(ids.map(|i| y0(&down, i)), [0, 730, 1460]);
        run(&mut l, Dir::Up);
        assert_eq!(ids.map(|i| y0(&l, i)), [3540, 4270, 5000]);
        assert!(ids.iter().all(|&i| x0(&l, i) == 0));
    }

    #[test]
    fn connected_geometry_moves_together() {
        let mut l = Layout::default();
        let (m, p) = (li("metal1"), li("poly"));
        let a = l.add_rect(m, 0, 0, 500, 1000);
        let b = l.add_rect(m, 3000, 0, 3500, 1000);
        let c = l.add_rect(m, 3500, 200, 4500, 400);
        let d = l.add_rect(p, 3200, -500, 3380, 1500);
        let rep = run(&mut l, Dir::Left);
        assert_eq!((x0(&l, a), x0(&l, b), x0(&l, c), x0(&l, d)), (0, 730, 1230, 930));
        assert_eq!((rep.moved, rep.groups), (1, 2));
    }

    #[test]
    fn fixed_and_unselected_items_stay() {
        let mut l = Layout::default();
        let m = li("metal1");
        let a = l.add_rect(m, 0, 0, 500, 1000);
        let b = l.add_rect(m, 1500, 0, 2000, 1000);
        let f = l.add_rect(m, 3000, 0, 3500, 1000);
        let c = l.add_rect(m, 6000, 0, 6500, 1000);
        l.constraints.push(Constraint::Fixed { items: vec![f] });
        let orig = l.clone();
        run(&mut l, Dir::Left);
        assert_eq!([a, b, f, c].map(|i| x0(&l, i)), [0, 730, 3000, 3730]);

        let mut l = orig;
        let opts = CompactOptions { only: Some([c].into()) };
        let rep = compact(&mut l, &NoCells, &tech(), None, Dir::Left, &opts);
        assert_eq!([a, b, f, c].map(|i| x0(&l, i)), [0, 1500, 3000, 3730]);
        assert_eq!(rep.before, Some(IRect::new(6000, 0, 6500, 1000)));
        assert_eq!(rep.after, Some(IRect::new(3730, 0, 4230, 1000)));
    }

    #[test]
    fn labels_follow_their_geometry() {
        let mut l = Layout::default();
        let m = li("metal1");
        l.add_rect(m, 0, 0, 500, 1000);
        let b = l.add_rect(m, 2000, 0, 2500, 1000);
        let t = l.add(li("text"), Geometry::Label { text: "out".into(), at: (2100, 500), size: 100 });
        let free = l.add(li("text"), Geometry::Label { text: "note".into(), at: (9000, 500), size: 100 });
        run(&mut l, Dir::Left);
        assert_eq!(x0(&l, b), 730);
        assert!(matches!(l.shape(t).unwrap().geom, Geometry::Label { at: (830, 500), .. }));
        assert!(matches!(l.shape(free).unwrap().geom, Geometry::Label { at: (9000, 500), .. }));
    }

    #[test]
    fn instances_compact() {
        let cell = CellRef::new("lib", "unit");
        let mut child = Layout::default();
        child.add_rect(li("metal1"), 0, 0, 500, 1000);
        let src: HashMap<CellRef, Layout> = [(cell.clone(), child)].into();
        let mut l = Layout::default();
        let i1 = l.add_instance(cell.clone(), Trans::new(0, 0), None);
        let i2 = l.add_instance(cell, Trans::new(4000, 0), None);
        let r = l.add_rect(li("metal1"), 10_000, 0, 10_400, 1000);
        let rep = compact(&mut l, &src, &tech(), None, Dir::Left, &CompactOptions::default());
        assert_eq!(l.instance(i1).unwrap().trans.x, 0);
        assert_eq!(l.instance(i2).unwrap().trans.x, 730);
        assert_eq!(x0(&l, r), 1460);
        assert_eq!(rep.moved, 2);
    }

    #[test]
    fn separation_rule_between_layers() {
        let mut l = Layout::default();
        l.add_rect(li("poly"), 0, 0, 180, 2000);
        let act = l.add_rect(li("active"), 1000, 500, 1500, 1000);
        let d = deck("separation poly active 0.05");
        compact(&mut l, &NoCells, &tech(), Some(&d), Dir::Left, &CompactOptions::default());
        assert_eq!(x0(&l, act), 230);
        assert!(drc::run(&d, &l, &NoCells, &tech()).violations.is_empty());
    }

    #[test]
    fn symmetry_group_moves_as_a_whole() {
        let mut l = Layout::default();
        let m = li("metal1");
        l.add_rect(m, -5000, 0, -4500, 400);
        let a = l.add_rect(m, -1000, 0, -500, 400);
        let b = l.add_rect(m, 500, 0, 1000, 400);
        let s = l.add_rect(m, -100, 600, 100, 800);
        let c = constraints::symmetry_of(&l, &NoCells, &[a, b, s].into(), Axis::Vertical).unwrap();
        l.constraints.push(c);
        assert_eq!(constraints::errors(&l, &NoCells), [0]);
        let rep = run(&mut l, Dir::Left);
        assert_eq!(rep.moved, 1);
        assert_eq!(x0(&l, a), -4270);
        assert!(matches!(l.constraints[0], Constraint::Symmetry { at: -3270, .. }));
        assert_eq!(constraints::errors(&l, &NoCells), [0]);
        run(&mut l, Dir::Down);
        assert_eq!(constraints::errors(&l, &NoCells), [0]);
    }

    #[test]
    fn no_new_drc_violations() {
        let d = deck("space metal1 0.23\nspace poly 0.25\nseparation poly active 0.05");
        let mut l = Layout::default();
        let (m, p, a) = (li("metal1"), li("poly"), li("active"));
        l.add_rect(m, 0, 0, 400, 400);
        l.add_rect(m, 1000, 100, 1400, 600);
        l.add_rect(m, 2100, -300, 2500, 200);
        l.add_rect(p, 3000, 0, 3180, 1500);
        l.add_rect(a, 4000, 200, 4600, 800);
        l.add_rect(m, 5000, 700, 5300, 1200);
        l.add_rect(p, 5600, -200, 5780, 300);
        l.add_rect(m, 1500, 3000, 1900, 3300);
        l.add_rect(a, 3500, 2500, 4200, 2700);
        assert!(drc::run(&d, &l, &NoCells, &tech()).violations.is_empty());
        let rep = compact_xy(&mut l, &NoCells, &tech(), Some(&d), [Dir::Left, Dir::Down], &CompactOptions::default());
        assert!(rep.moved > 0);
        assert!(drc::run(&d, &l, &NoCells, &tech()).violations.is_empty());
        let area = |r: IRect| r.width() * r.height();
        assert!(area(rep.after.unwrap()) < area(rep.before.unwrap()));
    }

    #[test]
    fn rules_the_model_misses_revert_moves() {
        let mut l = Layout::default();
        let (p, a) = (li("poly"), li("active"));
        l.add_rect(a, 0, 0, 1000, 600);
        l.add_rect(p, 400, -300, 580, 900);
        let a2 = l.add_rect(a, 5000, 0, 6000, 600);
        l.add_rect(p, 5400, -300, 5580, 900);
        let d = deck("space active 0.28\nspace poly 0.25\ngate = poly and active\nspace gate 2");
        let orig = l.clone();
        let rep = compact(&mut l, &NoCells, &tech(), Some(&d), Dir::Left, &CompactOptions::default());
        assert_eq!((rep.moved, rep.reverted), (0, 1));
        assert_eq!(l, orig);
        assert_eq!(x0(&l, a2), 5000);
    }
}
