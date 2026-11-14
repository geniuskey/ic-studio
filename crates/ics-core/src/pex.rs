//! Parasitic extraction and its back-annotation onto the schematic deck for post-layout
//! simulation:
//! - Wire capacitance to substrate per net from area and fringe coefficients.
//! - Coupling capacitance between nets. Lateral: facing parallel edges of two nets on one layer,
//!   `coupling · L / s` for run length L at spacing s, up to [`COUPLING_RANGE`] minimum spacings
//!   and only where no other geometry of that layer lies in between. Vertical: overlap of two
//!   capacitance layers not covered by one in between, with the plate coefficient derived from the
//!   substrate area coefficients, `1 / (1/c_upper - 1/c_lower)` (a lower layer has the larger one).
//!   Substrate capacitance is not reduced where a wire is shielded, so totals are conservative.
//! - MOS drain / source junctions from the extracted diffusion (`ad as pd ps` on the schematic
//!   device lines, so the device models compute the junction capacitance).
//! - Layout-dependent stress for PDK models ([`junction_params`] with `lde`): `nf sa sb sd` from
//!   the gates' distances to the edges of their rectangular diffusion (BSIM4 convention).
//! - Wire resistance ([`rc_networks`]): the wires of a net on layers with a sheet resistance
//!   become segments joined by resistors, vias by their cut resistance. A schematic net with a
//!   network keeps its name on its largest node; each MOS terminal on it moves to the node nearest
//!   to the device's gate (so poly gate resistance is included), and the net's capacitance to
//!   ground is spread over the nodes by wire area. Coupling stays on the named node.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rstar::{AABB, RTree, RTreeObject};

use crate::connectivity;
use crate::extract::{Device, Extraction, Junction, extract};
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::Layout;
use crate::lvs::{LvsOptions, LvsReport, compare};
use crate::netlist::spice_name;
use crate::project::{CellRef, Project};
use crate::region::{BoolOp, Region};
use crate::schematic::{DeviceKind, Schematic};
use crate::tech::LengthUnit;
use crate::tech::Tech;

/// Lateral coupling is extracted up to this many minimum spacings of the layer.
pub const COUPLING_RANGE: i64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct NetCap {
    /// Layout net name.
    pub net: String,
    pub farads: f64,
}

/// Coupling capacitance between two layout nets (`a` < `b` by net index).
#[derive(Debug, Clone, PartialEq)]
pub struct Coupling {
    pub a: String,
    pub b: String,
    pub farads: f64,
}

/// Capacitance to substrate of every extracted net (largest first).
pub fn net_caps(x: &Extraction, tech: &Tech) -> Vec<NetCap> {
    let um = tech.dbu_per_um.max(1) as f64;
    let coef: BTreeMap<usize, (f64, f64)> =
        tech.caps.iter().filter_map(|c| tech.layer_index(&c.layer).map(|l| (l, (c.area, c.fringe)))).collect();
    let mut caps: Vec<NetCap> = x
        .nets
        .iter()
        .map(|n| {
            let af: f64 = n
                .shapes
                .iter()
                .filter_map(|(l, r)| coef.get(l).map(|&(a, f)| a * r.area() as f64 / (um * um) + f * r.perimeter() as f64 / um))
                .sum();
            NetCap { net: n.name.clone(), farads: af * 1e-18 }
        })
        .filter(|c| c.farads > 0.0)
        .collect();
    caps.sort_by(|a, b| b.farads.total_cmp(&a.farads).then_with(|| a.net.cmp(&b.net)));
    caps
}

struct Piece {
    rect: IRect,
    net: usize,
}

impl RTreeObject for Piece {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.rect.x0, self.rect.y0], [self.rect.x1, self.rect.y1])
    }
}

fn within(tree: &RTree<Piece>, r: IRect) -> impl Iterator<Item = &Piece> {
    tree.locate_in_envelope_intersecting(AABB::from_corners([r.x0, r.y0], [r.x1, r.y1]))
}

fn overlap(a: &IRect, b: &IRect) -> IRect {
    IRect::new(a.x0.max(b.x0), a.y0.max(b.y0), a.x1.min(b.x1), a.y1.min(b.y1))
}

/// Coupling capacitance between every pair of extracted nets (largest first).
pub fn coupling_caps(x: &Extraction, tech: &Tech) -> Vec<Coupling> {
    let um = tech.dbu_per_um.max(1) as f64;
    let mut layers: Vec<(usize, f64, f64)> =
        tech.caps.iter().filter_map(|c| tech.layer_index(&c.layer).map(|l| (l, c.area, c.coupling))).collect();
    layers.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut pieces: HashMap<usize, Vec<Piece>> = HashMap::new();
    for (net, n) in x.nets.iter().enumerate() {
        for (l, r) in &n.shapes {
            pieces.entry(*l).or_default().extend(r.rects().iter().map(|&rect| Piece { rect, net }));
        }
    }
    let trees: HashMap<usize, RTree<Piece>> = pieces.into_iter().map(|(l, p)| (l, RTree::bulk_load(p))).collect();
    let mut af: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut add = |a: usize, b: usize, v: f64| {
        if a != b && v > 0.0 {
            *af.entry((a.min(b), a.max(b))).or_default() += v;
        }
    };

    for &(l, _, k) in &layers {
        let range = COUPLING_RANGE * tech.layer(l).map_or(0, |d| d.min_spacing);
        let Some(tree) = trees.get(&l).filter(|_| k > 0.0 && range > 0) else { continue };
        for p in tree.iter() {
            let r = p.rect;
            for horizontal in [true, false] {
                let window = if horizontal {
                    IRect::new(r.x1, r.y0, r.x1 + range, r.y1)
                } else {
                    IRect::new(r.x0, r.y1, r.x1, r.y1 + range)
                };
                for q in within(tree, window).filter(|q| q.net != p.net) {
                    let (s, gap) = if horizontal {
                        (q.rect.x0 - r.x1, IRect::new(r.x1, r.y0.max(q.rect.y0), q.rect.x0, r.y1.min(q.rect.y1)))
                    } else {
                        (q.rect.y0 - r.y1, IRect::new(r.x0.max(q.rect.x0), r.y1, r.x1.min(q.rect.x1), q.rect.y0))
                    };
                    let len = if horizontal { gap.height() } else { gap.width() };
                    if s <= 0 || s > range || len <= 0 || within(tree, gap).any(|o| o.rect.intersects(&gap)) {
                        continue;
                    }
                    add(p.net, q.net, k * len as f64 / s as f64);
                }
            }
        }
    }

    for (i, &(lo, c_lo, _)) in layers.iter().enumerate() {
        for (j, &(hi, c_hi, _)) in layers.iter().enumerate().skip(i + 1) {
            let (Some(t_lo), Some(t_hi)) = (trees.get(&lo), trees.get(&hi)) else { continue };
            if c_hi <= 0.0 || c_hi >= c_lo {
                continue;
            }
            let plate = 1.0 / (1.0 / c_hi - 1.0 / c_lo);
            let between: Vec<&RTree<Piece>> = layers[i + 1..j].iter().filter_map(|m| trees.get(&m.0)).collect();
            for p in t_hi.iter() {
                for q in within(t_lo, p.rect).filter(|q| q.net != p.net && q.rect.intersects(&p.rect)) {
                    let ov = overlap(&p.rect, &q.rect);
                    let shields: Vec<IRect> =
                        between.iter().flat_map(|t| within(t, ov)).filter(|m| m.rect.intersects(&ov)).map(|m| m.rect).collect();
                    let area = if shields.is_empty() {
                        (ov.width() as i128 * ov.height() as i128) as f64
                    } else {
                        Region::from_rects([ov]).boolean(&Region::from_rects(shields), BoolOp::Not).area() as f64
                    };
                    add(p.net, q.net, plate * area / (um * um));
                }
            }
        }
    }

    let mut out: Vec<Coupling> = af
        .into_iter()
        .map(|((a, b), v)| Coupling { a: x.nets[a].name.clone(), b: x.nets[b].name.clone(), farads: v * 1e-18 })
        .collect();
    out.sort_by(|p, q| q.farads.total_cmp(&p.farads).then_with(|| (&p.a, &p.b).cmp(&(&q.a, &q.b))));
    out
}

