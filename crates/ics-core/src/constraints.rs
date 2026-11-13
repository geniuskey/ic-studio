//! Placement constraints kept with a layout: symmetry about an axis, shared edges or centre lines,
//! and fixed items. Editing one member of a constraint moves the others so the constraint keeps
//! holding ([`enforce`]); [`infer`] derives symmetry from the schematic's matched groups.
//!
//! Items are top-level shapes and instances, positioned by their bounding box. Only positions are
//! constrained: a mirrored partner is moved, not flipped.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::Layout;
use crate::project::Project;
use crate::schematic::Schematic;
use crate::sdl::{device_of, matched_groups};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// A vertical line `x = at`: pairs sit left and right of it.
    Vertical,
    /// A horizontal line `y = at`: pairs sit below and above it.
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Left,
    Right,
    Bottom,
    Top,
    CenterX,
    CenterY,
}

impl Edge {
    pub const ALL: [Edge; 6] = [Edge::Left, Edge::Right, Edge::Bottom, Edge::Top, Edge::CenterX, Edge::CenterY];

    pub fn name(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Right => "right",
            Edge::Bottom => "bottom",
            Edge::Top => "top",
            Edge::CenterX => "centre x",
            Edge::CenterY => "centre y",
        }
    }

    /// The coordinate of `b` this edge names (centres rounded down).
    pub fn of(self, b: IRect) -> i64 {
        match self {
            Edge::Left => b.x0,
            Edge::Right => b.x1,
            Edge::Bottom => b.y0,
            Edge::Top => b.y1,
            Edge::CenterX => (b.x0 + b.x1).div_euclid(2),
            Edge::CenterY => (b.y0 + b.y1).div_euclid(2),
        }
    }

    /// Whether the edge is an x coordinate (items move horizontally to meet it).
    pub fn horizontal(self) -> bool {
        matches!(self, Edge::Left | Edge::Right | Edge::CenterX)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    /// Pairs mirror each other about the axis; singles are centred on it.
    Symmetry {
        axis: Axis,
        /// Axis position (x for a vertical axis, y for a horizontal one), in dbu.
        at: i64,
        #[serde(default)]
        pairs: Vec<(u64, u64)>,
        #[serde(default)]
        singles: Vec<u64>,
    },
    /// The items share an edge or centre line.
    Align { edge: Edge, items: Vec<u64> },
    /// The items stay where they are (enforcement and compaction do not move them).
    Fixed { items: Vec<u64> },
}

impl Constraint {
    pub fn items(&self) -> Vec<u64> {
        match self {
            Constraint::Symmetry { pairs, singles, .. } => {
                pairs.iter().flat_map(|&(a, b)| [a, b]).chain(singles.iter().copied()).collect()
            }
            Constraint::Align { items, .. } | Constraint::Fixed { items } => items.clone(),
        }
    }

    /// E.g. "symmetry about x = 1.2 µm (2 pairs, 1 centred)".
    pub fn describe(&self, dbu_per_um: i64) -> String {
        let um = |v: i64| v as f64 / dbu_per_um.max(1) as f64;
        match self {
            Constraint::Symmetry { axis, at, pairs, singles } => {
                let a = if *axis == Axis::Vertical { "x" } else { "y" };
                format!("Symmetry about {a} = {} µm ({} pair(s), {} centred)", um(*at), pairs.len(), singles.len())
            }
            Constraint::Align { edge, items } => format!("Align {} of {} item(s)", edge.name(), items.len()),
            Constraint::Fixed { items } => format!("Fixed: {} item(s)", items.len()),
        }
    }
}

