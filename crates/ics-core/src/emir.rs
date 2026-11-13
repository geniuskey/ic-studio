//! Electromigration (EM) and IR-drop analysis of one net (typically a supply such as VDD / VSS).
//!
//! - The net's wires on layers with a sheet resistance ([`crate::tech::LayerCap::resistance`]) are
//!   cut into tiles of at most [`SEGMENT_SQUARES`] squares; each tile is a node at its centre.
//!   Touching tiles of a layer are joined by the resistance between their centres, via cuts of the
//!   net by their cut resistance (one resistor per cut).
//! - Pads: top-cell labels carrying the net name hold their tile at the pad voltage.
//! - Loads: every MOS device with its source or drain (not both) on the net draws its current
//!   (given per layout device name) from the tile nearest to its gate, on the first metal if the
//!   net has one there. The network is linear, so a current returned into a ground net gives the
//!   same voltage rise as a drawn current gives drop.
//! - The DC network `G · drop = I` is solved by Jacobi-preconditioned conjugate gradients.
//!   Tiles not connected to a pad are reported as floating.
//! - EM: wire current per µm of width (across the shared edge of two tiles) against
//!   [`crate::tech::LayerCap::em`], via current per cut against [`crate::tech::ViaDef::em`].

use std::collections::{BTreeMap, HashMap, HashSet};

use rstar::{AABB, PointDistance, RTree, RTreeObject};

use crate::extract::{Extraction, extract};
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::lvs::{LvsOptions, compare};
use crate::netlist::element_name;
use crate::pex::{MIN_OHMS, SEGMENT_SQUARES};
use crate::project::{CellRef, Project};
use crate::region::Region;
use crate::tech::Tech;
use crate::units::format_eng;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub layer: usize,
    pub rect: IRect,
    /// Voltage drop from the pads in V (0 for pads and floating tiles).
    pub drop: f64,
    pub pad: bool,
    /// Not connected to any pad through the network.
    pub floating: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BranchKind {
    /// Between two tiles of `layer` sharing an edge of length `width` (dbu).
    Wire { layer: usize, width: i64 },
    /// One cut of technology via `via` (index into [`Tech::vias`]).
    Via { via: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    pub a: usize,
    pub b: usize,
    pub ohms: f64,
    pub kind: BranchKind,
    /// The two tiles (wire) or the cut (via).
    pub at: IRect,
    /// Current from `a` to `b` in A.
    pub current: f64,
    /// |current| per µm of width (wire) or per cut (via).
    pub density: f64,
    /// EM limit of `density` (0 = unchecked).
    pub limit: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    /// Layout device name.
    pub device: String,
    pub node: usize,
    pub amps: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmViolation {
    pub branch: usize,
    /// Layer or via name.
    pub layer: String,
    pub at: IRect,
    pub density: f64,
    pub limit: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmirReport {
    /// Layout net name.
    pub net: String,
    pub nodes: Vec<Node>,
    pub branches: Vec<Branch>,
    pub loads: Vec<Load>,
    pub pads: Vec<usize>,
    /// Worst first (largest density / limit).
    pub violations: Vec<EmViolation>,
    pub worst: Option<usize>,
    pub worst_drop: f64,
    /// Current of the loads on connected tiles, in A.
    pub total_current: f64,
    pub iterations: usize,
    pub warnings: Vec<String>,
}

impl EmirReport {
    /// One-line summary.
    pub fn summary(&self, tech: &Tech) -> String {
        let um = |v: i64| v as f64 / tech.dbu_per_um.max(1) as f64;
        let at = self.worst.map_or(String::new(), |w| {
            let r = self.nodes[w].rect;
            format!(" at ({}, {}) µm", um((r.x0 + r.x1) / 2), um((r.y0 + r.y1) / 2))
        });
        format!(
            "{}: {} nodes, {} resistors, {} pad(s), {} load(s) drawing {}; worst IR drop {}{at}; {} EM violation(s)",
            self.net,
            self.nodes.len(),
            self.branches.len(),
            self.pads.len(),
            self.loads.len(),
            format_eng(self.total_current, "A"),
            format_eng(self.worst_drop, "V"),
            self.violations.len()
        )
    }
}

#[derive(Clone, Copy)]
struct Tile {
    rect: IRect,
    idx: usize,
}

impl RTreeObject for Tile {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.rect.x0, self.rect.y0], [self.rect.x1, self.rect.y1])
    }
}

