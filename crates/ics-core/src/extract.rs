//! Layout connectivity extraction and MOS device recognition.
//!
//! Works on the flattened top cell with Manhattan region operations:
//! - Layers are found through the technology's [`LayerRoles`](crate::tech::LayerRoles).
//! - Conducting layers are the bottom / top layers of the technology vias. The diffusion layers
//!   merge into one conductor, `diffusion NOT poly`, so gates split it into source / drain pieces.
//! - Each edge-connected piece of a conducting layer is a node; via cuts join the bottom and
//!   top pieces they overlap. A cut shared by several vias (contact on active, polycon on poly)
//!   joins whichever bottom it lands on; contacts on a gate connect nothing.
//! - Well taps: `n+` diffusion inside nwell joins the well, `p+` diffusion outside it joins the
//!   global substrate net "sub".
//! - Gates are the pieces of `diffusion AND poly`. PMOS when inside the p implant (or, without an
//!   implant, inside nwell). L is measured across the two diffusion pieces, W = area / L. The
//!   model is the first PDK device ([`Tech::devices`]) of that polarity whose marker layers
//!   cover the gate, else the generic `nmos` / `pmos`.
//! - Junctions: each source / drain diffusion piece contributes its area and its perimeter
//!   without the edges against the device gates; a piece shared by several terminals (fingers,
//!   abutted devices) is split equally between them.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rstar::{AABB, RTree, RTreeObject};

use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::region::{BoolOp, Region};
use crate::tech::{MosType, Tech};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevKind {
    Nmos,
    Pmos,
}

impl DevKind {
    pub fn model(self) -> &'static str {
        match self {
            DevKind::Nmos => "nmos",
            DevKind::Pmos => "pmos",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Net {
    /// Label text, or a generated name ("net12", "nwell1", "sub") when unlabelled.
    pub name: String,
    pub labelled: bool,
    /// Geometry of the net per conducting layer, in top-cell coordinates.
    pub shapes: Vec<(usize, Region)>,
    /// Extent of `shapes` (`None` for the shapeless substrate net).
    pub bbox: Option<IRect>,
}

impl Net {
    fn contains(&self, layer: Option<usize>, (x, y): (i64, i64)) -> bool {
        self.bbox.is_some_and(|b| b.contains(x, y))
            && self
                .shapes
                .iter()
                .filter(|(l, _)| layer.is_none_or(|w| w == *l))
                .any(|(_, r)| r.rects().iter().any(|q| q.contains(x, y)))
    }
}

/// Source or drain junction of a device, in dbu² and dbu.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Junction {
    pub area: f64,
    /// Perimeter excluding the gate edges.
    pub perimeter: f64,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub kind: DevKind,
    /// Model name: the recognised PDK device, else `kind.model()`.
    pub model: String,
    /// "M1", "M2", ... in a deterministic (position based) order.
    pub name: String,
    /// Gate width and length in dbu.
    pub w: i64,
    pub l: i64,
    /// Net indices.
    pub d: usize,
    pub g: usize,
    pub s: usize,
    pub b: usize,
    pub gate: IRect,
    pub jd: Junction,
    pub js: Junction,
}

#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub nets: Vec<Net>,
    pub devices: Vec<Device>,
    pub warnings: Vec<String>,
    /// Conducting layers, topmost first (the order `net_at_any` and labels use).
    pub layers: Vec<usize>,
    /// Database units per micron of the technology used.
    pub dbu_per_um: i64,
}

impl Extraction {
    /// Net of the conducting geometry on `layer` at `p`.
    pub fn net_at(&self, layer: usize, p: (i64, i64)) -> Option<usize> {
        self.nets.iter().position(|n| n.contains(Some(layer), p))
    }

    /// Net of any conducting geometry at `p`, topmost layer first.
    pub fn net_at_any(&self, p: (i64, i64)) -> Option<usize> {
        self.layers.iter().find_map(|&l| self.net_at(l, p))
    }