fn spice_ident(net: &str) -> String {
    net.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// SPICE capacitor lines on schematic nets, via LVS net pairs (schematic, layout).
/// Capacitances below `min_farads` are dropped.
pub fn annotate(caps: &[NetCap], net_pairs: &[(String, String)], min_farads: f64) -> String {
    let by_net: BTreeMap<&str, f64> = caps.iter().map(|c| (c.net.as_str(), c.farads)).collect();
    let mut lines: BTreeMap<String, String> = BTreeMap::new();
    for (sch, lay) in net_pairs {
        let Some(&f) = by_net.get(lay.as_str()) else { continue };
        if sch == "0" || f < min_farads {
            continue;
        }
        let name = format!("Cpar_{}", spice_ident(sch));
        lines.entry(name.clone()).or_insert_with(|| format!("{name} {sch} 0 {f:.3e}\n"));
    }
    lines.into_values().collect()
}

/// `ad as pd ps` per schematic MOS instance (by SPICE name, upper case) from the matched layout
/// devices. Merged parallel devices share their junctions in proportion to `m`. With `lde` (PDK
/// models only: level-1 models reject them) the stress parameters follow, see [`lde_params`].
pub fn junction_params(
    sch: &Schematic,
    x: &Extraction,
    rep: &LvsReport,
    unit: LengthUnit,
    lde: bool,
) -> BTreeMap<String, String> {
    let conn = connectivity::extract(sch);
    let insts: HashMap<&str, _> = sch
        .instances
        .iter()
        .filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos))
        .map(|i| (i.name.as_str(), i))
        .collect();
    let devs: HashMap<&str, _> = x.devices.iter().map(|d| (d.name.as_str(), d)).collect();
    let to_layout: HashMap<&str, &str> = rep.nets.iter().map(|(s, l)| (s.as_str(), l.as_str())).collect();
    let f = unit.per_metre();
    let (um, m2) = (x.dbu_per_um.max(1) as f64, 1e-12 * f * f / (x.dbu_per_um.max(1) as f64).powi(2));
    let mut out = BTreeMap::new();
    for (sname, lname) in &rep.devices {
        let mut total: HashMap<&str, Junction> = HashMap::new();
        let fingers: Vec<&Device> = lname.split('+').filter_map(|n| devs.get(n).copied()).collect();
        for d in &fingers {
            for (net, j) in [(d.d, d.jd), (d.s, d.js)] {
                let t = total.entry(x.nets[net].name.as_str()).or_default();
                t.area += j.area;
                t.perimeter += j.perimeter;
            }
        }
        let members: Vec<_> = sname.split('+').filter_map(|n| insts.get(n)).collect();
        let mult = |i: &crate::schematic::Instance| crate::units::parse_spice(i.param("m")).filter(|m| *m >= 1.0).unwrap_or(1.0);
        let units: f64 = members.iter().map(|i| mult(i)).sum();
        for inst in members {
            let side = |pin: &str| {
                let lay = conn.net_of(inst.id, pin).and_then(|n| to_layout.get(n));
                lay.and_then(|n| total.get(n)).copied().unwrap_or_default()
            };
            let (mut jd, mut js) = (side("d"), side("s"));
            if conn.net_of(inst.id, "d") == conn.net_of(inst.id, "s") {
                (jd.area, jd.perimeter) = (jd.area / 2.0, jd.perimeter / 2.0);
                js = jd;
            }
            let k = 1.0 / units.max(1.0);
            let mut p = format!(
                "ad={:.4e} as={:.4e} pd={:.4e} ps={:.4e}",
                jd.area * k * m2,
                js.area * k * m2,
                jd.perimeter * k / um * 1e-6 * f,
                js.perimeter * k / um * 1e-6 * f
            );
            if lde {
                // The low side of a layout device is its source; flipped when that is the schematic drain.
                let drain = conn.net_of(inst.id, "d").and_then(|n| to_layout.get(n)).copied();
                let flip = |d: &Device| drain == Some(x.nets[d.s].name.as_str()) && d.s != d.d;
                p += &lde_params(&fingers, units, flip, |v| v as f64 / um * 1e-6 * f);
            }
            out.insert(spice_name(inst).to_ascii_uppercase(), p);
        }
    }
    out
}