impl PointDistance for Tile {
    fn distance_2(&self, p: &[i64; 2]) -> i64 {
        self.envelope().distance_2(p)
    }
}

fn within(tree: &RTree<Tile>, r: IRect) -> impl Iterator<Item = &Tile> {
    tree.locate_in_envelope_intersecting(AABB::from_corners([r.x0, r.y0], [r.x1, r.y1]))
}

fn tile_at(tree: &RTree<Tile>, (x, y): (i64, i64)) -> Option<usize> {
    within(tree, IRect::new(x, y, x, y)).filter(|t| t.rect.contains(x, y)).map(|t| t.idx).min()
}

/// Split `r` along its long side into tiles at most [`SEGMENT_SQUARES`] squares long.
fn tiles(r: IRect) -> impl Iterator<Item = IRect> {
    let (long, short) = (r.width().max(r.height()), r.width().min(r.height()).max(1));
    let n = ((long + SEGMENT_SQUARES * short - 1) / (SEGMENT_SQUARES * short)).max(1);
    (0..n).map(move |i| {
        let (a, b) = (long * i / n, long * (i + 1) / n);
        if r.width() >= r.height() {
            IRect::new(r.x0 + a, r.y0, r.x0 + b, r.y1)
        } else {
            IRect::new(r.x0, r.y0 + a, r.x1, r.y0 + b)
        }
    })
}

