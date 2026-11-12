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

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rstar::{AABB, RTree, RTreeObject};

use crate::connectivity;
use crate::extract::{Extraction, Junction, extract};
use crate::geom::IRect;
use crate::lvs::{LvsOptions, LvsReport, compare};
use crate::netlist::spice_name;
use crate::project::{CellRef, Project};
use crate::region::{BoolOp, Region};
use crate::schematic::{DeviceKind, Schematic};
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
/// devices. Merged parallel devices share their junctions in proportion to `m`.
pub fn junction_params(sch: &Schematic, x: &Extraction, rep: &LvsReport) -> BTreeMap<String, String> {
    let conn = connectivity::extract(sch);
    let insts: HashMap<&str, _> = sch
        .instances
        .iter()
        .filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos))
        .map(|i| (i.name.as_str(), i))
        .collect();
    let devs: HashMap<&str, _> = x.devices.iter().map(|d| (d.name.as_str(), d)).collect();
    let to_layout: HashMap<&str, &str> = rep.nets.iter().map(|(s, l)| (s.as_str(), l.as_str())).collect();
    let (um, m2) = (x.dbu_per_um.max(1) as f64, 1e-12 / (x.dbu_per_um.max(1) as f64).powi(2));
    let mut out = BTreeMap::new();
    for (sname, lname) in &rep.devices {
        let mut total: HashMap<&str, Junction> = HashMap::new();
        for d in lname.split('+').filter_map(|n| devs.get(n)) {
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
            out.insert(
                spice_name(inst).to_ascii_uppercase(),
                format!(
                    "ad={:.4e} as={:.4e} pd={:.4e} ps={:.4e}",
                    jd.area * k * m2,
                    js.area * k * m2,
                    jd.perimeter * k / um * 1e-6,
                    js.perimeter * k / um * 1e-6
                ),
            );
        }
    }
    out
}

/// Parasitics of a cell's layout, ready to add to its schematic deck.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PostLayout {
    /// Capacitor lines: `Cpar_*` to ground and `Cc_*` between nets.
    pub caps: String,
    /// Junction parameters per MOS SPICE name (upper case), e.g. `ad=1e-12 as=... pd=... ps=...`.
    pub junctions: BTreeMap<String, String>,
    /// One-line summary.
    pub summary: String,
}

impl PostLayout {
    /// The deck with the junction parameters appended to its MOS lines and the capacitors
    /// inserted before `.temp` / `.end` (or at the end).
    pub fn apply(&self, deck: &str) -> String {
        let mut out = String::with_capacity(deck.len() + self.caps.len() + 64 * self.junctions.len());
        let mut inserted = false;
        for line in deck.lines() {
            let t = line.trim().to_ascii_lowercase();
            if !inserted && (t.starts_with(".temp") || t == ".end") {
                out += "* --- layout parasitics ---\n";
                out += &self.caps;
                inserted = true;
            }
            out += line;
            let first = line.split_whitespace().next().unwrap_or_default().to_ascii_uppercase();
            if let Some(j) = self.junctions.get(&first).filter(|_| first.starts_with('M')) {
                out.push(' ');
                out += j;
            }
            out.push('\n');
        }
        if !inserted {
            out += "* --- layout parasitics ---\n";
            out += &self.caps;
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
    let rep = compare(sch, &x, &LvsOptions::default());
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
    let mut text = annotate(&caps, &rep.nets, MIN_FARADS);
    let grounded: f64 = text.lines().filter_map(|l| l.split_whitespace().nth(3).and_then(crate::units::parse_spice)).sum();
    let n_ground = text.lines().count();
    let (mut n_cc, mut cc_total) = (0, 0.0);
    for ((a, b), f) in coupled.into_iter().filter(|(_, f)| *f >= MIN_FARADS) {
        text += &format!("Cc_{}_{} {a} {b} {f:.3e}\n", spice_ident(&a), spice_ident(&b));
        n_cc += 1;
        cc_total += f;
    }
    let junctions = junction_params(sch, &x, &rep);

    let mut summary = format!(
        "post-layout: {n_ground} nets annotated, {:.3} fF to ground, {n_cc} coupling caps ({:.3} fF), junctions on {} devices",
        grounded * 1e15,
        cc_total * 1e15,
        junctions.len()
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
    Ok(PostLayout { caps: text, junctions, summary })
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

    #[test]
    fn apply_to_deck() {
        let pl = PostLayout {
            caps: "Cpar_out out 0 1e-15\n".into(),
            junctions: [("M1".to_string(), "ad=1e-12 as=2e-12 pd=4e-6 ps=6e-6".to_string())].into(),
            summary: String::new(),
        };
        let deck =
            "* t\nM1 out in 0 0 nmos w=1u l=1u\nm2 a b c d pmos w=1u l=1u\n.model nmos nmos (level=1)\n.temp 27\n.op\n.end\n";
        assert_eq!(
            pl.apply(deck),
            "* t\nM1 out in 0 0 nmos w=1u l=1u ad=1e-12 as=2e-12 pd=4e-6 ps=6e-6\nm2 a b c d pmos w=1u l=1u\n\
             .model nmos nmos (level=1)\n* --- layout parasitics ---\nCpar_out out 0 1e-15\n.temp 27\n.op\n.end\n"
        );
        assert!(pl.apply("* t\nR1 a 0 1\n").ends_with("* --- layout parasitics ---\nCpar_out out 0 1e-15\n"));
    }

    #[test]
    fn demo_post_layout() {
        let p = crate::examples::demo_project();
        let pl = post_layout(&p, &CellRef::new("work", "cmos_inverter")).unwrap();
        let nets: Vec<&str> =
            pl.caps.lines().filter(|l| l.starts_with("Cpar")).map(|l| l.split_whitespace().nth(1).unwrap()).collect();
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
        let deck = crate::netlist::netlist(c.schematic.as_ref().unwrap()).deck("ring", &c.sim);
        let post = pl.apply(&deck);
        assert_eq!(post.lines().filter(|l| l.contains(" ad=")).count(), 10, "{post}");
    }
}
