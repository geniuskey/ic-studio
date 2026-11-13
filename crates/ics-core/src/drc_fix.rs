//! Quick fixes for DRC violations: candidate edits of the top cell, each checked with live DRC.
//!
//! Only top-level shapes are reshaped and only top-level shapes and instances are moved; new
//! coordinates are snapped to [`Tech::grid`] (outward when growing). A candidate is kept when,
//! after it is applied, [`drc::run_window`] around the edit reports no violation of the same
//! rule touching the original marker. [`Fix::new_violations`] compares the violation counts per
//! rule in that window before (without the fixed violation) and after the edit (the sum of the
//! increases), so a moved shape that carries an unrelated violation along is not counted. Fixes are sorted by
//! `new_violations`, then by the changed area (added, removed or swept), then by title.

use std::collections::BTreeMap;

use crate::drc::{self, Deck, Rule, Violation};
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten, flatten_in};
use crate::layout::{Geometry, Layout};
use crate::region::Region;
use crate::tech::Tech;

const MAX_DEPTH: u32 = 64;

/// One edit of the top cell.
#[derive(Debug, Clone, PartialEq)]
pub enum FixEdit {
    /// Replace the geometry of a top-level shape.
    Reshape { id: u64, geom: Geometry },
    /// Move a top-level shape or instance.
    Move { id: u64, dx: i64, dy: i64 },
    /// Add a shape.
    Add { layer: usize, geom: Geometry },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    /// E.g. "Widen metal1 to 0.23 µm".
    pub title: String,
    pub edits: Vec<FixEdit>,
    /// Violations near the edit after it is applied that were not there before.
    pub new_violations: usize,
}

/// Carry out `fix` on `l`.
pub fn apply(l: &mut Layout, fix: &Fix) {
    for e in &fix.edits {
        match e {
            FixEdit::Reshape { id, geom } => {
                if let Some(s) = l.shape_mut(*id) {
                    s.geom = geom.clone();
                }
            }
            FixEdit::Move { id, dx, dy } => l.translate_item(*id, *dx, *dy),
            FixEdit::Add { layer, geom } => {
                l.add(*layer, geom.clone());
            }
        }
    }
}

/// Apply the best clean fix (one that adds no violation) of each violation in turn, re-checking
/// after every fix, until none is left that can be fixed or `max_fixes` were applied. Returns the
/// titles of the fixes applied, in order.
pub fn fix_all(deck: &Deck, top: &mut Layout, src: &dyn LayoutSource, tech: &Tech, max_fixes: usize) -> Vec<String> {
    let mut done = vec![];
    let mut skip: Vec<(String, IRect)> = vec![];
    while done.len() < max_fixes {
        let Some(area) = flatten(top, src, 64).iter().map(|f| f.geom.bbox()).reduce(|a, b| a.union(&b)) else { break };
        let rep = drc::run_window(deck, top, src, tech, area);
        let mut next = None;
        for v in &rep.violations {
            let key = (v.rule.clone(), v.marker);
            if skip.contains(&key) {
                continue;
            }
            match quick_fixes(deck, v, top, src, tech).into_iter().find(|f| f.new_violations == 0) {
                Some(f) => {
                    next = Some(f);
                    break;
                }
                None => skip.push(key),
            }
        }
        let Some(fix) = next else { break };
        apply(top, &fix);
        done.push(fix.title);
    }
    done
}