fn find(uf: &mut [usize], mut i: usize) -> usize {
    while uf[i] != i {
        uf[i] = uf[uf[i]];
        i = uf[i];
    }
    i
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Solve `G x = b` on the free nodes (`x = 0` on fixed ones) for the conductance graph `edges`
/// (node, node, S). Returns the solution and the iterations used.
fn solve(n: usize, edges: &[(usize, usize, f64)], fixed: &[bool], b: &[f64]) -> (Vec<f64>, usize, bool) {
    let mut diag = vec![0.0; n];
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(i, j, g) in edges.iter().filter(|e| e.0 != e.1) {
        diag[i] += g;
        diag[j] += g;
        if !fixed[i] && !fixed[j] {
            adj[i].push((j, g));
            adj[j].push((i, g));
        }
    }
    let mul = |v: &[f64], out: &mut [f64]| {
        for i in 0..n {
            out[i] = if fixed[i] { 0.0 } else { diag[i] * v[i] - adj[i].iter().map(|&(j, g)| g * v[j]).sum::<f64>() };
        }
    };
    let inv: Vec<f64> = (0..n).map(|i| if fixed[i] || diag[i] <= 0.0 { 0.0 } else { 1.0 / diag[i] }).collect();
    let mut x = vec![0.0; n];
    let mut r: Vec<f64> = (0..n).map(|i| if fixed[i] { 0.0 } else { b[i] }).collect();
    let norm_b = dot(&r, &r).sqrt();
    if norm_b == 0.0 {
        return (x, 0, true);
    }
    let mut z: Vec<f64> = r.iter().zip(&inv).map(|(r, d)| r * d).collect();
    let mut p = z.clone();
    let mut rz = dot(&r, &z);
    let mut ap = vec![0.0; n];
    let max = 10 * n + 100;
    for k in 1..=max {
        mul(&p, &mut ap);
        let pap = dot(&p, &ap);
        if pap <= 0.0 {
            return (x, k, false);
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        if dot(&r, &r).sqrt() <= 1e-11 * norm_b {
            return (x, k, true);
        }
        for i in 0..n {
            z[i] = r[i] * inv[i];
        }
        let next = dot(&r, &z);
        let beta = next / rz;
        rz = next;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
    }
    (x, max, false)
}

/// IR drop and EM of layout net `net` (by name, case-insensitive; every extracted net of that
/// name) of `top`, with `currents` in A per layout device name (`M1`, ...; see [`extract`]).
pub fn analyze(
    top: &Layout,
    src: &dyn LayoutSource,
    x: &Extraction,
    tech: &Tech,
    net: &str,
    currents: &BTreeMap<String, f64>,
) -> Result<EmirReport, String> {
    let nets: Vec<usize> = x.nets.iter().enumerate().filter(|(_, n)| n.name.eq_ignore_ascii_case(net)).map(|(i, _)| i).collect();
    let Some(&first) = nets.first() else {
        let labelled: Vec<&str> = x.nets.iter().filter(|n| n.labelled).map(|n| n.name.as_str()).take(20).collect();
        return Err(format!("no net '{net}' in the layout; labelled nets: {}", labelled.join(", ")));
    };
    let mut warnings = Vec::new();
    if nets.len() > 1 {
        warnings.push(format!("'{net}' labels {} unconnected nets; analysing them together", nets.len()));
    }
    let sheet: HashMap<usize, (f64, f64)> = tech
        .caps
        .iter()
        .filter(|c| c.resistance > 0.0)
        .filter_map(|c| tech.layer_index(&c.layer).map(|l| (l, (c.resistance, c.em))))
        .collect();
    if sheet.is_empty() {
        return Err("the technology defines no sheet resistance (caps[].resistance)".into());
    }

    let mut tiles_of: Vec<(usize, IRect)> = Vec::new();
    for &i in &nets {
        for (l, r) in x.nets[i].shapes.iter().filter(|(l, _)| sheet.contains_key(l)) {
            tiles_of.extend(r.rects().iter().flat_map(|&q| tiles(q)).map(|q| (*l, q)));
        }
    }
    if tiles_of.is_empty() {
        return Err(format!("net '{}' has no wires on layers with a sheet resistance", x.nets[first].name));
    }
    let mut by_layer: BTreeMap<usize, Vec<Tile>> = BTreeMap::new();
    for (idx, &(l, rect)) in tiles_of.iter().enumerate() {
        by_layer.entry(l).or_default().push(Tile { rect, idx });
    }
    let trees: BTreeMap<usize, RTree<Tile>> = by_layer.into_iter().map(|(l, v)| (l, RTree::bulk_load(v))).collect();

    let mut branches: Vec<Branch> = Vec::new();
    let branch = |a: usize, b: usize, ohms: f64, kind: BranchKind, at: IRect| Branch {
        a,
        b,
        ohms: ohms.max(MIN_OHMS),
        kind,
        at,
        current: 0.0,
        density: 0.0,
        limit: 0.0,
    };
    for (i, &(l, a)) in tiles_of.iter().enumerate() {
        let rs = sheet[&l].0;
        for q in within(&trees[&l], a).filter(|q| q.idx > i) {
            let b = q.rect;
            let (wa, ha, wb, hb) = (a.width() as f64, a.height() as f64, b.width() as f64, b.height() as f64);
            let (ox, oy) = (a.x1.min(b.x1) - a.x0.max(b.x0), a.y1.min(b.y1) - a.y0.max(b.y0));
            let kind = |width| BranchKind::Wire { layer: l, width };
            if (a.x1 == b.x0 || b.x1 == a.x0) && oy > 0 {
                branches.push(branch(i, q.idx, rs * (wa / 2.0 / ha + wb / 2.0 / hb), kind(oy), a.union(&b)));
            } else if (a.y1 == b.y0 || b.y1 == a.y0) && ox > 0 {
                branches.push(branch(i, q.idx, rs * (ha / 2.0 / wa + hb / 2.0 / wb), kind(ox), a.union(&b)));
            }
        }
    }

    let bbox = nets.iter().filter_map(|&i| x.nets[i].bbox).reduce(|a, b| a.union(&b)).unwrap_or_default();
    let vias: Vec<(usize, usize, usize, usize)> = tech
        .vias
        .iter()
        .enumerate()
        .filter_map(|(k, v)| Some((k, tech.layer_index(&v.bottom)?, tech.layer_index(&v.cut)?, tech.layer_index(&v.top)?)))
        .filter(|(_, b, _, t)| trees.contains_key(b) && trees.contains_key(t))
        .collect();
    let cut_layers: HashSet<usize> = vias.iter().map(|v| v.2).collect();
    let mut cuts: HashMap<usize, Vec<IRect>> = HashMap::new();
    for f in flatten(top, src, 64).into_iter().filter(|f| cut_layers.contains(&f.layer)) {
        if let Ok(r) = Region::from_geometry(&f.geom) {
            cuts.entry(f.layer).or_default().extend(r.rects().iter().filter(|q| q.intersects(&bbox)));
        }
    }
    let mut seen: HashSet<(IRect, usize, usize)> = HashSet::new();
    for &(k, b, c, t) in &vias {
        for &cut in cuts.get(&c).into_iter().flatten() {
            let p = ((cut.x0 + cut.x1) / 2, (cut.y0 + cut.y1) / 2);
            if let (Some(sb), Some(st)) = (tile_at(&trees[&b], p), tile_at(&trees[&t], p))
                && seen.insert((cut, sb, st))
            {
                branches.push(branch(sb, st, tech.vias[k].resistance, BranchKind::Via { via: k }, cut));
            }
        }
    }

    let mut pads: Vec<usize> = Vec::new();
    for s in &top.shapes {
        let Geometry::Label { text, at, .. } = &s.geom else { continue };
        if !text.trim().eq_ignore_ascii_case(net) {
            continue;
        }
        let own = trees.get(&tech.labelled_layer(s.layer).unwrap_or(s.layer)).and_then(|t| tile_at(t, *at));
        match own.or_else(|| trees.values().find_map(|t| tile_at(t, *at))) {
            Some(n) if !pads.contains(&n) => pads.push(n),
            Some(_) => {}
            None => warnings.push(format!("pin '{text}' at ({}, {}) is not on a resistive wire of the net", at.0, at.1)),
        }
    }
    if pads.is_empty() {
        return Err(format!("no pin labelled '{net}' on the net's wires; label the supply pads with the net name"));
    }

    let by_name: HashMap<String, f64> = currents.iter().map(|(k, v)| (k.trim().to_ascii_uppercase(), *v)).collect();
    let metal1 = tech.metal1();
    let mut loads: Vec<Load> = Vec::new();
    for d in &x.devices {
        if nets.contains(&d.d) == nets.contains(&d.s) {
            continue;
        }
        let Some(&amps) = by_name.get(&d.name.to_ascii_uppercase()) else { continue };
        let c = [(d.gate.x0 + d.gate.x1) / 2, (d.gate.y0 + d.gate.y1) / 2];
        let node = trees
            .iter()
            .filter_map(|(l, t)| t.nearest_neighbor(c).map(|n| (n.distance_2(&c), Some(*l) != metal1, n.idx)))
            .min()
            .map(|(_, _, n)| n)
            .expect("the net has tiles");
        loads.push(Load { device: d.name.clone(), node, amps });
    }
    let devices: HashSet<String> = x.devices.iter().map(|d| d.name.to_ascii_uppercase()).collect();
    let unknown: Vec<&str> =
        currents.keys().filter(|k| !devices.contains(&k.trim().to_ascii_uppercase())).map(String::as_str).collect();
    if !unknown.is_empty() {
        let more = if unknown.len() > 10 { format!(" and {} more", unknown.len() - 10) } else { String::new() };
        warnings.push(format!("no layout device {}{more}; currents ignored", unknown[..unknown.len().min(10)].join(", ")));
    }
    if loads.is_empty() {
        warnings.push(format!("no device current on '{net}'"));
    }

    let n = tiles_of.len();
    let mut uf: Vec<usize> = (0..n).collect();
    for br in &branches {
        let (a, b) = (find(&mut uf, br.a), find(&mut uf, br.b));
        uf[a.max(b)] = a.min(b);
    }
    let fed: HashSet<usize> = pads.iter().map(|&p| find(&mut uf, p)).collect();
    let floating: Vec<bool> = (0..n).map(|i| !fed.contains(&find(&mut uf, i))).collect();
    let fixed: Vec<bool> = (0..n).map(|i| floating[i] || pads.contains(&i)).collect();
    let mut rhs = vec![0.0; n];
    let mut total = 0.0;
    for l in &loads {
        if floating[l.node] {
            warnings.push(format!("{} draws from a wire not connected to a '{net}' pin", l.device));
        } else {
            rhs[l.node] += l.amps;
            total += l.amps;
        }
    }
    let n_floating = floating.iter().filter(|f| **f).count();
    if n_floating > 0 {
        warnings.push(format!("{n_floating} of {n} wire tiles are not connected to a '{net}' pin"));
    }
    let edges: Vec<(usize, usize, f64)> = branches.iter().map(|b| (b.a, b.b, 1.0 / b.ohms)).collect();
    let (drop, iterations, converged) = solve(n, &edges, &fixed, &rhs);
    if !converged {
        warnings.push(format!("the solver did not converge in {iterations} iterations"));
    }

    let um = tech.dbu_per_um.max(1) as f64;
    let mut violations = Vec::new();
    for (k, b) in branches.iter_mut().enumerate() {
        b.current = (drop[b.b] - drop[b.a]) / b.ohms;
        let (name, density, limit) = match b.kind {
            BranchKind::Wire { layer, width } => {
                (tech.layer(layer).map(|d| d.name.clone()), b.current.abs() / (width as f64 / um), sheet[&layer].1)
            }
            BranchKind::Via { via } => (Some(tech.vias[via].name.clone()), b.current.abs(), tech.vias[via].em),
        };
        (b.density, b.limit) = (density, limit);
        if limit > 0.0 && density > limit {
            violations.push(EmViolation { branch: k, layer: name.unwrap_or_default(), at: b.at, density, limit });
        }
    }
    violations.sort_by(|p, q| (q.density / q.limit).total_cmp(&(p.density / p.limit)));

    let nodes: Vec<Node> = tiles_of
        .iter()
        .enumerate()
        .map(|(i, &(layer, rect))| Node { layer, rect, drop: drop[i], pad: pads.contains(&i), floating: floating[i] })
        .collect();
    let worst = (0..n).filter(|&i| !floating[i]).max_by(|&a, &b| drop[a].total_cmp(&drop[b]));
    Ok(EmirReport {
        net: x.nets[first].name.clone(),
        worst_drop: worst.map_or(0.0, |w| drop[w]),
        worst,
        nodes,
        branches,
        loads,
        pads,
        violations,
        total_current: total,
        iterations,
        warnings,
    })
}

/// [`analyze`] on the layout of `cell`.
pub fn analyze_cell(
    project: &Project,
    cell: &CellRef,
    net: &str,
    currents: &BTreeMap<String, f64>,
) -> Result<EmirReport, String> {
    let lay = project.layout(cell).ok_or_else(|| format!("cell '{cell}' has no layout"))?;
    let x = extract(lay, project, &project.tech);
    analyze(lay, project, &x, &project.tech, net, currents)
}

/// Currents per layout device name from currents per schematic element name (as in the deck:
/// `M1`, `XM2`, ...; the magnitude is used), via LVS of `cell`. A merged group of parallel
/// devices shares its total current equally.
pub fn layout_currents(
    project: &Project,
    cell: &CellRef,
    by_element: &BTreeMap<String, f64>,
) -> Result<BTreeMap<String, f64>, String> {
    let c = project.cell(cell).ok_or_else(|| format!("no cell '{cell}'"))?;
    let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
    let lay = c.layout.as_ref().ok_or("cell has no layout")?;
    let x = extract(lay, project, &project.tech);
    let rep = compare(sch, &x, &LvsOptions::for_tech(&project.tech));
    let by: HashMap<String, f64> = by_element.iter().map(|(k, v)| (k.to_ascii_uppercase(), v.abs())).collect();
    let mut out = BTreeMap::new();
    for (sname, lname) in &rep.devices {
        let amps: f64 = sch
            .instances
            .iter()
            .filter(|i| sname.split('+').any(|n| n == i.name))
            .filter_map(|i| by.get(&element_name(i, &project.tech).to_ascii_uppercase()))
            .sum();
        let members: Vec<&str> = lname.split('+').collect();
        for m in &members {
            *out.entry(m.to_string()).or_default() += amps / members.len() as f64;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{DevKind, Device, Junction, Net};
    use crate::hier::NoCells;

    fn net(name: &str, shapes: Vec<(usize, Vec<IRect>)>) -> Net {
        let shapes: Vec<(usize, Region)> = shapes.into_iter().map(|(l, rs)| (l, Region::from_rects(rs))).collect();
        let bbox = shapes.iter().filter_map(|(_, r)| r.bbox()).reduce(|a, b| a.union(&b));
        Net { name: name.into(), labelled: true, shapes, bbox }
    }

    /// An nmos `name` with its source on net 0 at gate centre `at`.
    fn device(name: &str, at: (i64, i64)) -> Device {
        Device {
            kind: DevKind::Nmos,
            model: "nmos".into(),
            name: name.into(),
            w: 1000,
            l: 180,
            d: 1,
            g: 1,
            s: 0,
            b: 1,
            gate: IRect::new(at.0 - 90, at.1 - 500, at.0 + 90, at.1 + 500),
            jd: Junction::default(),
            js: Junction::default(),
        }
    }

    fn setup(shapes: Vec<(usize, Vec<IRect>)>, devices: Vec<Device>) -> Extraction {
        Extraction { nets: vec![net("vdd", shapes), net("other", vec![])], devices, dbu_per_um: 1000, ..Default::default() }
    }

    fn pins(layer: usize, at: &[(i64, i64)]) -> Layout {
        let mut lay = Layout::default();
        for &at in at {
            lay.add(layer, Geometry::Label { text: "VDD".into(), at, size: 100 });
        }
        lay
    }

    fn amps(v: &[(&str, f64)]) -> BTreeMap<String, f64> {
        v.iter().map(|(k, a)| (k.to_string(), *a)).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1e-15)
    }

    #[test]
    fn straight_wire_matches_analytic_drop() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let rs = tech.layer_cap("metal1").unwrap().resistance;
        // 100 x 1 µm: ten 10-square tiles; pad on the first, a 0.5 mA load on the last.
        let x = setup(vec![(m1, vec![IRect::new(0, 0, 100_000, 1000)])], vec![device("M1", (99_000, 500))]);
        let rep = analyze(&pins(m1, &[(100, 500)]), &NoCells, &x, &tech, "vdd", &amps(&[("m1", 0.5e-3)])).unwrap();
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        assert_eq!((rep.nodes.len(), rep.branches.len(), rep.pads.len(), rep.loads.len()), (10, 9, 1, 1));
        let expected = rs * 90.0 * 0.5e-3;
        assert!(close(rep.worst_drop, expected), "{} vs {expected}", rep.worst_drop);
        assert_eq!(rep.nodes[rep.worst.unwrap()].rect, IRect::new(90_000, 0, 100_000, 1000));
        assert!(rep.branches.iter().all(|b| close(b.current.abs(), 0.5e-3) && close(b.density, 0.5e-3)));
        assert!(rep.violations.is_empty());
        assert!(rep.summary(&tech).contains("1 load(s)"), "{}", rep.summary(&tech));
    }

    #[test]
    fn ladder_with_symmetric_loads() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        // Two 40 µm rails joined by three straps; pins at both ends of the bottom rail and two
        // equal loads on the top rail, mirrored about x = 20 µm.
        let rects = vec![
            IRect::new(0, 0, 40_000, 1000),
            IRect::new(0, 20_000, 40_000, 21_000),
            IRect::new(0, 1000, 1000, 20_000),
            IRect::new(19_500, 1000, 20_500, 20_000),
            IRect::new(39_000, 1000, 40_000, 20_000),
        ];
        let x = setup(vec![(m1, rects)], vec![device("M1", (5_000, 20_500)), device("M2", (35_000, 20_500))]);
        let lay = pins(m1, &[(100, 500), (39_900, 500)]);
        let rep = analyze(&lay, &NoCells, &x, &tech, "vdd", &amps(&[("M1", 1e-3), ("M2", 1e-3)])).unwrap();
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        assert_eq!(rep.pads.len(), 2);
        let (d1, d2) = (rep.nodes[rep.loads[0].node].drop, rep.nodes[rep.loads[1].node].drop);
        assert!(d1 > 0.0 && close(d1, d2), "{d1} vs {d2}");
        assert!(rep.worst_drop >= d1);
        // Each pin supplies half of the total current.
        for &p in &rep.pads {
            let out: f64 = rep.branches.iter().filter(|b| b.a == p).map(|b| b.current).sum::<f64>()
                - rep.branches.iter().filter(|b| b.b == p).map(|b| b.current).sum::<f64>();
            assert!(close(out, 1e-3), "pad {p} supplies {out}");
        }
        // Kirchhoff: the net current into every free tile is its load.
        let mut net_in = vec![0.0; rep.nodes.len()];
        for b in &rep.branches {
            net_in[b.b] += b.current;
            net_in[b.a] -= b.current;
        }
        for (i, v) in net_in.iter().enumerate().filter(|(i, _)| !rep.pads.contains(i)) {
            let load: f64 = rep.loads.iter().filter(|l| l.node == i).map(|l| l.amps).sum();
            assert!((v - load).abs() < 1e-12, "tile {i}: {v} vs {load}");
        }
    }

    #[test]
    fn narrow_wire_and_single_via_violate_em() {
        let tech = Tech::generic();
        let (m1, m2, via1) =
            (tech.layer_index("metal1").unwrap(), tech.layer_index("metal2").unwrap(), tech.layer_index("via1").unwrap());
        // A 0.3 µm metal2 wire carrying 1 mA (3.3 mA/µm) through one via1 cut onto a wide metal1.
        let x = setup(
            vec![(m1, vec![IRect::new(0, 0, 3000, 3000)]), (m2, vec![IRect::new(2500, 1350, 20_000, 1650)])],
            vec![device("M1", (19_800, 1500))],
        );
        let mut lay = pins(m1, &[(500, 500)]);
        lay.add_rect(via1, 2620, 1370, 2880, 1630);
        let rep = analyze(&lay, &NoCells, &x, &tech, "VDD", &amps(&[("M1", 1e-3), ("M9", 1.0)])).unwrap();
        assert!(rep.warnings.iter().any(|w| w.contains("M9")), "{:?}", rep.warnings);
        assert!(rep.branches.iter().any(|b| matches!(b.kind, BranchKind::Via { .. })), "{:?}", rep.branches);
        let layers: HashSet<&str> = rep.violations.iter().map(|v| v.layer.as_str()).collect();
        assert_eq!(layers, HashSet::from(["metal2", "via1"]));
        let wire = rep.violations.iter().find(|v| v.layer == "metal2").unwrap();
        assert!(close(wire.density, 1e-3 / 0.3) && wire.limit == 1e-3, "{wire:?}");
        assert!(rep.violations[0].density / rep.violations[0].limit >= wire.density / wire.limit);

        // A wide wire with the same current passes the wire check.
        let wide = setup(vec![(m2, vec![IRect::new(0, 0, 40_000, 2000)])], vec![device("M1", (39_800, 1000))]);
        let rep = analyze(&pins(m2, &[(100, 1000)]), &NoCells, &wide, &tech, "vdd", &amps(&[("M1", 1e-3)])).unwrap();
        assert!(rep.violations.is_empty(), "{:?}", rep.violations);
    }

    #[test]
    fn errors_and_floating_pieces() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let x = setup(
            vec![(m1, vec![IRect::new(0, 0, 10_000, 1000), IRect::new(20_000, 0, 30_000, 1000)])],
            vec![device("M1", (29_000, 500))],
        );
        let none = BTreeMap::new();
        assert!(analyze(&Layout::default(), &NoCells, &x, &tech, "gnd", &none).unwrap_err().contains("no net"));
        assert!(analyze(&Layout::default(), &NoCells, &x, &tech, "vdd", &none).unwrap_err().contains("no pin"));
        let rep = analyze(&pins(m1, &[(100, 500)]), &NoCells, &x, &tech, "vdd", &amps(&[("M1", 1e-3)])).unwrap();
        assert!(rep.nodes.iter().any(|n| n.floating) && rep.total_current == 0.0 && rep.worst_drop == 0.0);
        assert!(rep.warnings.iter().any(|w| w.contains("not connected")), "{:?}", rep.warnings);
    }

    #[test]
    fn demo_inverter_supply() {
        let p = crate::examples::demo_project();
        let r = CellRef::new("work", "cmos_inverter");
        let sch = p.cell(&r).unwrap().schematic.as_ref().unwrap();
        let by_element: BTreeMap<String, f64> = sch
            .instances
            .iter()
            .filter(|i| matches!(i.kind, crate::schematic::DeviceKind::Pmos))
            .map(|i| (element_name(i, &p.tech), -2e-4))
            .collect();
        let currents = layout_currents(&p, &r, &by_element).unwrap();
        assert_eq!(currents.values().filter(|a| **a > 0.0).count(), 1, "{currents:?}");
        let rep = analyze_cell(&p, &r, "vdd", &currents).unwrap();
        assert_eq!(rep.loads.len(), 1, "{:?}", rep.warnings);
        assert!(rep.worst_drop > 0.0 && close(rep.total_current, 2e-4), "{}", rep.summary(&p.tech));
    }
}