/// Remove item `id` from every constraint; constraints left with nothing to relate are dropped.
pub fn forget(cs: &mut Vec<Constraint>, id: u64) {
    for c in cs.iter_mut() {
        match c {
            Constraint::Symmetry { pairs, singles, .. } => {
                pairs.retain(|&(a, b)| a != id && b != id);
                singles.retain(|&s| s != id);
            }
            Constraint::Align { items, .. } | Constraint::Fixed { items } => items.retain(|&i| i != id),
        }
    }
    cs.retain(|c| match c {
        Constraint::Symmetry { pairs, singles, .. } => !pairs.is_empty() || !singles.is_empty(),
        Constraint::Align { items, .. } => items.len() > 1,
        Constraint::Fixed { items } => !items.is_empty(),
    });
}

/// Bounding boxes of the top-level items (instances through `src`).
pub fn item_boxes(l: &Layout, src: &dyn LayoutSource) -> BTreeMap<u64, IRect> {
    let mut out: BTreeMap<u64, IRect> = l.shapes.iter().map(|s| (s.id, s.bbox())).collect();
    for inst in &l.instances {
        let Some(child) = src.layout(&inst.cell) else { continue };
        let one = Layout { instances: vec![inst.clone()], ..Default::default() };
        let b = flatten(&one, src, 64).iter().map(|f| f.geom.bbox()).reduce(|a, b| a.union(&b));
        let b = b.or_else(|| child.shapes_bbox().map(|cb| inst.bbox(cb)));
        if let Some(b) = b {
            out.insert(inst.id, b);
        }
    }
    out
}

fn center2(b: IRect) -> (i64, i64) {
    (b.x0 + b.x1, b.y0 + b.y1)
}

/// How far a constraint is from holding (dbu), per constraint; 0 when it holds.
pub fn errors(l: &Layout, src: &dyn LayoutSource) -> Vec<i64> {
    let boxes = item_boxes(l, src);
    l.constraints.iter().map(|c| error(c, &boxes)).collect()
}

fn error(c: &Constraint, boxes: &BTreeMap<u64, IRect>) -> i64 {
    let mut worst = 0;
    match c {
        Constraint::Symmetry { axis, at, pairs, singles } => {
            let v = *axis == Axis::Vertical;
            for &(a, b) in pairs {
                let (Some(&ba), Some(&bb)) = (boxes.get(&a), boxes.get(&b)) else { continue };
                let (ca, cb) = (center2(ba), center2(bb));
                let (along, across) =
                    if v { ((ca.0 + cb.0) - 4 * at, ca.1 - cb.1) } else { ((ca.1 + cb.1) - 4 * at, ca.0 - cb.0) };
                worst = worst.max(along.abs() / 4).max(across.abs() / 2);
            }
            for s in singles {
                let Some(&b) = boxes.get(s) else { continue };
                let c = center2(b);
                worst = worst.max(((if v { c.0 } else { c.1 }) - 2 * at).abs() / 2);
            }
        }
        Constraint::Align { edge, items } => {
            let vals: Vec<i64> = items.iter().filter_map(|i| boxes.get(i)).map(|b| edge.of(*b)).collect();
            if let (Some(lo), Some(hi)) = (vals.iter().min(), vals.iter().max()) {
                worst = hi - lo;
            }
        }
        Constraint::Fixed { .. } => {}
    }
    worst
}