/// Fixes for `v` that remove it, best first.
pub fn quick_fixes(deck: &Deck, v: &Violation, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Vec<Fix> {
    let Some(nr) = deck.rules.iter().find(|r| r.name == v.rule) else { return vec![] };
    if nr.rule.is_global() || nr.rule.layers().iter().any(|&l| l >= tech.layers.len()) {
        return vec![];
    }
    let cx = Ctx { top, src, tech, m: v.marker, g: tech.grid.max(1) };
    let cands = match nr.rule {
        Rule::Width { layer, min } => cx.width(layer, min),
        Rule::Space { layer, min } => cx.space(layer, min),
        Rule::Separation { a, b, min } => cx.separation(a, b, min),
        Rule::Enclosure { inner, outer, min } => cx.enclosure(inner, outer, min),
        Rule::Extension { a, b, min } => cx.extension(a, b, min),
        Rule::Area { layer, min } => cx.area(layer, min),
        _ => vec![],
    };
    let reach = deck.reach(tech);
    let mut fixes: Vec<(usize, i128, Fix)> = Vec::new();
    for c in cands {
        if fixes.iter().any(|(_, _, f)| f.edits == c.edits) {
            continue;
        }
        let w = grow(c.extent.union(&v.marker), reach);
        let mut after = top.clone();
        let mut fix = Fix { title: c.title, edits: c.edits, new_violations: 0 };
        apply(&mut after, &fix);
        let rep = drc::run_window(deck, &after, src, tech, w);
        if rep.violations.iter().any(|x| x.rule == v.rule && touch(&x.marker, &v.marker)) {
            continue;
        }
        let before = drc::run_window(deck, top, src, tech, w);
        let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
        for x in &rep.violations {
            *counts.entry(&x.rule).or_default() += 1;
        }
        for x in before.violations.iter().filter(|x| !(x.rule == v.rule && touch(&x.marker, &v.marker))) {
            *counts.entry(&x.rule).or_default() -= 1;
        }
        fix.new_violations = counts.values().map(|&n| n.max(0) as usize).sum();
        fixes.push((fix.new_violations, c.cost, fix));
    }
    fixes.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| a.2.title.cmp(&b.2.title)));
    fixes.into_iter().map(|(_, _, f)| f).collect()
}

struct Cand {
    title: String,
    edits: Vec<FixEdit>,
    /// Geometry touched by the edit, before and after.
    extent: IRect,
    /// Changed area (dbu²).
    cost: i128,
}

/// A top-level shape or instance with its geometry on one layer near the marker.
struct Item {
    id: u64,
    /// The top-level rectangle, when the item is a `Geometry::Rect` shape.
    rect: Option<IRect>,
    instance: bool,
    rects: Vec<IRect>,
    /// Full extent of the item.
    bbox: IRect,
}

struct Ctx<'a> {
    top: &'a Layout,
    src: &'a dyn LayoutSource,
    tech: &'a Tech,
    m: IRect,
    g: i64,
}