    /// The extracted circuit as a SPICE subcircuit body (devices only, `w=`/`l=` in metres).
    pub fn spice(&self, title: &str, dbu_per_um: i64) -> String {
        let um = |v: i64| format!("{}u", v as f64 / dbu_per_um.max(1) as f64);
        let net =
            |i: usize| self.nets.get(i).map_or("?".to_string(), |n| n.name.split_whitespace().collect::<Vec<_>>().join("_"));
        let mut s = format!("* {title}\n");
        for d in &self.devices {
            s += &format!(
                "{} {} {} {} {} {} w={} l={}\n",
                d.name,
                net(d.d),
                net(d.g),
                net(d.s),
                net(d.b),
                d.model,
                um(d.w),
                um(d.l)
            );
        }
        s
    }
}

#[derive(Clone, Copy)]
struct Entry {
    rect: IRect,
    idx: usize,
}

impl RTreeObject for Entry {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.rect.x0, self.rect.y0], [self.rect.x1, self.rect.y1])
    }
}

/// One layer's region with its edge-connected components and a spatial index over its rects.
#[derive(Default)]
struct Plane {
    region: Region,
    comp: Vec<usize>,
    members: Vec<Vec<usize>>,
    tree: RTree<Entry>,
}

impl Plane {
    fn new(region: Region) -> Plane {
        let rs = region.rects();
        let n = rs.len();
        let mut uf = Dsu::new(n);
        for (i, r) in rs.iter().enumerate() {
            let mut j = rs.partition_point(|q| q.y0 < r.y1 || (q.y0 == r.y1 && q.x1 <= r.x0));
            while j < n && rs[j].y0 == r.y1 && rs[j].x0 < r.x1 {
                uf.union(i, j);
                j += 1;
            }
        }
        let mut ids: HashMap<usize, usize> = HashMap::new();
        let mut members: Vec<Vec<usize>> = Vec::new();
        let comp = (0..n)
            .map(|i| {
                let root = uf.find(i);
                let c = *ids.entry(root).or_insert_with(|| {
                    members.push(Vec::new());
                    members.len() - 1
                });
                members[c].push(i);
                c
            })
            .collect();
        let tree = RTree::bulk_load(rs.iter().enumerate().map(|(idx, &rect)| Entry { rect, idx }).collect());
        Plane { region, comp, members, tree }
    }

    fn count(&self) -> usize {
        self.members.len()
    }

    fn candidates(&self, r: IRect) -> impl Iterator<Item = &Entry> {
        self.tree.locate_in_envelope_intersecting(AABB::from_corners([r.x0, r.y0], [r.x1, r.y1]))
    }

    /// Component containing `p` (half-open, or closed rect test when `closed`).
    fn comp_at(&self, (x, y): (i64, i64), closed: bool) -> Option<usize> {
        self.candidates(IRect::new(x, y, x, y))
            .find(|e| {
                let r = e.rect;
                if closed { r.contains(x, y) } else { r.x0 <= x && x < r.x1 && r.y0 <= y && y < r.y1 }
            })
            .map(|e| self.comp[e.idx])
    }

    /// Components overlapping `r` with positive area.
    fn comps_overlapping(&self, r: IRect) -> BTreeSet<usize> {
        self.candidates(r).filter(|e| e.rect.intersects(&r)).map(|e| self.comp[e.idx]).collect()
    }

    fn comp_rects(&self, c: usize) -> impl Iterator<Item = IRect> + '_ {
        self.members[c].iter().map(|&i| self.region.rects()[i])
    }
}

struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu { parent: (0..n).collect() }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a.max(b)] = a.min(b);
        }
    }
}

/// Anchor point of every component of `r`: the lower-left corner of its first rect.
fn component_anchors(r: Region) -> Vec<(i64, i64)> {
    let p = Plane::new(r);
    p.members.iter().map(|m| (p.region.rects()[m[0]].x0, p.region.rects()[m[0]].y0)).collect()
}

