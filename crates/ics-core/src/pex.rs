//! Simple parasitic extraction: wire capacitance to substrate per net from area and fringe
//! coefficients, and its annotation onto the schematic netlist for post-layout simulation.

use std::collections::{BTreeMap, BTreeSet};

use crate::extract::{Extraction, extract};
use crate::lvs::{LvsOptions, compare};
use crate::project::{CellRef, Project};
use crate::region::Region;
use crate::tech::Tech;

#[derive(Debug, Clone, PartialEq)]
pub struct NetCap {
    /// Layout net name.
    pub net: String,
    pub farads: f64,
}

/// Capacitance of every extracted net (largest first).
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
                .filter_map(|(l, r)| coef.get(l).map(|&(a, f)| a * r.area() as f64 / (um * um) + f * perimeter(r) as f64 / um))
                .sum();
            NetCap { net: n.name.clone(), farads: af * 1e-18 }
        })
        .filter(|c| c.farads > 0.0)
        .collect();
    caps.sort_by(|a, b| b.farads.total_cmp(&a.farads).then_with(|| a.net.cmp(&b.net)));
    caps
}

/// Boundary length of a region: rect edges not shared with an abutting rect.
pub fn perimeter(r: &Region) -> i64 {
    let mut horiz: BTreeMap<i64, [Vec<(i64, i64)>; 2]> = BTreeMap::new();
    let mut vert: BTreeMap<i64, [Vec<(i64, i64)>; 2]> = BTreeMap::new();
    for q in r.rects() {
        horiz.entry(q.y0).or_default()[0].push((q.x0, q.x1));
        horiz.entry(q.y1).or_default()[1].push((q.x0, q.x1));
        vert.entry(q.x0).or_default()[0].push((q.y0, q.y1));
        vert.entry(q.x1).or_default()[1].push((q.y0, q.y1));
    }
    horiz.into_values().chain(vert.into_values()).map(|[mut lo, mut hi]| exposed(&mut lo, &mut hi)).sum()
}

/// Length of the symmetric difference of two sets of disjoint intervals on one line.
fn exposed(a: &mut [(i64, i64)], b: &mut [(i64, i64)]) -> i64 {
    a.sort_unstable();
    b.sort_unstable();
    let len = |v: &[(i64, i64)]| v.iter().map(|(s, e)| e - s).sum::<i64>();
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        shared += (a[i].1.min(b[j].1) - a[i].0.max(b[j].0)).max(0);
        if a[i].1 < b[j].1 { i += 1 } else { j += 1 }
    }
    len(a) + len(b) - 2 * shared
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
        let name: String = sch.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
        lines.entry(format!("Cpar_{name}")).or_insert_with(|| format!("Cpar_{name} {sch} 0 {f:.3e}\n"));
    }
    lines.into_values().collect()
}

/// Extract the cell's layout, match it to its schematic and return the parasitic lines to add
/// to the schematic deck, plus a one-line summary.
pub fn post_layout_lines(project: &Project, cell: &CellRef) -> Result<(String, String), String> {
    let c = project.cell(cell).ok_or_else(|| format!("no cell '{cell}'"))?;
    let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
    let lay = c.layout.as_ref().ok_or("cell has no layout")?;
    let x = extract(lay, project, &project.tech);
    let rep = compare(sch, &x, &LvsOptions::default());
    let caps = net_caps(&x, &project.tech);
    let text = annotate(&caps, &rep.nets, 1e-18);
    let n = text.lines().count();
    let total: f64 = text.lines().filter_map(|l| l.split_whitespace().nth(3).and_then(crate::units::parse_spice)).sum();
    let mut summary = format!("post-layout: {n} nets annotated, {:.3} fF total", total * 1e15);
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
    Ok((text, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Net;
    use crate::geom::IRect;

    fn net(name: &str, layer: usize, rects: &[IRect]) -> Net {
        let r = Region::from_rects(rects.iter().copied());
        Net { name: name.into(), labelled: true, bbox: r.bbox(), shapes: vec![(layer, r)] }
    }

    #[test]
    fn single_metal1_rect() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let x =
            Extraction { nets: vec![net("a", m1, &[IRect::new(0, 0, 10_000, 1_000)])], dbu_per_um: 1000, ..Default::default() };
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
        let x = Extraction {
            nets: vec![
                net("diff", act, &[IRect::new(0, 0, 5000, 5000)]),
                net("small", m2, &[IRect::new(0, 0, 1000, 1000)]),
                net("big", m1, &[IRect::new(0, 0, 9000, 9000)]),
            ],
            dbu_per_um: 1000,
            ..Default::default()
        };
        let names: Vec<String> = net_caps(&x, &tech).into_iter().map(|c| c.net).collect();
        assert_eq!(names, ["big", "small"]);
    }

    #[test]
    fn l_shape_perimeter() {
        let r = Region::from_rects([IRect::new(0, 0, 3000, 1000), IRect::new(0, 1000, 1000, 3000)]);
        assert_eq!(perimeter(&r), 12_000);
        let split = Region::from_rects([IRect::new(0, 0, 1000, 1000), IRect::new(2000, 0, 3000, 1000)]);
        assert_eq!(perimeter(&split), 8000);
        assert_eq!(perimeter(&Region::default()), 0);
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
        for l in s.lines() {
            let v = crate::units::parse_spice(l.split_whitespace().nth(3).unwrap()).unwrap();
            assert!(v > 1e-18);
        }
        assert_eq!(annotate(&caps, &pairs, 1e-16).lines().count(), 1);
    }

    #[test]
    fn demo_inverter_post_layout() {
        let p = crate::examples::demo_project();
        let (text, summary) = post_layout_lines(&p, &CellRef::new("work", "cmos_inverter")).unwrap();
        let nets: Vec<&str> = text.lines().map(|l| l.split_whitespace().nth(1).unwrap()).collect();
        assert_eq!(nets, ["in", "out", "vdd"], "{text}");
        assert!(summary.starts_with("post-layout: 3 nets annotated"), "{summary}");
        assert!(summary.contains("LVS:"), "{summary}");
        assert!(post_layout_lines(&p, &CellRef::new("work", "inv_chain")).is_err());
    }
}