impl Ctx<'_> {
    fn name(&self, l: usize) -> &str {
        &self.tech.layers[l].name
    }

    fn um(&self, v: i64) -> String {
        format!("{}", v as f64 / self.tech.dbu_per_um.max(1) as f64)
    }

    fn down(&self, v: i64) -> i64 {
        v.div_euclid(self.g) * self.g
    }

    fn up(&self, v: i64) -> i64 {
        -self.down(-v)
    }

    fn snap_out(&self, r: IRect) -> IRect {
        IRect::new(self.down(r.x0), self.down(r.y0), self.up(r.x1), self.up(r.y1))
    }

    fn items(&self, layer: usize, area: IRect) -> Vec<Item> {
        let mut out = Vec::new();
        for s in self.top.shapes.iter().filter(|s| s.layer == layer) {
            let rects: Vec<IRect> = rects_of(&s.geom).into_iter().filter(|r| touch(r, &area)).collect();
            if !rects.is_empty() {
                let rect = match s.geom {
                    Geometry::Rect(r) => Some(r),
                    _ => None,
                };
                out.push(Item { id: s.id, rect, instance: false, rects, bbox: s.bbox() });
            }
        }
        for inst in &self.top.instances {
            let one = Layout { instances: vec![inst.clone()], ..Default::default() };
            let rects: Vec<IRect> = flatten_in(&one, self.src, MAX_DEPTH, area)
                .iter()
                .filter(|f| f.layer == layer)
                .flat_map(|f| rects_of(&f.geom))
                .filter(|r| touch(r, &area))
                .collect();
            if rects.is_empty() {
                continue;
            }
            let bbox =
                flatten(&one, self.src, MAX_DEPTH).iter().map(|f| f.geom.bbox()).reduce(|a, b| a.union(&b)).unwrap_or(rects[0]);
            out.push(Item { id: inst.id, rect: None, instance: true, rects, bbox });
        }
        out
    }

    fn reshape(&self, title: String, id: u64, old: IRect, new: IRect) -> Cand {
        Cand {
            title,
            edits: vec![FixEdit::Reshape { id, geom: Geometry::Rect(new) }],
            extent: old.union(&new),
            cost: area(&new) + area(&old) - 2 * area(&clip(&old, &new)),
        }
    }

    fn moved(&self, layer: usize, it: &Item, (dx, dy): (i64, i64)) -> Cand {
        let dir = match (dx.signum(), dy.signum()) {
            (1, _) => "right",
            (-1, _) => "left",
            (_, 1) => "up",
            _ => "down",
        };
        let what = if it.instance { "instance".to_string() } else { format!("{} shape", self.name(layer)) };
        let swept = dx.abs() as i128 * it.bbox.height() as i128 + dy.abs() as i128 * it.bbox.width() as i128;
        Cand {
            title: format!("Move {what} {dir} by {} µm", self.um(dx.abs().max(dy.abs()))),
            edits: vec![FixEdit::Move { id: it.id, dx, dy }],
            extent: it.bbox.union(&it.bbox.translate(dx, dy)),
            cost: 2 * swept.min(area(&it.bbox)),
        }
    }

    fn width(&self, layer: usize, min: i64) -> Vec<Cand> {
        let mut out = Vec::new();
        let target = self.up(min);
        for s in self.top.shapes.iter().filter(|s| s.layer == layer) {
            match &s.geom {
                Geometry::Rect(r) if r.intersects(&self.m) || touch(r, &self.m) => {
                    let widen = |lo: i64, hi: i64| {
                        let extra = target - (hi - lo);
                        let a = self.down(extra / 2);
                        (self.down(lo - a), self.up(hi + extra - a))
                    };
                    let mut n = *r;
                    if r.width() < min {
                        (n.x0, n.x1) = widen(r.x0, r.x1);
                    }
                    if r.height() < min {
                        (n.y0, n.y1) = widen(r.y0, r.y1);
                    }
                    if n != *r {
                        let w = if r.width() < min { n.width() } else { n.height() };
                        out.push(self.reshape(format!("Widen {} to {} µm", self.name(layer), self.um(w)), s.id, *r, n));
                    }
                }
                Geometry::Path { points, width } if *width < min && s.rects().iter().any(|r| touch(r, &self.m)) => {
                    let w = -(-min).div_euclid(2 * self.g) * 2 * self.g;
                    let geom = Geometry::Path { points: points.clone(), width: w };
                    let (old, new) = (s.bbox(), geom.bbox());
                    out.push(Cand {
                        title: format!("Widen {} to {} µm", self.name(layer), self.um(w)),
                        edits: vec![FixEdit::Reshape { id: s.id, geom }],
                        extent: old.union(&new),
                        cost: area(&new) - area(&old),
                    });
                }
                _ => {}
            }
        }
        out
    }

    fn space(&self, layer: usize, min: i64) -> Vec<Cand> {
        let items = self.items(layer, grow(self.m, 2 * min + self.g));
        let all: Vec<(usize, IRect)> =
            items.iter().enumerate().flat_map(|(i, it)| it.rects.iter().map(move |&r| (i, r))).collect();
        let comp = components(&all.iter().map(|&(_, r)| r).collect::<Vec<_>>());
        let adj: Vec<usize> = (0..all.len()).filter(|&k| touch(&all[k].1, &self.m)).collect();
        let mut out = Vec::new();
        if !adj.is_empty() && adj.iter().all(|&k| comp[k] == comp[adj[0]]) {
            let r = self.snap_out(self.m);
            out.push(Cand {
                title: format!("Fill notch on {}", self.name(layer)),
                edits: vec![FixEdit::Add { layer, geom: Geometry::Rect(r) }],
                extent: r,
                cost: area(&r),
            });
        }
        let min_w = self.tech.layers[layer].min_width.max(1);
        for (i, it) in items.iter().enumerate() {
            let mine: Vec<usize> = adj.iter().copied().filter(|&k| all[k].0 == i).collect();
            let others: Vec<usize> = adj.iter().copied().filter(|&k| all[k].0 != i).collect();
            let mut moves = Vec::new();
            for &a in &mine {
                for &b in &others {
                    let (r, o) = (all[a].1, all[b].1);
                    if comp[a] != comp[b] {
                        moves.extend(self.pushes(r, o, min));
                    }
                    if let Some(rect) = it.rect
                        && let Some(n) = self.shrink(rect, o, min).filter(|n| n.width().min(n.height()) >= min_w)
                    {
                        let d = (rect.width() - n.width()).max(rect.height() - n.height());
                        let t = format!("Shrink {} shape by {} µm", self.name(layer), self.um(d));
                        out.push(self.reshape(t, it.id, rect, n));
                    }
                }
            }
            out.extend(combine(moves).into_iter().map(|d| self.moved(layer, it, d)));
        }
        out
    }

    fn separation(&self, a: usize, b: usize, min: i64) -> Vec<Cand> {
        let area_ = grow(self.m, 2 * min + self.g);
        let near = grow(self.m, min);
        let others: Vec<IRect> = self.items(a, area_).into_iter().flat_map(|it| it.rects).filter(|r| touch(r, &near)).collect();
        let mut out = Vec::new();
        for it in self.items(b, area_) {
            let mut moves = Vec::new();
            for r in it.rects.iter().filter(|r| r.intersects(&self.m) || touch(r, &self.m)) {
                for o in others.iter().filter(|o| !o.intersects(r)) {
                    moves.extend(self.pushes(*r, *o, min));
                }
            }
            out.extend(combine(moves).into_iter().map(|d| self.moved(b, &it, d)));
        }
        out
    }

    fn enclosure(&self, inner: usize, outer: usize, min: i64) -> Vec<Cand> {
        let ins: Vec<IRect> = self
            .items(inner, grow(self.m, self.g))
            .into_iter()
            .filter(|it| it.rects.iter().any(|r| r.intersects(&self.m)))
            .flat_map(|it| it.rects)
            .collect();
        let Some(ib) = ins.iter().copied().reduce(|x, y| x.union(&y)) else { return vec![] };
        let need = self.snap_out(grow(ib, min));
        let by = self.um(min);
        let (ni, no) = (self.name(inner), self.name(outer));
        let mut out = Vec::new();
        for s in self.top.shapes.iter().filter(|s| s.layer == outer) {
            if let Geometry::Rect(r) = s.geom
                && r.intersects(&ib)
            {
                let n = r.union(&need);
                out.push(self.reshape(format!("Grow {no} to enclose {ni} by {by} µm"), s.id, r, n));
            }
        }
        if out.is_empty() {
            out.push(Cand {
                title: format!("Add {no} enclosing {ni} by {by} µm"),
                edits: vec![FixEdit::Add { layer: outer, geom: Geometry::Rect(need) }],
                extent: need,
                cost: area(&need),
            });
        }
        out
    }

    fn extension(&self, a: usize, b: usize, min: i64) -> Vec<Cand> {
        let band = self.snap_out(self.m);
        let mut out = Vec::new();
        for s in self.top.shapes.iter().filter(|s| s.layer == a) {
            let Geometry::Rect(r) = s.geom else { continue };
            let covers = (r.x0 <= band.x0 && band.x1 <= r.x1) || (r.y0 <= band.y0 && band.y1 <= r.y1);
            if touch(&r, &band) && covers {
                let n = r.union(&band);
                if n != r {
                    let t = format!("Extend {} past {} by {} µm", self.name(a), self.name(b), self.um(min));
                    out.push(self.reshape(t, s.id, r, n));
                }
            }
        }
        out
    }

    fn area(&self, layer: usize, min: i128) -> Vec<Cand> {
        let items = self.items(layer, self.m);
        let piece: Vec<IRect> =
            items.iter().flat_map(|it| it.rects.iter()).filter(|r| r.intersects(&self.m)).map(|r| clip(r, &self.m)).collect();
        let deficit = min - Region::from_rects(piece).area();
        if deficit <= 0 {
            return vec![];
        }
        let d = self.tech.dbu_per_um.max(1) as f64;
        let title = format!("Grow {} to {} µm²", self.name(layer), min as f64 / (d * d));
        let mut out = Vec::new();
        for it in &items {
            let Some(r) = it.rect.filter(|r| r.intersects(&self.m)) else { continue };
            let grow_axis = |lo: i64, hi: i64, side: i64| {
                let e = ((deficit + side as i128 - 1) / side.max(1) as i128) as i64;
                (self.down(lo - (e + 1) / 2), self.up(hi + e / 2))
            };
            let mut along_x = r;
            (along_x.x0, along_x.x1) = grow_axis(r.x0, r.x1, r.height());
            let mut along_y = r;
            (along_y.y0, along_y.y1) = grow_axis(r.y0, r.y1, r.width());
            let (first, second) = if r.width() >= r.height() { (along_x, along_y) } else { (along_y, along_x) };
            out.push(self.reshape(title.clone(), it.id, r, first));
            out.push(self.reshape(title.clone(), it.id, r, second));
        }
        out
    }

    /// Moves of `r` that put it `min` away from `o` (on grid).
    fn pushes(&self, r: IRect, o: IRect, min: i64) -> Vec<(i64, i64)> {
        let xo = r.x1.min(o.x1) - r.x0.max(o.x0);
        let yo = r.y1.min(o.y1) - r.y0.max(o.y0);
        let sx = if r.x0 + r.x1 < o.x0 + o.x1 { -1 } else { 1 };
        let sy = if r.y0 + r.y1 < o.y0 + o.y1 { -1 } else { 1 };
        let (gx, gy) = (-xo, -yo);
        let mut out = Vec::new();
        let x = |d: i64| (sx * self.up(d), 0);
        let y = |d: i64| (0, sy * self.up(d));
        if xo > 0 && yo > 0 {
            return out;
        }
        if yo > 0 {
            out.extend((min > gx).then(|| x(min - gx)));
        } else if xo > 0 {
            out.extend((min > gy).then(|| y(min - gy)));
        } else {
            // Corner to corner: clear along one axis, or just enough for the Euclidean distance.
            let euclid = |g: i64| ((min * min - g * g).max(0) as f64).sqrt().ceil() as i64;
            out.extend((min > gx).then(|| x(min - gx)));
            out.extend((min > gy).then(|| y(min - gy)));
            out.extend((euclid(gy) > gx).then(|| x(euclid(gy) - gx)));
            out.extend((euclid(gx) > gy).then(|| y(euclid(gx) - gy)));
        }
        out
    }

    /// `r` with its edge facing `o` pulled back so the gap becomes `min`.
    fn shrink(&self, r: IRect, o: IRect, min: i64) -> Option<IRect> {
        let xo = r.x1.min(o.x1) - r.x0.max(o.x0);
        let yo = r.y1.min(o.y1) - r.y0.max(o.y0);
        let mut n = r;
        if yo > 0 && xo <= 0 && min > -xo {
            if r.x1 <= o.x0 {
                n.x1 = self.down(o.x0 - min);
            } else {
                n.x0 = self.up(o.x1 + min);
            }
        } else if xo > 0 && yo <= 0 && min > -yo {
            if r.y1 <= o.y0 {
                n.y1 = self.down(o.y0 - min);
            } else {
                n.y0 = self.up(o.y1 + min);
            }
        } else {
            return None;
        }
        (n.x0 < n.x1 && n.y0 < n.y1).then_some(n)
    }
}