/// Items no constraint may move.
pub fn fixed_items(cs: &[Constraint]) -> BTreeSet<u64> {
    cs.iter()
        .filter_map(|c| match c {
            Constraint::Fixed { items } => Some(items.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Restore the constraints after an edit that moved the items in `moved` (the leaders): their
/// partners follow. A symmetry whose members all moved by the same vector moves its axis with
/// them. Returns the ids of the items moved here.
pub fn enforce(l: &mut Layout, src: &dyn LayoutSource, moved: &BTreeSet<u64>) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    if l.constraints.is_empty() {
        return out;
    }
    let fixed = fixed_items(&l.constraints);
    let mut boxes = item_boxes(l, src);
    let mut cs = std::mem::take(&mut l.constraints);
    for c in cs.iter_mut() {
        let mut shift = |l: &mut Layout, boxes: &mut BTreeMap<u64, IRect>, id: u64, dx: i64, dy: i64| {
            if (dx, dy) == (0, 0) || fixed.contains(&id) {
                return;
            }
            l.translate_item(id, dx, dy);
            if let Some(b) = boxes.get_mut(&id) {
                *b = b.translate(dx, dy);
            }
            out.insert(id);
        };
        match c {
            Constraint::Symmetry { axis, at, pairs, singles } => {
                let v = *axis == Axis::Vertical;
                let members: Vec<u64> = pairs.iter().flat_map(|&(a, b)| [a, b]).chain(singles.iter().copied()).collect();
                let leaders: Vec<u64> = members.iter().copied().filter(|m| moved.contains(m)).collect();
                if leaders.is_empty() {
                    continue;
                }
                if leaders.len() == members.len() {
                    // The whole group moved: the axis goes with it.
                    let c: Vec<(i64, i64)> = members.iter().filter_map(|m| boxes.get(m)).map(|b| center2(*b)).collect();
                    if !c.is_empty() {
                        let sum: i64 = c.iter().map(|p| if v { p.0 } else { p.1 }).sum();
                        *at = (sum as f64 / (2 * c.len()) as f64).round() as i64;
                    }
                    continue;
                }
                for &(a, b) in pairs.iter() {
                    let (lead, follow) = match (moved.contains(&a), moved.contains(&b)) {
                        (true, false) => (a, b),
                        (false, true) => (b, a),
                        _ => continue,
                    };
                    let (Some(&bl), Some(&bf)) = (boxes.get(&lead), boxes.get(&follow)) else { continue };
                    let (cl, cf) = (center2(bl), center2(bf));
                    let (dx, dy) = if v { (4 * *at - cl.0 - cf.0, cl.1 - cf.1) } else { (cl.0 - cf.0, 4 * *at - cl.1 - cf.1) };
                    shift(l, &mut boxes, follow, dx.div_euclid(2), dy.div_euclid(2));
                }
                for &s in singles.iter().filter(|s| !moved.contains(s)) {
                    let Some(&b) = boxes.get(&s) else { continue };
                    let c = center2(b);
                    let d = (2 * *at - if v { c.0 } else { c.1 }).div_euclid(2);
                    if v {
                        shift(l, &mut boxes, s, d, 0);
                    } else {
                        shift(l, &mut boxes, s, 0, d);
                    }
                }
                // A moved single re-centres the axis on itself when no pair member moved.
                if leaders.iter().all(|m| singles.contains(m))
                    && let Some(b) = leaders.first().and_then(|m| boxes.get(m))
                {
                    let c = center2(*b);
                    *at = (if v { c.0 } else { c.1 }).div_euclid(2);
                    for &(a, b) in pairs.iter() {
                        let (Some(&ba), Some(&bb)) = (boxes.get(&a), boxes.get(&b)) else { continue };
                        let (ca, cb) = (center2(ba), center2(bb));
                        let d = (4 * *at - if v { ca.0 + cb.0 } else { ca.1 + cb.1 }).div_euclid(4);
                        for id in [a, b] {
                            if v {
                                shift(l, &mut boxes, id, d, 0);
                            } else {
                                shift(l, &mut boxes, id, 0, d);
                            }
                        }
                    }
                }
            }
            Constraint::Align { edge, items } => {
                let Some(lead) = items.iter().copied().find(|i| moved.contains(i)) else { continue };
                let Some(&lb) = boxes.get(&lead) else { continue };
                let target = edge.of(lb);
                for &i in items.iter().filter(|i| !moved.contains(i)) {
                    let Some(&b) = boxes.get(&i) else { continue };
                    let d = target - edge.of(b);
                    if edge.horizontal() {
                        shift(l, &mut boxes, i, d, 0);
                    } else {
                        shift(l, &mut boxes, i, 0, d);
                    }
                }
            }
            Constraint::Fixed { .. } => {}
        }
    }
    l.constraints = cs;
    out
}

/// A symmetry constraint over `items` about the centre of their extent: items pair up with the
/// item nearest their mirror image, items straddling the axis are centred on it. `None` when an
/// item finds no partner within half its size.
pub fn symmetry_of(l: &Layout, src: &dyn LayoutSource, items: &BTreeSet<u64>, axis: Axis) -> Option<Constraint> {
    let boxes = item_boxes(l, src);
    let bs: Vec<(u64, IRect)> = items.iter().filter_map(|i| boxes.get(i).map(|b| (*i, *b))).collect();
    let ext = bs.iter().map(|(_, b)| *b).reduce(|a, b| a.union(&b))?;
    let v = axis == Axis::Vertical;
    let at = if v { (ext.x0 + ext.x1).div_euclid(2) } else { (ext.y0 + ext.y1).div_euclid(2) };
    let (mut pairs, mut singles) = (vec![], vec![]);
    let mut used = BTreeSet::new();
    for &(id, b) in &bs {
        if used.contains(&id) {
            continue;
        }
        let c = center2(b);
        let pos = if v { c.0 } else { c.1 };
        let size = if v { b.width() } else { b.height() };
        if (pos - 2 * at).abs() <= size / 2 {
            singles.push(id);
            used.insert(id);
            continue;
        }
        let mirror = if v { (4 * at - c.0, c.1) } else { (c.0, 4 * at - c.1) };
        let partner = bs
            .iter()
            .filter(|(o, _)| *o != id && !used.contains(o))
            .map(|(o, ob)| {
                let oc = center2(*ob);
                (o, (oc.0 - mirror.0).abs() + (oc.1 - mirror.1).abs())
            })
            .min_by_key(|(_, d)| *d)?;
        if partner.1 > size.max(1) {
            return None;
        }
        used.insert(id);
        used.insert(*partner.0);
        pairs.push((id, *partner.0));
    }
    Some(Constraint::Symmetry { axis, at, pairs, singles })
}

/// Make every member of `c` satisfy it, moving the right / upper partner of each pair and the
/// singles (the left / lower members lead).
pub fn snap(l: &mut Layout, src: &dyn LayoutSource, k: usize) -> BTreeSet<u64> {
    let Some(c) = l.constraints.get(k).cloned() else { return BTreeSet::new() };
    let leaders: BTreeSet<u64> = match &c {
        Constraint::Symmetry { pairs, .. } => pairs.iter().map(|p| p.0).collect(),
        Constraint::Align { items, .. } => items.first().copied().into_iter().collect(),
        Constraint::Fixed { .. } => BTreeSet::new(),
    };
    let others = std::mem::take(&mut l.constraints);
    l.constraints = vec![c];
    let moved = enforce(l, src, &leaders);
    let c = l.constraints.pop();
    l.constraints = others;
    if let Some(c) = c {
        l.constraints[k] = c;
    }
    moved
}

/// Symmetry constraints for the schematic's matched groups (differential pairs, current mirrors,
/// devices sharing a `match` parameter) whose layout instances are already placed mirror
/// symmetrically about a vertical axis, e.g. by schematic-driven or common-centroid placement.
/// Groups already constrained are skipped. Returns how many constraints were added.
pub fn infer(l: &mut Layout, project: &Project, sch: &Schematic) -> usize {
    let taken: BTreeSet<u64> = l.constraints.iter().flat_map(|c| c.items()).collect();
    let mut added = 0;
    for group in matched_groups(sch) {
        let ids: BTreeSet<u64> = l
            .instances
            .iter()
            .filter(|i| i.name.as_deref().is_some_and(|n| group.iter().any(|g| g == device_of(n))))
            .map(|i| i.id)
            .collect();
        if ids.len() < 2 || ids.iter().any(|i| taken.contains(i)) {
            continue;
        }
        if let Some(c) = symmetry_of(l, project, &ids, Axis::Vertical) {
            let boxes = item_boxes(l, project);
            if error(&c, &boxes) <= project.tech.grid.max(1) * 10 {
                l.constraints.push(c);
                added += 1;
            }
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::NoCells;

    fn pair() -> (Layout, u64, u64, u64) {
        let mut l = Layout::default();
        let a = l.add_rect(1, -1000, 0, -500, 400);
        let b = l.add_rect(1, 500, 0, 1000, 400);
        let s = l.add_rect(2, -100, 600, 100, 800);
        (l, a, b, s)
    }

    #[test]
    fn symmetry_follows_the_moved_member() {
        let (mut l, a, b, s) = pair();
        let c = symmetry_of(&l, &NoCells, &[a, b, s].into(), Axis::Vertical).unwrap();
        assert_eq!(c, Constraint::Symmetry { axis: Axis::Vertical, at: 0, pairs: vec![(a, b)], singles: vec![s] });
        l.constraints.push(c);
        assert_eq!(errors(&l, &NoCells), [0]);
        l.translate_item(a, -200, 50);
        assert!(errors(&l, &NoCells)[0] > 0);
        let moved = enforce(&mut l, &NoCells, &[a].into());
        assert_eq!(moved, [b].into());
        assert_eq!(l.shape(b).unwrap().bbox(), IRect::new(700, 50, 1200, 450));
        assert_eq!(errors(&l, &NoCells), [0]);
        // Moving everything moves the axis.
        for id in [a, b, s] {
            l.translate_item(id, 300, 0);
        }
        assert!(enforce(&mut l, &NoCells, &[a, b, s].into()).is_empty());
        assert!(matches!(l.constraints[0], Constraint::Symmetry { at: 300, .. }));
        assert_eq!(errors(&l, &NoCells), [0]);
        // Moving the centred item re-centres the pairs on it.
        l.translate_item(s, 100, 0);
        enforce(&mut l, &NoCells, &[s].into());
        assert!(matches!(l.constraints[0], Constraint::Symmetry { at: 400, .. }));
        assert_eq!(errors(&l, &NoCells), [0]);
    }

    #[test]
    fn align_and_fixed() {
        let mut l = Layout::default();
        let a = l.add_rect(1, 0, 0, 100, 100);
        let b = l.add_rect(1, 300, 50, 400, 200);
        let c = l.add_rect(1, 600, -20, 700, 300);
        l.constraints.push(Constraint::Align { edge: Edge::Bottom, items: vec![a, b, c] });
        l.constraints.push(Constraint::Fixed { items: vec![c] });
        assert_eq!(errors(&l, &NoCells), [70, 0]);
        l.translate_item(a, 0, 10);
        let moved = enforce(&mut l, &NoCells, &[a].into());
        assert_eq!(moved, [b].into(), "the fixed item stays");
        assert_eq!(l.shape(b).unwrap().bbox().y0, 10);
        l.remove(c);
        assert_eq!(l.constraints.len(), 1, "fixed constraint without items is dropped");
    }

    #[test]
    fn asymmetric_items_have_no_symmetry() {
        let mut l = Layout::default();
        let a = l.add_rect(1, 0, 0, 100, 100);
        let b = l.add_rect(1, 300, 0, 400, 100);
        let c = l.add_rect(1, 380, 0, 2000, 100);
        assert!(symmetry_of(&l, &NoCells, &[a, b, c].into(), Axis::Vertical).is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let (mut l, a, b, s) = pair();
        l.constraints.push(symmetry_of(&l, &NoCells, &[a, b, s].into(), Axis::Vertical).unwrap());
        let text = serde_json::to_string(&l).unwrap();
        assert!(text.contains("\"kind\":\"symmetry\""), "{text}");
        let back: Layout = serde_json::from_str(&text).unwrap();
        assert_eq!(back, l);
        let plain: Layout = serde_json::from_str(r#"{"shapes":[],"next_id":0}"#).unwrap();
        assert!(plain.constraints.is_empty());
    }
}