/// ` nf=.. sa=.. sb=.. sd=..` of one of `units` schematic units sharing the layout `fingers`.
/// `nf` is the fingers per unit. `sa` / `sb` are the distances from the gate to the diffusion
/// edge on the source / drain side; with several fingers on one diffusion, those of the outermost
/// fingers, and `sd` the uniform spacing between adjacent gates. Left out when the geometry is
/// ambiguous: a bent or any-angle gate, non-rectangular or separate diffusions, uneven spacing,
/// or single-finger units whose devices differ.
fn lde_params(fingers: &[&Device], units: f64, flip: impl Fn(&Device) -> bool, len: impl Fn(i64) -> f64) -> String {
    let k = fingers.len() as f64 / units;
    if fingers.is_empty() || k < 1.0 || k.fract() != 0.0 {
        return String::new();
    }
    let nf = k as usize;
    let oriented = |d: &Device, lo: i64, hi: i64| if flip(d) { (hi, lo) } else { (lo, hi) };
    let ch: Option<Vec<_>> = fingers.iter().map(|d| d.channel().map(|c| (*d, c))).collect();
    let geom = ch.and_then(|mut ch| {
        if nf == 1 {
            let ext: Vec<(i64, i64)> = ch.iter().map(|(d, (_, g, od))| oriented(d, g.0 - od.0, od.1 - g.1)).collect();
            return ext.iter().all(|e| *e == ext[0]).then_some((ext[0], None));
        }
        let (d0, (axis, _, od)) = ch[0];
        if nf != fingers.len() || ch.iter().any(|(d, c)| c.0 != axis || d.od != d0.od) {
            return None;
        }
        ch.sort_by_key(|(_, c)| c.1.0);
        let gaps: Vec<i64> = ch.windows(2).map(|w| w[1].1.1.0 - w[0].1.1.1).collect();
        let sd = gaps[0];
        if sd <= 0 || gaps.iter().any(|g| *g != sd) {
            return None;
        }
        let ((first, (_, g0, _)), (_, (_, gn, _))) = (ch[0], ch[ch.len() - 1]);
        Some((oriented(first, g0.0 - od.0, od.1 - gn.1), Some(sd)))
    });
    let mut s = format!(" nf={nf}");
    if let Some(((sa, sb), sd)) = geom {
        s += &format!(" sa={:.4e} sb={:.4e}", len(sa), len(sb));
        if let Some(sd) = sd {
            s += &format!(" sd={:.4e}", len(sd));
        }
    }
    s
}

/// Wire segments are at most this many squares long, so taps along a wire see the resistance
/// between them.
pub const SEGMENT_SQUARES: i64 = 10;
/// Resistances below this (Ω) are treated as shorts.
pub const MIN_OHMS: f64 = 1e-3;

/// Resistor network of one net over its layers with a sheet resistance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RcNet {
    /// Wire segments (layer, rect).
    pub segments: Vec<(usize, IRect)>,
    /// Node of each segment; segments joined by a negligible resistance share a node.
    pub node: Vec<usize>,
    pub nodes: usize,
    /// Resistors (node, node, Ω).
    pub resistors: Vec<(usize, usize, f64)>,
}

impl RcNet {
    /// Node of the segment nearest to `p`.
    pub fn node_at(&self, p: (i64, i64)) -> Option<usize> {
        let d = |r: &IRect| {
            let dx = (r.x0 - p.0).max(p.0 - r.x1).max(0) as i128;
            let dy = (r.y0 - p.1).max(p.1 - r.y1).max(0) as i128;
            dx * dx + dy * dy
        };
        self.segments.iter().enumerate().min_by_key(|(_, (_, r))| d(r)).map(|(i, _)| self.node[i])
    }

    /// Wire area per node (dbu²).
    pub fn node_areas(&self) -> Vec<f64> {
        let mut a = vec![0.0; self.nodes];
        for (i, (_, r)) in self.segments.iter().enumerate() {
            a[self.node[i]] += r.width() as f64 * r.height() as f64;
        }
        a
    }
}

/// Split `r` along its long side into pieces at most [`SEGMENT_SQUARES`] squares long.
fn segments(r: IRect) -> Vec<IRect> {
    let (long, short) = (r.width().max(r.height()), r.width().min(r.height()).max(1));
    let n = ((long + SEGMENT_SQUARES * short - 1) / (SEGMENT_SQUARES * short)).max(1);
    (0..n)
        .map(|i| {
            let (a, b) = (long * i / n, long * (i + 1) / n);
            if r.width() >= r.height() {
                IRect::new(r.x0 + a, r.y0, r.x0 + b, r.y1)
            } else {
                IRect::new(r.x0, r.y0 + a, r.x1, r.y0 + b)
            }
        })
        .collect()
}

fn root(uf: &mut [usize], mut i: usize) -> usize {
    while uf[i] != i {
        uf[i] = uf[uf[i]];
        i = uf[i];
    }
    i
}