struct Gate {
    kind: DevKind,
    model: String,
    rect: IRect,
    rects: Vec<IRect>,
    /// Diffusion components (plane-local) of the source and drain.
    sd: (usize, usize),
    w: i64,
    l: i64,
    s: usize,
    d: usize,
    g: usize,
    b: usize,
}

/// Extract nets and MOS devices from the flattened hierarchy of `top`.
pub fn extract(top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Extraction {
    let n_layers = tech.layers.len();
    let mut warnings = Vec::new();
    let mut rects: Vec<Vec<IRect>> = vec![Vec::new(); n_layers];
    let mut labels: Vec<(usize, String, (i64, i64))> = Vec::new();
    let mut skipped = 0usize;
    // A depth cap keeps cyclic references (invalid, but possible in imported data) finite.
    for s in flatten(top, src, 64) {
        if s.layer >= n_layers {
            continue;
        }
        match s.geom {
            // Only the top cell's labels name nets; labels inside placed cells are local to them.
            Geometry::Label { text, at, .. } if s.id.is_some() => labels.push((s.layer, text, at)),
            Geometry::Label { .. } => {}
            Geometry::Rect(r) => rects[s.layer].push(r),
            g => match Region::from_geometry(&g) {
                Ok(r) => rects[s.layer].extend_from_slice(r.rects()),
                Err(_) => skipped += 1,
            },
        }
    }
    if skipped > 0 {
        warnings.push(format!("{skipped} non-Manhattan shapes were skipped"));
    }
    let region = |l: Option<usize>| l.map(|l| Region::from_rects(rects[l].iter().copied())).unwrap_or_default();
    let lid = |n: &str| tech.layer_index(n);
    let diffusions = tech.diffusion_layers();
    let active = diffusions.first().copied();
    let (poly, nwell, nplus, pplus) = (tech.poly(), tech.nwell(), tech.implant(false), tech.implant(true));
    // Every diffusion layer conducts as the first one.
    let canon = |l: usize| if diffusions.contains(&l) { diffusions[0] } else { l };

    let mut conducting: Vec<usize> = Vec::new();
    let mut vias: Vec<(usize, usize, usize)> = Vec::new();
    let mut diff_cuts: Vec<usize> = Vec::new();
    for v in &tech.vias {
        let (Some(b), Some(c), Some(t)) = (lid(&v.bottom), lid(&v.cut), lid(&v.top)) else {
            warnings.push(format!("via '{}' refers to missing layers", v.name));
            continue;
        };
        if diffusions.contains(&b) && !diff_cuts.contains(&c) {
            diff_cuts.push(c);
        }
        let (b, t) = (canon(b), canon(t));
        vias.push((b, c, t));
        for l in [b, t] {
            if !conducting.contains(&l) {
                conducting.push(l);
            }
        }
    }
    let mut level = vec![0usize; n_layers];
    for _ in 0..vias.len() {
        for &(b, _, t) in &vias {
            level[t] = level[t].max(level[b] + 1);
        }
    }
    conducting.sort_by_key(|&l| std::cmp::Reverse((level[l], l)));

    let active_r = Region::from_rects(diffusions.iter().flat_map(|&l| rects[l].iter().copied()));
    let poly_r = region(poly);
    let gate_r = active_r.boolean(&poly_r, BoolOp::And);
    let diff_r = active_r.boolean(&poly_r, BoolOp::Not);
    let mut planes: Vec<Plane> =
        conducting.iter().map(|&l| Plane::new(if Some(l) == active { diff_r.clone() } else { region(Some(l)) })).collect();
    let nwell_r = region(nwell);
    let has_nwell = nwell.is_some() && !nwell_r.is_empty();
    planes.push(Plane::new(nwell_r.clone()));
    let nwell_pi = planes.len() - 1;
    let pi_of = |l: Option<usize>| l.and_then(|l| conducting.iter().position(|&c| c == l));
    let (diff_pi, poly_pi) = (pi_of(active), pi_of(poly));

    let mut offset = Vec::with_capacity(planes.len());
    let mut total = 0;
    for p in &planes {
        offset.push(total);
        total += p.count();
    }
    let sub = total;
    let mut uf = Dsu::new(total + 1);
    let mut sub_used = false;

    let mut cuts: HashMap<usize, Region> = HashMap::new();
    for &(b, c, t) in &vias {
        let (Some(bp), Some(tp)) = (pi_of(Some(b)), pi_of(Some(t))) else { continue };
        let cut = cuts.entry(c).or_insert_with(|| region(Some(c)));
        let mut bottom = planes[bp].region.clone();
        if Some(b) == poly {
            bottom = bottom.boolean(&active_r, BoolOp::Not);
        }
        let overlap = cut.boolean(&bottom, BoolOp::And).boolean(&planes[tp].region, BoolOp::And);
        for p in component_anchors(overlap) {
            if let (Some(x), Some(y)) = (planes[bp].comp_at(p, false), planes[tp].comp_at(p, false)) {
                uf.union(offset[bp] + x, offset[tp] + y);
            }
        }
    }
    for c in diff_cuts {
        let on_gate = cuts.entry(c).or_insert_with(|| region(Some(c))).boolean(&gate_r, BoolOp::And);
        if !on_gate.is_empty() {
            warnings.push(format!("{} contact cut(s) on a gate are ignored", component_anchors(on_gate).len()));
        }
    }
    // PDK device markers, evaluated once per layer.
    let markers: HashMap<usize, Plane> = tech
        .devices
        .iter()
        .flat_map(|d| d.layers.iter().chain(&d.exclude))
        .filter_map(|n| lid(n))
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .map(|l| (l, Plane::new(region(Some(l)))))
        .collect();
    let covers =
        |name: &str, p: (i64, i64)| lid(name).and_then(|l| markers.get(&l)).is_some_and(|m| m.comp_at(p, false).is_some());

    // Without an n implant layer, n+ is the diffusion outside the p implant.
    let nplus_p = Plane::new(match nplus {
        Some(_) => region(nplus),
        None if tech.roles.nimplant.is_empty() => active_r.boolean(&region(pplus), BoolOp::Not),
        None => Region::default(),
    });
    let pplus_p = Plane::new(region(pplus));
    if let Some(dp) = diff_pi {
        let diff = &planes[dp].region;
        let ntap = diff.boolean(&nwell_r, BoolOp::And).boolean(&nplus_p.region, BoolOp::And);
        for p in component_anchors(ntap) {
            if let (Some(x), Some(w)) = (planes[dp].comp_at(p, false), planes[nwell_pi].comp_at(p, false)) {
                uf.union(offset[dp] + x, offset[nwell_pi] + w);
            }
        }
        let ptap = diff.boolean(&pplus_p.region, BoolOp::And).boolean(&nwell_r, BoolOp::Not);
        for p in component_anchors(ptap) {
            if let Some(x) = planes[dp].comp_at(p, false) {
                uf.union(offset[dp] + x, sub);
                sub_used = true;
            }
        }
    }

    let mut gates = Vec::new();
    if let (Some(dp), Some(pp)) = (diff_pi, poly_pi) {
        let gp = Plane::new(gate_r);
        for m in &gp.members {
            let rs: Vec<IRect> = m.iter().map(|&i| gp.region.rects()[i]).collect();
            let Some(bb) = rs.iter().copied().reduce(|a, b| a.union(&b)) else { continue };
            let area: i64 = rs.iter().map(|r| r.width() * r.height()).sum();
            let side = |r: IRect| planes[dp].comps_overlapping(r).into_iter().next();
            let (left, right) =
                (side(IRect::new(bb.x0 - 1, bb.y0, bb.x0, bb.y1)), side(IRect::new(bb.x1, bb.y0, bb.x1 + 1, bb.y1)));
            let (below, above) =
                (side(IRect::new(bb.x0, bb.y0 - 1, bb.x1, bb.y0)), side(IRect::new(bb.x0, bb.y1, bb.x1, bb.y1 + 1)));
            let (s, d, l) = match (left, right, below, above) {
                (Some(s), Some(d), _, _) => (s, d, bb.width()),
                (_, _, Some(s), Some(d)) => (s, d, bb.height()),
                _ => {
                    warnings.push(format!(
                        "gate at ({}, {})-({}, {}) lacks diffusion on two opposite sides; skipped",
                        bb.x0, bb.y0, bb.x1, bb.y1
                    ));
                    continue;
                }
            };
            let anchor = (rs[0].x0, rs[0].y0);
            let Some(g) = planes[pp].comp_at(anchor, false) else { continue };
            let in_nwell = planes[nwell_pi].comp_at(anchor, false);
            let pmos = if pplus_p.comp_at(anchor, false).is_some() {
                true
            } else if nplus_p.comp_at(anchor, false).is_some() {
                false
            } else {
                in_nwell.is_some()
            };
            let b = match (pmos, in_nwell) {
                (true, Some(w)) => offset[nwell_pi] + w,
                (true, None) => {
                    warnings.push(format!("pmos gate at ({}, {}) is not inside an nwell; bulk tied to sub", bb.x0, bb.y0));
                    sub_used = true;
                    sub
                }
                (false, _) => {
                    sub_used = true;
                    sub
                }
            };
            let kind = if pmos { DevKind::Pmos } else { DevKind::Nmos };
            let ty = if pmos { MosType::Pmos } else { MosType::Nmos };
            let model = if tech.devices.is_empty() {
                kind.model().to_string()
            } else {
                match tech.devices.iter().find(|d| {
                    d.kind == ty && d.layers.iter().all(|n| covers(n, anchor)) && !d.exclude.iter().any(|n| covers(n, anchor))
                }) {
                    Some(d) => d.name.clone(),
                    None => {
                        warnings.push(format!(
                            "{} gate at ({}, {}) matches no device of the technology",
                            kind.model(),
                            bb.x0,
                            bb.y0
                        ));
                        kind.model().to_string()
                    }
                }
            };
            gates.push(Gate {
                kind,
                model,
                rect: bb,
                rects: rs,
                sd: (s, d),
                w: area / l.max(1),
                l,
                s: offset[dp] + s,
                d: offset[dp] + d,
                g: offset[pp] + g,
                b,
            });
        }
    }

    let mut junctions: HashMap<usize, Junction> = HashMap::new();
    if let Some(dp) = diff_pi {
        let mut users: BTreeMap<usize, (usize, Vec<IRect>)> = BTreeMap::new();
        for g in &gates {
            for c in [g.sd.0, g.sd.1] {
                let u = users.entry(c).or_default();
                u.0 += 1;
                u.1.extend_from_slice(&g.rects);
            }
        }
        for (c, (refs, gate_rects)) in users {
            let piece = Region::from_rects(planes[dp].comp_rects(c));
            let gates = Region::from_rects(gate_rects);
            let p = piece.perimeter();
            let gate_edge = (p + gates.perimeter() - piece.boolean(&gates, BoolOp::Or).perimeter()) / 2;
            let k = refs as f64;
            junctions.insert(c, Junction { area: piece.area() as f64 / k, perimeter: (p - gate_edge) as f64 / k });
        }
    }

    // Nets: group pieces by root, ordered by position.
    let mut roots: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (pi, p) in planes.iter().enumerate() {
        for c in 0..p.count() {
            roots.entry(uf.find(offset[pi] + c)).or_default().push((pi, c));
        }
    }
    if sub_used {
        roots.entry(uf.find(sub)).or_default();
    }
    struct Proto {
        root: usize,
        shapes: Vec<(usize, Region)>,
        bbox: Option<IRect>,
        well_only: bool,
    }
    let mut protos: Vec<Proto> = roots
        .into_iter()
        .map(|(root, pieces)| {
            let mut per_plane: BTreeMap<usize, Vec<IRect>> = BTreeMap::new();
            for (pi, c) in pieces {
                per_plane.entry(pi).or_default().extend(planes[pi].comp_rects(c));
            }
            let well_only = per_plane.keys().all(|&pi| pi == nwell_pi) && !per_plane.is_empty();
            let shapes: Vec<(usize, Region)> = per_plane
                .into_iter()
                .filter_map(|(pi, rs)| {
                    let layer = if pi == nwell_pi { nwell? } else { conducting[pi] };
                    Some((layer, Region::from_rects(rs)))
                })
                .collect();
            let bbox = shapes.iter().filter_map(|(_, r)| r.bbox()).reduce(|a, b| a.union(&b));
            Proto { root, shapes, bbox, well_only }
        })
        .collect();
    protos.sort_by_key(|p| (p.bbox.is_none(), p.bbox.map(|b| (b.y0, b.x0, b.y1, b.x1)), p.root));
    let net_of_root: HashMap<usize, usize> = protos.iter().enumerate().map(|(i, p)| (p.root, i)).collect();

    // Labels.
    let mut search: Vec<usize> = (0..conducting.len()).collect();
    if has_nwell {
        search.push(nwell_pi);
    }
    let mut net_labels: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for (layer, text, at) in &labels {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let own = pi_of(Some(*layer)).and_then(|pi| planes[pi].comp_at(*at, true).map(|c| offset[pi] + c));
        let hit = own.or_else(|| search.iter().find_map(|&pi| planes[pi].comp_at(*at, true).map(|c| offset[pi] + c)));
        match hit {
            Some(piece) => {
                net_labels.entry(net_of_root[&uf.find(piece)]).or_default().insert(text.to_string());
            }
            None => warnings.push(format!("label '{text}' at ({}, {}) is not on conducting geometry", at.0, at.1)),
        }
    }
    let mut label_nets: BTreeMap<&str, usize> = BTreeMap::new();
    for set in net_labels.values() {
        if set.len() > 1 {
            let all: Vec<&str> = set.iter().map(String::as_str).collect();
            warnings.push(format!("net labelled {} (shorted labels); using '{}'", all.join(", "), all[0]));
        }
        for t in set {
            *label_nets.entry(t.as_str()).or_default() += 1;
        }
    }
    for (t, n) in &label_nets {
        if *n > 1 {
            warnings.push(format!("label '{t}' is on {n} unconnected nets"));
        }
    }

    let (mut auto, mut wells) = (0, 0);
    let sub_root = sub_used.then(|| uf.find(sub));
    let nets: Vec<Net> = protos
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let (name, labelled) = match net_labels.get(&i).and_then(|s| s.iter().next()) {
                Some(t) => (t.clone(), true),
                None if Some(p.root) == sub_root => ("sub".to_string(), false),
                None if p.well_only => {
                    wells += 1;
                    (format!("nwell{wells}"), false)
                }
                None => {
                    auto += 1;
                    (format!("net{auto}"), false)
                }
            };
            Net { name, labelled, shapes: p.shapes, bbox: p.bbox }
        })
        .collect();

    gates.sort_by_key(|g| (g.rect.y0, g.rect.x0));
    let mut net = |piece: usize| net_of_root[&uf.find(piece)];
    let devices = gates
        .into_iter()
        .enumerate()
        .map(|(i, g)| Device {
            kind: g.kind,
            model: g.model,
            name: format!("M{}", i + 1),
            w: g.w,
            l: g.l,
            d: net(g.d),
            g: net(g.g),
            s: net(g.s),
            b: net(g.b),
            gate: g.rect,
            js: junctions.get(&g.sd.0).copied().unwrap_or_default(),
            jd: junctions.get(&g.sd.1).copied().unwrap_or_default(),
        })
        .collect();

    let mut layers = conducting;
    if has_nwell && let Some(nw) = nwell {
        layers.push(nw);
    }
    Extraction { nets, devices, warnings, layers, dbu_per_um: tech.dbu_per_um }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::NoCells;
    use crate::pcell::PCell;
    use crate::project::CellRef;

    fn names(x: &Extraction, d: &Device) -> [String; 4] {
        [d.d, d.g, d.s, d.b].map(|i| x.nets[i].name.clone())
    }

    #[test]
    fn single_transistor() {
        let tech = Tech::generic();
        let l = |n: &str| tech.layer_index(n).unwrap();
        let mut lay = Layout::default();
        lay.add_rect(l("nplus"), -200, -200, 2200, 1200);
        lay.add_rect(l("active"), 0, 0, 2000, 1000);
        lay.add_rect(l("poly"), 900, -300, 1080, 1300);
        lay.add_rect(l("poly"), 780, 1300, 1200, 1720);
        for (x, y) in [(200, 390), (1500, 390), (880, 1400)] {
            lay.add_rect(l("contact"), x, y, x + 220, y + 220);
            lay.add_rect(l("metal1"), x - 100, y - 100, x + 320, y + 320);
        }
        for (t, at) in [("a", (150, 350)), ("b", (1450, 350)), ("in", (800, 1320))] {
            lay.add(l("text"), Geometry::Label { text: t.into(), at, size: 100 });
        }
        let x = extract(&lay, &NoCells, &tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_eq!(x.devices.len(), 1);
        let d = &x.devices[0];
        assert_eq!((d.kind, d.w, d.l, d.gate), (DevKind::Nmos, 1000, 180, IRect::new(900, 0, 1080, 1000)));
        assert_eq!(names(&x, d), ["b", "in", "a", "sub"].map(String::from));
        assert_eq!(
            x.spice("t", 1000),
            "* t
M1 b in a sub nmos w=1u l=0.18u
"
        );
        let a = x.nets.iter().position(|n| n.name == "a").unwrap();
        assert_eq!(x.net_at(l("metal1"), (150, 350)), Some(a));
        assert_eq!(x.net_at(l("active"), (50, 50)), Some(a));
        assert_eq!(x.net_at(l("metal1"), (50, 50)), None);
        assert_eq!(x.net_at_any((d.gate.x0 + 10, 500)).map(|i| x.nets[i].name.as_str()), Some("in"));
        assert_eq!(x.net_at_any((5000, 5000)), None);
    }

    #[test]
    fn rotated_gate_and_shapes() {
        let tech = Tech::generic();
        let l = |n: &str| tech.layer_index(n).unwrap();
        let mut lay = Layout::default();
        lay.add_rect(l("pplus"), -200, -200, 1200, 2200);
        lay.add_rect(l("nwell"), -600, -600, 1600, 2600);
        lay.add_rect(l("active"), 0, 0, 1000, 2000);
        lay.add(l("poly"), Geometry::Path { points: vec![(-300, 1000), (1300, 1000)], width: 200 });
        lay.add(l("poly"), Geometry::Polygon { points: vec![(0, 0), (10, 5), (0, 10)] });
        let x = extract(&lay, &NoCells, &tech);
        assert_eq!(x.devices.len(), 1);
        let d = &x.devices[0];
        assert_eq!((d.kind, d.w, d.l), (DevKind::Pmos, 1000, 200));
        assert_eq!(names(&x, d), ["net3", "net2", "net1", "nwell1"].map(String::from));
        assert!(x.warnings.iter().any(|w| w.contains("1 non-Manhattan")), "{:?}", x.warnings);
    }

    #[test]
    fn demo_inverter() {
        let tech = Tech::generic();
        let x = extract(&crate::examples::inverter_layout(&tech), &NoCells, &tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        let (n, p) = (&x.devices[0], &x.devices[1]);
        assert_eq!((n.kind, n.w, n.l), (DevKind::Nmos, 1000, 180));
        assert_eq!((p.kind, p.w, p.l), (DevKind::Pmos, 1200, 180));
        assert_eq!(names(&x, n), ["out", "in", "gnd", "sub"].map(String::from));
        assert_eq!(names(&x, p), ["out", "in", "vdd", "nwell1"].map(String::from));
        let labelled: Vec<&str> = x.nets.iter().filter(|n| n.labelled).map(|n| n.name.as_str()).collect();
        assert_eq!(labelled, ["gnd", "in", "out", "vdd"]);
        assert_eq!(x.spice("inv", 1000).lines().nth(2), Some("M2 out in vdd nwell1 pmos w=1.2u l=0.18u"));
    }

    #[test]
    fn inv_chain_connects_stages() {
        let p = crate::examples::demo_project();
        let top = p.cell(&CellRef::new("work", "inv_chain")).and_then(|c| c.layout.as_ref()).unwrap();
        let x = extract(top, &p, &p.tech);
        assert_eq!(x.devices.len(), 8);
        let (nmos, pmos) = x.devices.split_at(4);
        assert!(nmos.iter().all(|d| d.kind == DevKind::Nmos) && pmos.iter().all(|d| d.kind == DevKind::Pmos));
        for k in 0..3 {
            assert_eq!(nmos[k].d, nmos[k + 1].g, "stage {k} output drives stage {}", k + 1);
            assert_eq!(pmos[k].d, nmos[k + 1].g);
        }
        let gates: BTreeSet<usize> = nmos.iter().map(|d| d.g).collect();
        assert_eq!(gates.len(), 4);
        assert!(nmos.iter().all(|d| x.nets[d.s].name == "gnd") && pmos.iter().all(|d| x.nets[d.s].name == "vdd"));
        assert_eq!(x.nets[nmos[3].d].name, "out");
        assert_eq!(x.nets[nmos[0].g].name, "in");
        assert!(!x.warnings.iter().any(|w| w.contains("unconnected nets")), "child labels must not name nets: {:?}", x.warnings);
    }

    #[test]
    fn pcell_fingers() {
        let tech = Tech::generic();
        let lay = PCell::mos(false, 1000, 180, 2).generate(&tech).unwrap();
        let x = extract(&lay, &NoCells, &tech);
        assert_eq!(x.devices.len(), 2);
        assert!(x.devices.iter().all(|d| d.kind == DevKind::Nmos && d.w == 1000 && d.l == 180));
        assert_eq!(x.devices[0].g, x.devices[1].g);
        assert_eq!(x.devices[0].d, x.devices[1].s);
        assert_ne!(x.devices[0].s, x.devices[1].d);
        assert!(x.warnings.iter().any(|w| w == "label 'S' is on 2 unconnected nets"), "{:?}", x.warnings);
    }

    #[test]
    fn demo_devices_cell() {
        let p = crate::examples::demo_project();
        let top = p.cell(&CellRef::new("work", "devices")).and_then(|c| c.layout.as_ref()).unwrap();
        let x = extract(top, &p, &p.tech);
        let kinds: Vec<(DevKind, i64)> = x.devices.iter().map(|d| (d.kind, d.w)).collect();
        assert_eq!(kinds, [(DevKind::Nmos, 1000), (DevKind::Pmos, 2000)]);
    }

    #[test]
    #[ignore]
    fn bench_inv_tiles() {
        let p = crate::examples::demo_project();
        let top = p.cell(&CellRef::new("work", "inv_tiles")).and_then(|c| c.layout.as_ref()).unwrap();
        let t = std::time::Instant::now();
        let x = extract(top, &p, &p.tech);
        println!("inv_tiles: {} devices, {} nets in {:?}", x.devices.len(), x.nets.len(), t.elapsed());
        assert_eq!(x.devices.len(), 2 * 8192);
    }
}