/// The distinct moves, plus per direction the largest one (which satisfies every neighbour).
fn combine(moves: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    let mut best: BTreeMap<(bool, i64), i64> = BTreeMap::new();
    for (dx, dy) in moves {
        if !out.contains(&(dx, dy)) {
            out.push((dx, dy));
        }
        let (key, mag) = if dx != 0 { ((true, dx.signum()), dx.abs()) } else { ((false, dy.signum()), dy.abs()) };
        let e = best.entry(key).or_default();
        *e = (*e).max(mag);
    }
    for ((x, s), m) in best {
        let d = if x { (s * m, 0) } else { (0, s * m) };
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Edge-connected components (rects sharing a boundary of positive length or overlapping).
fn components(rects: &[IRect]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..rects.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..rects.len() {
        for j in i + 1..rects.len() {
            let (a, b) = (rects[i], rects[j]);
            let xo = a.x1.min(b.x1) - a.x0.max(b.x0);
            let yo = a.y1.min(b.y1) - a.y0.max(b.y0);
            if xo >= 0 && yo >= 0 && (xo > 0 || yo > 0) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    (0..rects.len()).map(|i| find(&mut parent, i)).collect()
}

fn rects_of(g: &Geometry) -> Vec<IRect> {
    match g {
        Geometry::Rect(r) => vec![*r],
        Geometry::Label { .. } => vec![],
        g => Region::from_geometry(g).map(|r| r.rects().to_vec()).unwrap_or_default(),
    }
}

fn touch(a: &IRect, b: &IRect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

fn grow(r: IRect, d: i64) -> IRect {
    IRect::new(r.x0 - d, r.y0 - d, r.x1 + d, r.y1 + d)
}

fn clip(a: &IRect, b: &IRect) -> IRect {
    let r = IRect { x0: a.x0.max(b.x0), y0: a.y0.max(b.y0), x1: a.x1.min(b.x1), y1: a.y1.min(b.y1) };
    if r.x0 < r.x1 && r.y0 < r.y1 { r } else { IRect::default() }
}

fn area(r: &IRect) -> i128 {
    r.width() as i128 * r.height() as i128
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::drc::parse_deck;
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

    fn on_grid(f: &Fix) -> bool {
        let g = |v: i64| v % 10 == 0;
        let geom = |geom: &Geometry| match geom {
            Geometry::Rect(r) => [r.x0, r.y0, r.x1, r.y1].into_iter().all(g),
            Geometry::Path { points, width } => width % 20 == 0 && points.iter().all(|p| g(p.0) && g(p.1)),
            Geometry::Polygon { points } => points.iter().all(|p| g(p.0) && g(p.1)),
            Geometry::Label { .. } => true,
        };
        f.edits.iter().all(|e| match e {
            FixEdit::Reshape { geom: gm, .. } | FixEdit::Add { geom: gm, .. } => geom(gm),
            FixEdit::Move { dx, dy, .. } => g(*dx) && g(*dy),
        })
    }

    /// Fixes of the single violation of `deck` in `lay`; each is on grid and removes it.
    fn fixes_in(deck: &str, lay: &Layout, src: &dyn LayoutSource) -> Vec<Fix> {
        let t = tech();
        let d = parse_deck(deck, &t).unwrap();
        let rep = drc::run(&d, lay, src, &t);
        assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations);
        let fixes = quick_fixes(&d, &rep.violations[0], lay, src, &t);
        assert!(!fixes.is_empty(), "no fix for {deck}");
        for f in &fixes {
            assert!(on_grid(f), "{f:?}");
            let mut l = lay.clone();
            apply(&mut l, f);
            let after = drc::run(&d, &l, src, &t);
            assert!(
                !after.violations.iter().any(|x| x.marker == rep.violations[0].marker),
                "{} left {:?}",
                f.title,
                after.violations
            );
        }
        for w in fixes.windows(2) {
            assert!(w[0].new_violations <= w[1].new_violations);
        }
        fixes
    }

    fn clean_after(deck: &str, lay: &Layout, src: &dyn LayoutSource, f: &Fix) -> bool {
        let t = tech();
        let mut l = lay.clone();
        apply(&mut l, f);
        drc::run(&parse_deck(deck, &t).unwrap(), &l, src, &t).violations.is_empty()
    }

    #[test]
    fn width_rect_and_path() {
        let deck = "width metal1 0.23";
        let lay = layout(&[("metal1", (0, 0, 220, 2000))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert_eq!(fixes[0].title, "Widen metal1 to 0.23 µm");
        assert_eq!(fixes[0].new_violations, 0);
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));

        let mut path = Layout::default();
        path.add(li("metal1"), Geometry::Path { points: vec![(0, 0), (2000, 0)], width: 200 });
        let fixes = fixes_in(deck, &path, &NoCells);
        assert_eq!(fixes[0].title, "Widen metal1 to 0.24 µm");
        assert!(clean_after(deck, &path, &NoCells, &fixes[0]));

        let neck =
            layout(&[("metal1", (0, 0, 1000, 1000)), ("metal1", (1000, 400, 2000, 600)), ("metal1", (2000, 0, 3000, 1000))]);
        let fixes = fixes_in(deck, &neck, &NoCells);
        assert!(clean_after(deck, &neck, &NoCells, &fixes[0]));
    }

    #[test]
    fn space_pair() {
        let deck = "space metal1 0.23";
        let lay = layout(&[("metal1", (0, 0, 500, 1000)), ("metal1", (700, 200, 1500, 800))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert!(fixes.iter().all(|f| !f.title.starts_with("Fill")), "separate pieces must not be joined");
        assert!(fixes.iter().any(|f| f.title == "Move metal1 shape right by 0.03 µm"));
        assert!(fixes.iter().any(|f| f.title == "Move metal1 shape left by 0.03 µm"));
        assert!(fixes.iter().any(|f| f.title.starts_with("Shrink")));
        assert!(fixes.iter().all(|f| clean_after(deck, &lay, &NoCells, f)));
    }

    #[test]
    fn space_diagonal() {
        let deck = "space metal1 0.23";
        let lay = layout(&[("metal1", (0, 0, 500, 500)), ("metal1", (600, 650, 1500, 1500))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));
        let touching = layout(&[("metal1", (0, 0, 500, 500)), ("metal1", (500, 500, 1500, 1500))]);
        let fixes = fixes_in(deck, &touching, &NoCells);
        assert!(clean_after(deck, &touching, &NoCells, &fixes[0]));
    }

    #[test]
    fn fill_notch_of_u() {
        let deck = "space metal1 0.23";
        let mut u = Layout::default();
        let points = vec![(0, 0), (1200, 0), (1200, 1500), (700, 1500), (700, 300), (500, 300), (500, 1500), (0, 1500)];
        u.add(li("metal1"), Geometry::Polygon { points });
        let fixes = fixes_in(deck, &u, &NoCells);
        let fill = fixes.iter().find(|f| f.title == "Fill notch on metal1").expect("fill");
        assert_eq!(fill.edits, [FixEdit::Add { layer: li("metal1"), geom: Geometry::Rect(IRect::new(500, 300, 700, 1500)) }]);
        assert!(clean_after(deck, &u, &NoCells, fill));

        // The same U built from three touching rectangles: fill, or shrink an arm.
        let rects = layout(&[("metal1", (0, 0, 1200, 300)), ("metal1", (0, 0, 500, 1500)), ("metal1", (700, 0, 1200, 1500))]);
        let fixes = fixes_in(deck, &rects, &NoCells);
        assert!(fixes.iter().any(|f| f.title == "Fill notch on metal1"));
        assert!(fixes.iter().any(|f| f.title == "Shrink metal1 shape by 0.03 µm"));
        assert!(fixes.iter().all(|f| !f.title.starts_with("Move")), "arms of one piece are not moved apart");
    }

    #[test]
    fn move_into_third_shape_ranks_last() {
        let deck = "space metal1 0.23";
        let lay =
            layout(&[("metal1", (-2000, 0, 500, 1000)), ("metal1", (700, 0, 1200, 1000)), ("metal1", (1440, 0, 2000, 1000))]);
        let t = tech();
        let d = parse_deck(deck, &t).unwrap();
        let v = drc::run(&d, &lay, &NoCells, &t).violations;
        assert_eq!(v.len(), 1);
        let fixes = quick_fixes(&d, &v[0], &lay, &NoCells, &t);
        let bad = fixes.iter().position(|f| f.title == "Move metal1 shape right by 0.03 µm").expect("move right");
        assert!(fixes[bad].new_violations >= 1);
        let good = fixes.iter().position(|f| f.title == "Move metal1 shape left by 0.03 µm").expect("move left");
        assert_eq!(fixes[good].new_violations, 0);
        assert!(good < bad);
        assert_eq!(fixes[0].new_violations, 0);
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));
    }

    #[test]
    fn space_moves_instance() {
        let deck = "space metal1 0.23";
        let cell = CellRef::new("lib", "pad");
        let mut cells = HashMap::new();
        cells.insert(cell.clone(), layout(&[("metal1", (0, 0, 500, 1000))]));
        let mut lay = layout(&[("metal1", (-500, 0, 0, 1000))]);
        lay.add_instance(cell, Trans::new(200, 0), None);
        let fixes = fixes_in(deck, &lay, &cells);
        let f = fixes.iter().find(|f| f.title == "Move instance right by 0.03 µm").expect("instance move");
        assert!(clean_after(deck, &lay, &cells, f));
    }

    #[test]
    fn separation() {
        let deck = "separation poly contact 0.16";
        let lay = layout(&[("poly", (0, 0, 180, 2000)), ("contact", (280, 500, 500, 720))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert_eq!(fixes[0].title, "Move contact shape right by 0.06 µm");
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));
    }

    #[test]
    fn enclosure() {
        let deck = "enclosure contact active 0.1";
        let lay = layout(&[("active", (0, 0, 1000, 1000)), ("contact", (50, 400, 270, 620))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert_eq!(fixes[0].title, "Grow active to enclose contact by 0.1 µm");
        assert_eq!(fixes[0].edits, [FixEdit::Reshape { id: 1, geom: Geometry::Rect(IRect::new(-50, 0, 1000, 1000)) }]);
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));

        // Outer geometry inside an instance: add an outer rectangle instead.
        let cell = CellRef::new("lib", "diff");
        let mut cells = HashMap::new();
        cells.insert(cell.clone(), layout(&[("active", (0, 0, 1000, 1000))]));
        let mut lay = layout(&[("contact", (50, 400, 270, 620))]);
        lay.add_instance(cell, Trans::IDENTITY, None);
        let fixes = fixes_in(deck, &lay, &cells);
        assert_eq!(fixes[0].title, "Add active enclosing contact by 0.1 µm");
        assert!(clean_after(deck, &lay, &cells, &fixes[0]));
    }

    #[test]
    fn extension() {
        let deck = "extension poly active 0.22";
        let lay = layout(&[("active", (0, 0, 1500, 1000)), ("poly", (600, -100, 780, 1300))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert_eq!(fixes[0].title, "Extend poly past active by 0.22 µm");
        assert_eq!(fixes[0].edits, [FixEdit::Reshape { id: 2, geom: Geometry::Rect(IRect::new(600, -220, 780, 1300)) }]);
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));
    }

    #[test]
    fn area() {
        let deck = "area metal1 0.1";
        let lay = layout(&[("metal1", (0, 0, 230, 300))]);
        let fixes = fixes_in(deck, &lay, &NoCells);
        assert_eq!(fixes[0].title, "Grow metal1 to 0.1 µm²");
        let FixEdit::Reshape { geom: Geometry::Rect(r), .. } = fixes[0].edits[0] else { panic!("{:?}", fixes[0]) };
        assert_eq!((r.width(), r.x0, r.x1), (230, 0, 230), "grows along the longer side");
        assert!(r.height() as i128 * 230 >= 100_000);
        assert!(clean_after(deck, &lay, &NoCells, &fixes[0]));
    }

    #[test]
    fn no_fixes_for_global_or_derived_rules() {
        let t = tech();
        let lay = layout(&[("metal1", (0, 0, 220, 2000)), ("poly", (0, 0, 100, 100)), ("active", (0, 0, 50, 50))]);
        for deck in ["flag metal1", "density metal1 50 80 10", "gate = poly and active\nwidth gate 0.3"] {
            let d = parse_deck(deck, &t).unwrap();
            for v in drc::run(&d, &lay, &NoCells, &t).violations {
                assert!(quick_fixes(&d, &v, &lay, &NoCells, &t).is_empty(), "{deck}");
            }
        }
    }
}