/// Resistor networks of the extracted nets (`None` for nets without resistive wires), from the
/// sheet resistances of the technology layers and the via cuts of `top`. Pieces of a net joined
/// only through layers without resistance (e.g. diffusion) are tied together.
pub fn rc_networks(top: &Layout, src: &dyn LayoutSource, x: &Extraction, tech: &Tech) -> Vec<Option<RcNet>> {
    let sheet: HashMap<usize, f64> = tech
        .caps
        .iter()
        .filter(|c| c.resistance > 0.0)
        .filter_map(|c| tech.layer_index(&c.layer).map(|l| (l, c.resistance)))
        .collect();
    if sheet.is_empty() {
        return vec![None; x.nets.len()];
    }
    let vias: Vec<(usize, usize, usize, f64)> = tech
        .vias
        .iter()
        .filter_map(|v| Some((tech.layer_index(&v.bottom)?, tech.layer_index(&v.cut)?, tech.layer_index(&v.top)?, v.resistance)))
        .filter(|(b, _, t, _)| sheet.contains_key(b) && sheet.contains_key(t))
        .collect();
    let cut_layers: BTreeSet<usize> = vias.iter().map(|v| v.1).collect();
    let mut cut_rects: HashMap<usize, Vec<Piece>> = HashMap::new();
    for f in flatten(top, src, 64).into_iter().filter(|f| cut_layers.contains(&f.layer)) {
        if let Ok(r) = Region::from_geometry(&f.geom) {
            cut_rects.entry(f.layer).or_default().extend(r.rects().iter().map(|&rect| Piece { rect, net: 0 }));
        }
    }
    let cuts: HashMap<usize, RTree<Piece>> = cut_rects.into_iter().map(|(l, v)| (l, RTree::bulk_load(v))).collect();

    x.nets
        .iter()
        .map(|net| {
            let mut segs: Vec<(usize, IRect)> = Vec::new();
            for (l, r) in net.shapes.iter().filter(|(l, _)| sheet.contains_key(l)) {
                segs.extend(r.rects().iter().flat_map(|&q| segments(q)).map(|q| (*l, q)));
            }
            if segs.is_empty() {
                return None;
            }
            let mut by_layer: HashMap<usize, Vec<Piece>> = HashMap::new();
            for (i, &(l, rect)) in segs.iter().enumerate() {
                by_layer.entry(l).or_default().push(Piece { rect, net: i });
            }
            let trees: HashMap<usize, RTree<Piece>> = by_layer.into_iter().map(|(l, v)| (l, RTree::bulk_load(v))).collect();
            let mut edges: Vec<(usize, usize, f64)> = Vec::new();
            for (i, &(l, a)) in segs.iter().enumerate() {
                let rs = sheet[&l];
                for q in within(&trees[&l], a).filter(|q| q.net > i) {
                    let b = q.rect;
                    let (wa, ha, wb, hb) = (a.width() as f64, a.height() as f64, b.width() as f64, b.height() as f64);
                    let across_x = (a.x1 == b.x0 || b.x1 == a.x0) && a.y0.max(b.y0) < a.y1.min(b.y1);
                    let across_y = (a.y1 == b.y0 || b.y1 == a.y0) && a.x0.max(b.x0) < a.x1.min(b.x1);
                    if across_x {
                        edges.push((i, q.net, rs * (wa / 2.0 / ha + wb / 2.0 / hb)));
                    } else if across_y {
                        edges.push((i, q.net, rs * (ha / 2.0 / wa + hb / 2.0 / wb)));
                    }
                }
            }
            let bb = net.bbox?;
            for &(b, c, t, ohms) in &vias {
                let (Some(cut_tree), Some(tb), Some(tt)) = (cuts.get(&c), trees.get(&b), trees.get(&t)) else { continue };
                for cut in within(cut_tree, bb) {
                    let p = ((cut.rect.x0 + cut.rect.x1) / 2, (cut.rect.y0 + cut.rect.y1) / 2);
                    let at = |tree: &RTree<Piece>| {
                        tree.locate_in_envelope_intersecting(AABB::from_point([p.0, p.1]))
                            .find(|s| s.rect.contains(p.0, p.1))
                            .map(|s| s.net)
                    };
                    if let (Some(sb), Some(st)) = (at(tb), at(tt)) {
                        edges.push((sb, st, ohms));
                    }
                }
            }
            let n = segs.len();
            let mut uf: Vec<usize> = (0..n).collect();
            let mut conn: Vec<usize> = (0..n).collect();
            for &(a, b, ohms) in &edges {
                let (x, y) = (root(&mut conn, a), root(&mut conn, b));
                conn[x.max(y)] = x.min(y);
                if ohms < MIN_OHMS {
                    let (x, y) = (root(&mut uf, a), root(&mut uf, b));
                    uf[x.max(y)] = x.min(y);
                }
            }
            for i in 0..n {
                if root(&mut conn, i) != root(&mut conn, 0) {
                    let c = root(&mut conn, i);
                    conn[c] = root(&mut conn, 0);
                    let (x, y) = (root(&mut uf, i), root(&mut uf, 0));
                    uf[x.max(y)] = x.min(y);
                }
            }
            let mut ids: HashMap<usize, usize> = HashMap::new();
            let node: Vec<usize> = (0..n)
                .map(|i| {
                    let r = root(&mut uf, i);
                    let k = ids.len();
                    *ids.entry(r).or_insert(k)
                })
                .collect();
            let resistors = edges
                .into_iter()
                .filter(|&(_, _, ohms)| ohms >= MIN_OHMS)
                .map(|(a, b, ohms)| (node[a], node[b], ohms))
                .filter(|(a, b, _)| a != b)
                .collect();
            Some(RcNet { segments: segs, node, nodes: ids.len(), resistors })
        })
        .collect()
}

/// Parasitics of a cell's layout, ready to add to its schematic deck.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PostLayout {
    /// Capacitor lines: `Cpar_*` to ground and `Cc_*` between nets.
    pub caps: String,
    /// Junction parameters per MOS SPICE name (upper case), e.g. `ad=1e-12 as=... pd=... ps=...`.
    pub junctions: BTreeMap<String, String>,
    /// Wire resistor lines (`Rpar_*`).
    pub resistors: String,
    /// Drain, gate and source nodes per MOS SPICE name (upper case) moved onto a resistor network.
    pub terminals: BTreeMap<String, [Option<String>; 3]>,
    /// One-line summary.
    pub summary: String,
}

impl PostLayout {
    /// The deck with the MOS lines moved onto their resistor network nodes and given their
    /// junction parameters, and the resistors and capacitors inserted before `.temp` / `.end`
    /// (or at the end).
    pub fn apply(&self, deck: &str) -> String {
        let mut out = String::with_capacity(deck.len() + self.caps.len() + self.resistors.len() + 64 * self.junctions.len());
        let mut inserted = false;
        let block = |out: &mut String| {
            *out += "* --- layout parasitics ---\n";
            *out += &self.resistors;
            *out += &self.caps;
        };
        for line in deck.lines() {
            let t = line.trim().to_ascii_lowercase();
            if !inserted && (t.starts_with(".temp") || t == ".end") {
                block(&mut out);
                inserted = true;
            }
            let element = line.split_whitespace().next().unwrap_or_default().to_ascii_uppercase();
            // PDK subcircuit models put an `X` in front of the schematic's `M` name.
            let first = element.strip_prefix('X').filter(|k| k.starts_with('M')).unwrap_or(&element).to_string();
            let mos = first.starts_with('M');
            match self.terminals.get(&first).filter(|_| mos) {
                Some(nodes) => {
                    let mut toks: Vec<&str> = line.split_whitespace().collect();
                    for (k, n) in nodes.iter().enumerate() {
                        if let (Some(n), Some(t)) = (n, toks.get_mut(k + 1)) {
                            *t = n;
                        }
                    }
                    out += &toks.join(" ");
                }
                None => out += line,
            }
            if let Some(j) = self.junctions.get(&first).filter(|_| mos) {
                out.push(' ');
                out += j;
            }
            out.push('\n');
        }
        if !inserted {
            block(&mut out);
        }
        out
    }
}

/// Extract the cell's layout, match it to its schematic and compute the parasitics to add to the
/// schematic deck. Coupling to a net without a schematic counterpart is lumped to ground.
pub fn post_layout(project: &Project, cell: &CellRef) -> Result<PostLayout, String> {
    const MIN_FARADS: f64 = 1e-18;
    let c = project.cell(cell).ok_or_else(|| format!("no cell '{cell}'"))?;
    let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
    let lay = c.layout.as_ref().ok_or("cell has no layout")?;
    let x = extract(lay, project, &project.tech);
    let rep = compare(sch, &x, &LvsOptions::for_tech(&project.tech));
    let to_sch: HashMap<&str, &str> = rep.nets.iter().map(|(s, l)| (l.as_str(), s.as_str())).collect();

    let mut ground: BTreeMap<String, f64> = net_caps(&x, &project.tech).into_iter().map(|c| (c.net, c.farads)).collect();
    let mut coupled: BTreeMap<(String, String), f64> = BTreeMap::new();
    for cc in coupling_caps(&x, &project.tech) {
        match (to_sch.get(cc.a.as_str()), to_sch.get(cc.b.as_str())) {
            (Some(a), Some(b)) if a != b => {
                let key = if a < b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) };
                *coupled.entry(key).or_default() += cc.farads;
            }
            (Some(_), None) => *ground.entry(cc.a).or_default() += cc.farads,
            (None, Some(_)) => *ground.entry(cc.b).or_default() += cc.farads,
            _ => {}
        }
    }
    let caps: Vec<NetCap> = ground.into_iter().map(|(net, farads)| NetCap { net, farads }).collect();

    // Resistor networks of the matched nets.
    let rc = rc_networks(lay, project, &x, &project.tech);
    let index: HashMap<&str, usize> = x.nets.iter().enumerate().rev().map(|(i, n)| (n.name.as_str(), i)).collect();
    let mut networks: HashMap<&str, (&RcNet, usize)> = HashMap::new();
    let mut resistors = String::new();
    let mut n_res = 0;
    let node_name = |sch: &str, node: usize, port: usize| {
        if node == port { sch.to_string() } else { format!("{}_n{node}", spice_ident(sch)) }
    };
    for (sch, lay_net) in &rep.nets {
        let Some(net) = index.get(lay_net.as_str()).and_then(|&i| rc[i].as_ref()).filter(|n| n.nodes > 1) else { continue };
        let areas = net.node_areas();
        let port = (0..net.nodes).max_by(|&a, &b| areas[a].total_cmp(&areas[b]).then(b.cmp(&a))).unwrap_or(0);
        for (k, &(a, b, ohms)) in net.resistors.iter().enumerate() {
            let (na, nb) = (node_name(sch, a, port), node_name(sch, b, port));
            resistors += &format!("Rpar_{}_{k} {na} {nb} {ohms:.4e}\n", spice_ident(sch));
            n_res += 1;
        }
        networks.insert(sch.as_str(), (net, port));
    }
    let lumped: Vec<NetCap> =
        caps.iter().filter(|c| !networks.contains_key(to_sch.get(c.net.as_str()).copied().unwrap_or(""))).cloned().collect();
    let mut text = annotate(&lumped, &rep.nets, MIN_FARADS);
    let mut grounded: f64 = text.lines().filter_map(|l| l.split_whitespace().nth(3).and_then(crate::units::parse_spice)).sum();
    let mut n_ground = text.lines().count();
    for c in &caps {
        let Some(sch) = to_sch.get(c.net.as_str()).copied().filter(|s| *s != "0") else { continue };
        let Some(&(net, port)) = networks.get(sch).filter(|_| c.farads >= MIN_FARADS) else { continue };
        let areas = net.node_areas();
        let total: f64 = areas.iter().sum();
        for (node, a) in areas.iter().enumerate().filter(|(_, a)| **a > 0.0) {
            let f = c.farads * a / total;
            text += &format!("Cpar_{}_{node} {} 0 {f:.3e}\n", spice_ident(sch), node_name(sch, node, port));
        }
        grounded += c.farads;
        n_ground += 1;
    }

    // MOS terminals onto the network node nearest to their gates.
    let conn = connectivity::extract(sch);
    let gates: HashMap<&str, IRect> = x.devices.iter().map(|d| (d.name.as_str(), d.gate)).collect();
    let mut terminals: BTreeMap<String, [Option<String>; 3]> = BTreeMap::new();
    for (sname, lname) in &rep.devices {
        let centres: Vec<(i64, i64)> =
            lname.split('+').filter_map(|n| gates.get(n)).map(|g| ((g.x0 + g.x1) / 2, (g.y0 + g.y1) / 2)).collect();
        let Some(k) = i64::try_from(centres.len()).ok().filter(|k| *k > 0) else { continue };
        let at = (centres.iter().map(|c| c.0).sum::<i64>() / k, centres.iter().map(|c| c.1).sum::<i64>() / k);
        for inst in sch.instances.iter().filter(|i| sname.split('+').any(|n| n == i.name)) {
            let nodes = ["d", "g", "s"].map(|pin| {
                let net = conn.net_of(inst.id, pin)?;
                let &(rc, port) = networks.get(net)?;
                Some(node_name(net, rc.node_at(at)?, port))
            });
            if nodes.iter().any(Option::is_some) {
                terminals.insert(spice_name(inst).to_ascii_uppercase(), nodes);
            }
        }
    }
    let (mut n_cc, mut cc_total) = (0, 0.0);
    for ((a, b), f) in coupled.into_iter().filter(|(_, f)| *f >= MIN_FARADS) {
        text += &format!("Cc_{}_{} {a} {b} {f:.3e}\n", spice_ident(&a), spice_ident(&b));
        n_cc += 1;
        cc_total += f;
    }
    let junctions =
        junction_params(sch, &x, &rep, project.tech.length_unit(), project.tech.models.as_ref().is_some_and(|m| m.lde));

    let mut summary = format!(
        "post-layout: {n_ground} nets annotated, {:.3} fF to ground, {n_cc} coupling caps ({:.3} fF), junctions on {} devices, {n_res} resistors on {} nets",
        grounded * 1e15,
        cc_total * 1e15,
        junctions.len(),
        networks.len()
    );
    let paired: BTreeSet<&str> = rep.nets.iter().map(|(_, l)| l.as_str()).collect();
    let unmatched: Vec<&str> = caps.iter().map(|c| c.net.as_str()).filter(|n| !paired.contains(n)).collect();
    if !unmatched.is_empty() {
        summary += &format!("; unmatched nets: {}", unmatched.join(", "));
    }
    if rep.matched {
        summary += "; LVS: match";
    } else {
        summary += &format!("; LVS: {} difference(s)", rep.errors.len());
    }
    Ok(PostLayout { caps: text, junctions, resistors, terminals, summary })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Net;
    use crate::layout::Layout;

    fn net(name: &str, layer: usize, rects: &[IRect]) -> Net {
        let r = Region::from_rects(rects.iter().copied());
        Net { name: name.into(), labelled: true, bbox: r.bbox(), shapes: vec![(layer, r)] }
    }

    fn nets(v: Vec<Net>) -> Extraction {
        Extraction { nets: v, dbu_per_um: 1000, ..Default::default() }
    }

    #[test]
    fn single_metal1_rect() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let x = nets(vec![net("a", m1, &[IRect::new(0, 0, 10_000, 1_000)])]);
        let c = tech.layer_cap("metal1").unwrap();
        let expected = (c.area * 10.0 + c.fringe * 22.0) * 1e-18;
        let caps = net_caps(&x, &tech);
        assert_eq!(caps.len(), 1);
        assert!((caps[0].farads - expected).abs() < 1e-24, "{} vs {expected}", caps[0].farads);
        assert_eq!(caps[0].farads, (38.0 * 10.0 + 40.0 * 22.0) * 1e-18);
    }

    #[test]
    fn excluded_layers_and_order() {
        let tech = Tech::generic();
        let (m1, m2, act) =
            (tech.layer_index("metal1").unwrap(), tech.layer_index("metal2").unwrap(), tech.layer_index("active").unwrap());
        let x = nets(vec![
            net("diff", act, &[IRect::new(0, 0, 5000, 5000)]),
            net("small", m2, &[IRect::new(0, 0, 1000, 1000)]),
            net("big", m1, &[IRect::new(0, 0, 9000, 9000)]),
        ]);
        let names: Vec<String> = net_caps(&x, &tech).into_iter().map(|c| c.net).collect();
        assert_eq!(names, ["big", "small"]);
    }

    #[test]
    fn region_perimeter() {
        let r = Region::from_rects([IRect::new(0, 0, 3000, 1000), IRect::new(0, 1000, 1000, 3000)]);
        assert_eq!(r.perimeter(), 12_000);
        let split = Region::from_rects([IRect::new(0, 0, 1000, 1000), IRect::new(2000, 0, 3000, 1000)]);
        assert_eq!(split.perimeter(), 8000);
        assert_eq!(Region::default().perimeter(), 0);
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1e-30)
    }

    #[test]
    fn lateral_coupling() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let k = tech.layer_cap("metal1").unwrap().coupling;
        // Two parallel 10 µm wires 0.5 µm apart, partially overlapping (8 µm run).
        let x = nets(vec![net("a", m1, &[IRect::new(0, 0, 10_000, 300)]), net("b", m1, &[IRect::new(2000, 800, 12_000, 1100)])]);
        let cc = coupling_caps(&x, &tech);
        assert_eq!(cc.len(), 1);
        assert_eq!((cc[0].a.as_str(), cc[0].b.as_str()), ("a", "b"));
        assert!(approx(cc[0].farads, k * 8.0 / 0.5 * 1e-18), "{cc:?}");

        // A third wire in between shields the pair; beyond the range nothing couples.
        let shielded = nets(vec![
            net("a", m1, &[IRect::new(0, 0, 10_000, 300)]),
            net("b", m1, &[IRect::new(0, 1000, 10_000, 1300)]),
            net("c", m1, &[IRect::new(0, 500, 10_000, 800)]),
        ]);
        let pairs: Vec<(String, String)> = coupling_caps(&shielded, &tech).into_iter().map(|c| (c.a, c.b)).collect();
        assert_eq!(pairs, [("a".to_string(), "c".to_string()), ("b".into(), "c".into())]);
        let far = nets(vec![net("a", m1, &[IRect::new(0, 0, 300, 5000)]), net("b", m1, &[IRect::new(1600, 0, 1900, 5000)])]);
        assert!(coupling_caps(&far, &tech).is_empty(), "1.3 µm > 5 x 0.23 µm");
        let same = nets(vec![net("a", m1, &[IRect::new(0, 0, 300, 5000), IRect::new(800, 0, 1100, 5000)])]);
        assert!(coupling_caps(&same, &tech).is_empty());
    }

    #[test]
    fn overlap_coupling() {
        let tech = Tech::generic();
        let (m1, m2, m3) =
            (tech.layer_index("metal1").unwrap(), tech.layer_index("metal2").unwrap(), tech.layer_index("metal3").unwrap());
        let plate = |up: &str, lo: &str| 1.0 / (1.0 / tech.layer_cap(up).unwrap().area - 1.0 / tech.layer_cap(lo).unwrap().area);
        // metal2 crossing metal1: 1 x 2 µm overlap.
        let x = nets(vec![net("a", m1, &[IRect::new(0, 0, 1000, 10_000)]), net("b", m2, &[IRect::new(-5000, 0, 5000, 2000)])]);
        let cc = coupling_caps(&x, &tech);
        assert_eq!(cc.len(), 1);
        assert!(approx(cc[0].farads, plate("metal2", "metal1") * 2.0 * 1e-18), "{cc:?}");

        // metal3 over metal1, half shielded by a metal2 plate of a third net.
        let x = nets(vec![
            net("a", m1, &[IRect::new(0, 0, 2000, 2000)]),
            net("b", m3, &[IRect::new(0, 0, 2000, 2000)]),
            net("c", m2, &[IRect::new(0, 0, 1000, 2000)]),
        ]);
        let get = |a: &str, b: &str| coupling_caps(&x, &tech).into_iter().find(|c| c.a == a && c.b == b).unwrap().farads;
        assert!(approx(get("a", "b"), plate("metal3", "metal1") * 2.0 * 1e-18));
        assert!(approx(get("a", "c"), plate("metal2", "metal1") * 2.0 * 1e-18));
        assert!(approx(get("b", "c"), plate("metal3", "metal2") * 2.0 * 1e-18));
    }

    #[test]
    fn annotate_format_ground_and_threshold() {
        let caps = [
            NetCap { net: "n_out".into(), farads: 1.234e-16 },
            NetCap { net: "gnd".into(), farads: 5e-16 },
            NetCap { net: "tiny".into(), farads: 5e-19 },
            NetCap { net: "x".into(), farads: 2e-17 },
        ];
        let pairs = [
            ("out".to_string(), "n_out".to_string()),
            ("0".into(), "gnd".into()),
            ("t".into(), "tiny".into()),
            ("a<1>".into(), "x".into()),
            ("nocap".into(), "missing".into()),
        ];
        let s = annotate(&caps, &pairs, 1e-18);
        assert_eq!(s, "Cpar_a_1_ a<1> 0 2.000e-17\nCpar_out out 0 1.234e-16\n");
        assert_eq!(annotate(&caps, &pairs, 1e-16).lines().count(), 1);
    }

    #[test]
    fn junctions_of_single_and_shared_diffusion() {
        let tech = Tech::generic();
        let l = |n: &str| tech.layer_index(n).unwrap();
        // Two NMOS gates on one 4 µm x 1 µm active: three diffusion strips, the middle shared.
        let mut lay = Layout::default();
        lay.add_rect(l("nplus"), -500, -500, 4500, 1500);
        lay.add_rect(l("active"), 0, 0, 4000, 1000);
        for x0 in [1000, 2800] {
            lay.add_rect(l("poly"), x0, -300, x0 + 200, 1300);
        }
        let x = extract(&lay, &crate::hier::NoCells, &tech);
        assert_eq!(x.devices.len(), 2);
        let (left, right) = (&x.devices[0], &x.devices[1]);
        // Outer strips: 1 x 1 µm (left) and 1 x 1 µm (right); perimeter 3 µm without the gate edge.
        // Middle strip 1.6 x 1 µm with two gate edges: perimeter 3.2 µm, split in two.
        let outer = Junction { area: 1e6, perimeter: 3000.0 };
        let shared = Junction { area: 0.8e6, perimeter: 1600.0 };
        let mut got = [left.js, left.jd, right.js, right.jd];
        got.sort_by(|a, b| a.area.total_cmp(&b.area));
        assert_eq!(got, [shared, shared, outer, outer]);
        assert!([left.s, left.d].iter().any(|n| [right.s, right.d].contains(n)), "the middle strip is shared");
    }

    /// NMOS gates `[x0, x1)` across the active `[0, len) x [0, 1000)`, and one schematic NMOS
    /// whose drain and source are the layout pieces at `d` / `s`, matched to all of them.
    fn lde_case(len: i64, gates: &[(i64, i64)], d: (i64, i64), s: (i64, i64), lde: bool, unit: LengthUnit) -> String {
        use crate::geom::{Orient, Pt};
        let tech = Tech::generic();
        let l = |n: &str| tech.layer_index(n).unwrap();
        let mut lay = Layout::default();
        lay.add_rect(l("nplus"), -500, -500, len + 500, 1500);
        lay.add_rect(l("active"), 0, 0, len, 1000);
        for &(x0, x1) in gates {
            lay.add_rect(l("poly"), x0, -300, x1, 1300);
        }
        let x = extract(&lay, &crate::hier::NoCells, &tech);
        assert_eq!(x.devices.len(), gates.len());
        let mut sch = Schematic::default();
        let id = sch.add_instance(DeviceKind::Nmos, Pt::new(0, 0), Orient::default());
        let pins = sch.instance(id).unwrap().pin_positions();
        for (pin, net) in [("d", "d"), ("g", "g"), ("s", "s")] {
            sch.add_label(net, pins.iter().find(|(n, _)| *n == pin).unwrap().1);
        }
        let at = |p: (i64, i64)| x.nets[x.net_at(l("active"), p).unwrap()].name.clone();
        let rep = LvsReport {
            devices: vec![(
                sch.instances[0].name.clone(),
                x.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join("+"),
            )],
            nets: vec![("d".into(), at(d)), ("s".into(), at(s))],
            ..LvsReport::default()
        };
        junction_params(&sch, &x, &rep, unit, lde).remove("M1").unwrap()
    }

    fn lde_part(p: &str) -> &str {
        p.find(" nf=").map_or("", |i| &p[i..])
    }

    #[test]
    fn lde_single_device() {
        // Gate 1.0-1.2 µm on a 3 µm active: 1 µm of diffusion on the left, 1.8 µm on the right.
        let p = lde_case(3000, &[(1000, 1200)], (2500, 500), (500, 500), true, LengthUnit::M);
        assert_eq!(lde_part(&p), " nf=1 sa=1.0000e-6 sb=1.8000e-6", "{p}");
        // Drain on the left: sa stays on the source side.
        let p = lde_case(3000, &[(1000, 1200)], (500, 500), (2500, 500), true, LengthUnit::Um);
        assert_eq!(lde_part(&p), " nf=1 sa=1.8000e0 sb=1.0000e0", "{p}");
    }

    #[test]
    fn lde_two_fingers() {
        // Fingers 1.0-1.2 and 2.8-3.0 µm on a 4.5 µm active: outer 1 / 1.5 µm, 1.6 µm between.
        let p = lde_case(4500, &[(1000, 1200), (2800, 3000)], (2000, 500), (500, 500), true, LengthUnit::M);
        assert_eq!(lde_part(&p), " nf=2 sa=1.0000e-6 sb=1.5000e-6 sd=1.6000e-6", "{p}");
        // Uneven spacing: nf only.
        let p = lde_case(5000, &[(1000, 1200), (2000, 2200), (3800, 4000)], (1500, 500), (500, 500), true, LengthUnit::M);
        assert_eq!(lde_part(&p), " nf=3", "{p}");
    }

    #[test]
    fn lde_off_without_pdk_models() {
        let on = lde_case(4500, &[(1000, 1200), (2800, 3000)], (2000, 500), (500, 500), true, LengthUnit::M);
        let off = lde_case(4500, &[(1000, 1200), (2800, 3000)], (2000, 500), (500, 500), false, LengthUnit::M);
        assert_eq!(off.split_whitespace().count(), 4, "{off}");
        assert!(off.starts_with("ad=") && !off.contains("nf="), "{off}");
        assert_eq!(on, format!("{off}{}", lde_part(&on)));
        // The demo technology has no PDK models: post-layout junctions stay `ad as pd ps`.
        let p = crate::examples::demo_project();
        assert!(p.tech.models.is_none());
        let pl = post_layout(&p, &CellRef::new("work", "cmos_inverter")).unwrap();
        assert!(pl.junctions.values().all(|j| j.split_whitespace().count() == 4), "{:?}", pl.junctions);
    }

    #[test]
    fn lde_needs_rectangular_diffusion() {
        let tech = Tech::generic();
        let l = |n: &str| tech.layer_index(n).unwrap();
        let device = |arm: bool| {
            let mut lay = Layout::default();
            lay.add_rect(l("nplus"), -500, -500, 3500, 3500);
            lay.add_rect(l("active"), 0, 0, 3000, 1000);
            if arm {
                lay.add_rect(l("active"), 2000, 1000, 3000, 3000);
            }
            lay.add_rect(l("poly"), 1000, -300, 1200, 1300);
            extract(&lay, &crate::hier::NoCells, &tech).devices.remove(0)
        };
        assert_eq!(device(false).channel(), Some((true, (1000, 1200), (0, 3000))));
        let bent = device(true);
        assert_eq!((bent.od, bent.channel()), (None, None));
    }

    #[test]
    fn apply_to_deck() {
        let pl = PostLayout {
            caps: "Cpar_out out 0 1e-15\n".into(),
            junctions: [("M1".to_string(), "ad=1e-12 as=2e-12 pd=4e-6 ps=6e-6".to_string())].into(),
            resistors: "Rpar_out_0 out out_n1 2\n".into(),
            terminals: [("M1".to_string(), [Some("out_n1".to_string()), None, None])].into(),
            summary: String::new(),
        };
        let deck =
            "* t\nM1 out in 0 0 nmos w=1u l=1u\nm2 a b c d pmos w=1u l=1u\n.model nmos nmos (level=1)\n.temp 27\n.op\n.end\n";
        assert_eq!(
            pl.apply(deck),
            "* t\nM1 out_n1 in 0 0 nmos w=1u l=1u ad=1e-12 as=2e-12 pd=4e-6 ps=6e-6\nm2 a b c d pmos w=1u l=1u\n\
             .model nmos nmos (level=1)\n* --- layout parasitics ---\nRpar_out_0 out out_n1 2\nCpar_out out 0 1e-15\n\
             .temp 27\n.op\n.end\n"
        );
        assert!(
            pl.apply("* t\nR1 a 0 1\n").ends_with("* --- layout parasitics ---\nRpar_out_0 out out_n1 2\nCpar_out out 0 1e-15\n")
        );
    }

    #[test]
    fn wire_resistance() {
        let tech = Tech::generic();
        let (m1, m2, via1) =
            (tech.layer_index("metal1").unwrap(), tech.layer_index("metal2").unwrap(), tech.layer_index("via1").unwrap());
        let rs = tech.layer_cap("metal1").unwrap().resistance;
        // A 10 x 0.3 µm wire: four 2.5 µm segments in series.
        let x = nets(vec![net("a", m1, &[IRect::new(0, 0, 10_000, 300)])]);
        let rc = rc_networks(&Layout::default(), &crate::hier::NoCells, &x, &tech).remove(0).unwrap();
        assert_eq!((rc.segments.len(), rc.nodes, rc.resistors.len()), (4, 4, 3));
        let total: f64 = rc.resistors.iter().map(|r| r.2).sum();
        assert!(approx(total, rs * 7.5 / 0.3), "{total}");
        assert_ne!(rc.node_at((0, 150)), rc.node_at((10_000, 150)));

        // metal1 and metal2 joined by one via1 cut.
        let mut x = nets(vec![net("a", m1, &[IRect::new(0, 0, 1000, 300)])]);
        x.nets[0].shapes.push((m2, Region::from_rects([IRect::new(700, 0, 3000, 300)])));
        x.nets[0].bbox = Some(IRect::new(0, 0, 3000, 300));
        let mut lay = Layout::default();
        lay.add_rect(via1, 720, 20, 980, 280);
        let rc = rc_networks(&lay, &crate::hier::NoCells, &x, &tech).remove(0).unwrap();
        assert!(rc.resistors.iter().any(|r| r.2 == tech.via("via1").unwrap().resistance), "{rc:?}");
        assert_ne!(rc.node_at((100, 150)), rc.node_at((2900, 150)));

        // Pieces joined only off the network collapse onto one node; no sheet resistance, no network.
        let x = nets(vec![net("a", m1, &[IRect::new(0, 0, 300, 300), IRect::new(1000, 0, 1300, 300)])]);
        assert_eq!(rc_networks(&Layout::default(), &crate::hier::NoCells, &x, &tech)[0].as_ref().unwrap().nodes, 1);
        let mut plain = tech.clone();
        plain.caps.iter_mut().for_each(|c| c.resistance = 0.0);
        assert!(rc_networks(&Layout::default(), &crate::hier::NoCells, &x, &plain)[0].is_none());
    }

    #[test]
    fn demo_post_layout() {
        let p = crate::examples::demo_project();
        let pl = post_layout(&p, &CellRef::new("work", "cmos_inverter")).unwrap();
        let mut nets: Vec<&str> = pl
            .caps
            .lines()
            .filter(|l| l.starts_with("Cpar"))
            .map(|l| l.split_whitespace().nth(1).unwrap())
            .map(|n| n.rsplit_once("_n").map_or(n, |(a, _)| a))
            .collect();
        nets.dedup();
        assert_eq!(nets, ["in", "out", "vdd"], "{}", pl.caps);
        assert!(pl.summary.starts_with("post-layout: 3 nets annotated"), "{}", pl.summary);
        assert!(pl.summary.contains("junctions on 2 devices"), "{}", pl.summary);
        assert!(pl.summary.contains("LVS:"), "{}", pl.summary);
        assert_eq!(pl.junctions.len(), 2);
        for j in pl.junctions.values() {
            let v: Vec<f64> = j.split_whitespace().map(|kv| kv.split_once('=').unwrap().1.parse().unwrap()).collect();
            assert!(v.iter().all(|v| *v > 0.0), "{j}");
        }
        assert!(post_layout(&p, &CellRef::new("work", "inv_chain")).is_err());

        // The ring oscillator slows down once the layout parasitics are in the deck.
        let r = CellRef::new("work", "ring_osc5");
        let pl = post_layout(&p, &r).unwrap();
        assert!(pl.summary.contains("LVS: match"), "{}", pl.summary);
        assert!(pl.caps.contains("Cc_"), "{}", pl.caps);
        assert_eq!(pl.junctions.len(), 10, "{:?}", pl.junctions);
        let c = p.cell(&r).unwrap();
        let deck = crate::netlist::netlist_for(c.schematic.as_ref().unwrap(), &p.tech, "").deck("ring", &c.sim);
        let post = pl.apply(&deck);
        assert_eq!(post.lines().filter(|l| l.contains(" ad=")).count(), 10, "{post}");
        assert!(pl.summary.contains(" resistors on "), "{}", pl.summary);
        assert!(!pl.resistors.is_empty() && !pl.terminals.is_empty(), "{pl:?}");
        let moved = post.lines().filter(|l| l.starts_with('M') && l.split_whitespace().take(4).any(|t| t.contains("_n"))).count();
        assert!(moved > 0, "{post}");
        // Every network node reaches its net's named node through resistors.
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for l in pl.resistors.lines() {
            let t: Vec<&str> = l.split_whitespace().collect();
            adj.entry(t[1].into()).or_default().push(t[2].into());
            adj.entry(t[2].into()).or_default().push(t[1].into());
        }
        for node in adj.keys().filter(|n| n.contains("_n")) {
            let named = node.rsplit_once("_n").unwrap().0;
            let (mut seen, mut stack) = (BTreeSet::new(), vec![node.clone()]);
            while let Some(n) = stack.pop() {
                if seen.insert(n.clone()) {
                    stack.extend(adj[&n].iter().cloned());
                }
            }
            assert!(seen.contains(named), "{node} is not connected to {named}");
        }
    }
}
