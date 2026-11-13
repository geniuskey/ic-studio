//! Layout by example: reuse the placement of a finished cell for another cell with the same
//! circuit topology. The two schematics are matched device for device and net for net by
//! colour refinement (as in LVS), then every MOS device of the target is placed where its
//! counterpart sits in the reference layout (units of matched devices included), sized from the
//! target's own W / L, together with the reference's well / implant geometry and taps. Wires are
//! not copied: route the result (net names may differ).

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::connectivity;
use crate::layout::{Geometry, Layout};
use crate::pcell::PCell;
use crate::project::{CellRef, Project};
use crate::schematic::{DeviceKind, Schematic};
use crate::sdl::{DUMMY, NTAP, PTAP, device_of, mos_pcell};

/// Device and net correspondence between two schematics (reference name, target name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Correspondence {
    pub devices: Vec<(String, String)>,
    pub nets: Vec<(String, String)>,
}

struct Graph {
    devs: Vec<(String, DeviceKind)>,
    /// Per device: (pin role, net index).
    pins: Vec<Vec<(&'static str, usize)>>,
    nets: Vec<String>,
    /// Per net: (pin role, device index).
    users: Vec<Vec<(&'static str, usize)>>,
    /// Nets tied to a ground or supply symbol keep that identity.
    rail: Vec<Option<String>>,
}

fn role(kind: DeviceKind, pin: &'static str) -> &'static str {
    match (kind, pin) {
        (DeviceKind::Nmos | DeviceKind::Pmos, "d" | "s") => "sd",
        _ => pin,
    }
}

fn graph(sch: &Schematic) -> Graph {
    let conn = connectivity::extract(sch);
    let nets = conn.nets();
    let index: HashMap<&str, usize> = nets.iter().enumerate().map(|(k, n)| (n.as_str(), k)).collect();
    let mut g =
        Graph { devs: vec![], pins: vec![], nets: nets.clone(), users: vec![vec![]; nets.len()], rail: vec![None; nets.len()] };
    for inst in &sch.instances {
        let pins: Vec<(&'static str, usize)> = inst
            .pin_positions()
            .into_iter()
            .filter_map(|(p, _)| conn.net_of(inst.id, p).and_then(|n| index.get(n)).map(|&k| (p, k)))
            .collect();
        match inst.kind {
            DeviceKind::Ground | DeviceKind::Supply => {
                let tag =
                    if inst.kind == DeviceKind::Ground { "ground".to_string() } else { format!("supply:{}", inst.param("net")) };
                for (_, k) in pins {
                    g.rail[k] = Some(tag.clone());
                }
            }
            kind => {
                let d = g.devs.len();
                g.devs.push((inst.name.clone(), kind));
                let pins: Vec<_> = pins.into_iter().map(|(p, k)| (role(kind, p), k)).collect();
                for &(r, k) in &pins {
                    g.users[k].push((r, d));
                }
                g.pins.push(pins);
            }
        }
    }
    g
}

fn h(v: impl Hash) -> u64 {
    let mut s = DefaultHasher::new();
    v.hash(&mut s);
    s.finish()
}

/// One refinement round: device colours from their pins' net colours, then net colours from
/// their users' device colours.
fn refine(g: &Graph, dc: &mut [u64], nc: &mut [u64]) {
    let new_d: Vec<u64> = (0..g.devs.len())
        .map(|d| {
            let mut sig: Vec<(&str, u64)> = g.pins[d].iter().map(|&(r, n)| (r, nc[n])).collect();
            sig.sort();
            h((dc[d], sig))
        })
        .collect();
    dc.copy_from_slice(&new_d);
    let new_n: Vec<u64> = (0..g.nets.len())
        .map(|n| {
            let mut sig: Vec<(&str, u64)> = g.users[n].iter().map(|&(r, d)| (r, dc[d])).collect();
            sig.sort();
            h((nc[n], sig))
        })
        .collect();
    nc.copy_from_slice(&new_n);
}

/// Match the devices and nets of `reference` and `target`, which must have the same topology
/// (device kinds and connections; parameter values and names may differ). Symmetric devices
/// (e.g. the two halves of a differential pair) are paired in name order.
pub fn correspond(reference: &Schematic, target: &Schematic) -> Result<Correspondence, String> {
    let (a, b) = (graph(reference), graph(target));
    if a.devs.len() != b.devs.len() || a.nets.len() != b.nets.len() {
        return Err(format!(
            "different circuits: {} devices / {} nets in the reference, {} / {} in the target",
            a.devs.len(),
            a.nets.len(),
            b.devs.len(),
            b.nets.len()
        ));
    }
    let init = |g: &Graph| -> (Vec<u64>, Vec<u64>) {
        (
            g.devs.iter().map(|(_, k)| h(format!("{k:?}"))).collect(),
            (0..g.nets.len()).map(|n| h((&g.rail[n], g.users[n].len()))).collect(),
        )
    };
    let (mut da, mut na) = init(&a);
    let (mut db, mut nb) = init(&b);
    let rounds = 2 * (a.devs.len() + a.nets.len()).max(2);
    let mut seed = 1u64;
    loop {
        for _ in 0..rounds.min(64) {
            refine(&a, &mut da, &mut na);
            refine(&b, &mut db, &mut nb);
        }
        let mut classes: BTreeMap<u64, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
        for (d, c) in da.iter().enumerate() {
            classes.entry(*c).or_default().0.push(d);
        }
        for (d, c) in db.iter().enumerate() {
            classes.entry(*c).or_default().1.push(d);
        }
        if let Some((_, (x, y))) = classes.iter().find(|(_, (x, y))| x.len() != y.len()) {
            let name = |g: &Graph, v: &[usize]| v.iter().map(|&d| g.devs[d].0.clone()).collect::<Vec<_>>().join(", ");
            return Err(format!("no counterpart: reference [{}] vs target [{}]", name(&a, x), name(&b, y)));
        }
        // Break the first tie: give one device on each side a fresh colour.
        let tie = classes.values().find(|(x, _)| x.len() > 1);
        let Some((x, y)) = tie else { break };
        let pick = |g: &Graph, v: &[usize]| *v.iter().min_by_key(|&&d| natural(&g.devs[d].0)).expect("non-empty");
        let (pa, pb) = (pick(&a, x), pick(&b, y));
        seed += 1;
        da[pa] = h((da[pa], "tie", seed));
        db[pb] = h((db[pb], "tie", seed));
    }
    let by = |c: &[u64]| c.iter().enumerate().map(|(k, c)| (*c, k)).collect::<HashMap<u64, usize>>();
    let (mb, nbm) = (by(&db), by(&nb));
    let devices = da.iter().enumerate().map(|(d, c)| (a.devs[d].0.clone(), b.devs[mb[c]].0.clone())).collect();
    let nets = na.iter().enumerate().filter_map(|(n, c)| Some((a.nets[n].clone(), b.nets[*nbm.get(c)?].clone()))).collect();
    Ok(Correspondence { devices, nets })
}

/// Sort key that orders `M2` before `M10`.
fn natural(s: &str) -> (String, u64, String) {
    let head: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &s[head.len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (head, num.parse().unwrap_or(0), rest[num.len()..].to_string())
}

#[derive(Debug, Clone, Default)]
pub struct ByExampleReport {
    pub placed: usize,
    pub taps: usize,
    pub shapes: usize,
    pub notes: Vec<String>,
}

/// Place the target cell's devices like the reference cell's layout (see the module doc). The
/// target's layout is replaced; it is created if missing.
pub fn place_like(project: &mut Project, reference: &CellRef, target: &CellRef) -> Result<ByExampleReport, String> {
    let rc = project.cell(reference).ok_or("no reference cell")?;
    let rsch = rc.schematic.clone().ok_or("the reference cell has no schematic")?;
    let rlay = rc.layout.clone().ok_or("the reference cell has no layout")?;
    let tsch = project.cell(target).and_then(|c| c.schematic.clone()).ok_or("the target cell has no schematic")?;
    let corr = correspond(&rsch, &tsch)?;
    let dev_map: HashMap<&str, &str> = corr.devices.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let net_map: HashMap<&str, &str> = corr.nets.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let tech = project.tech.clone();
    let mut out = Layout::default();
    let mut rep = ByExampleReport::default();
    // Reference placements per schematic device.
    let mut units: BTreeMap<&str, Vec<&crate::layout::CellInst>> = BTreeMap::new();
    for inst in &rlay.instances {
        if let Some(n) = inst.name.as_deref() {
            units.entry(device_of(n)).or_default().push(inst);
        }
    }
    for (rdev, insts) in &units {
        let Some(&tdev) = dev_map.get(rdev) else { continue };
        let Some(ti) = tsch.instances.iter().find(|i| i.name == tdev) else { continue };
        if !matches!(ti.kind, DeviceKind::Nmos | DeviceKind::Pmos) {
            continue;
        }
        let mut pc = mos_pcell(ti, &tech)?;
        let k = insts.len() as u32;
        if k > 1
            && let PCell::Mos { w, fingers, .. } = &mut pc
        {
            if *fingers % k == 0 {
                *fingers /= k;
            } else if *fingers == 1 {
                *w = (*w / k as i64 / tech.grid.max(1)) * tech.grid.max(1);
            } else {
                rep.notes.push(format!("{tdev}: {fingers} finger(s) do not split into {k} units; unit size kept"));
            }
        }
        let cell = project.ensure_pcell(&pc)?;
        let mut sorted = insts.clone();
        sorted.sort_by_key(|i| natural(i.name.as_deref().unwrap_or_default()));
        for (u, inst) in sorted.iter().enumerate() {
            let id = out.add_instance(cell.clone(), inst.trans, inst.array);
            let name = if k > 1 { format!("{tdev}.{}", u + 1) } else { tdev.to_string() };
            if let Some(i) = out.instance_mut(id) {
                i.name = Some(name);
            }
            rep.placed += 1;
        }
    }
    // Taps and dummies follow their nets.
    let map_net = |n: &str| net_map.get(n).map_or(n.to_string(), |m| m.to_string());
    for inst in &rlay.instances {
        let Some(n) = inst.name.as_deref() else { continue };
        let renamed = if let Some(net) = n.strip_prefix(NTAP) {
            format!("{NTAP}{}", map_net(net))
        } else if let Some(net) = n.strip_prefix(PTAP) {
            format!("{PTAP}{}", map_net(net))
        } else if n.starts_with(DUMMY) {
            match n.rsplit_once(':') {
                Some((head, bulk)) if !bulk.is_empty() => format!("{head}:{}", map_net(bulk)),
                _ => n.to_string(),
            }
        } else {
            continue;
        };
        let id = out.add_instance(inst.cell.clone(), inst.trans, inst.array);
        if let Some(i) = out.instance_mut(id) {
            i.name = Some(renamed);
        }
        rep.taps += 1;
    }
    // Well and implant geometry: non-routing layers that are no via cut.
    let cuts: Vec<usize> = tech.vias.iter().filter_map(|v| tech.layer_index(&v.cut)).collect();
    for s in &rlay.shapes {
        let routing = tech.layers.get(s.layer).is_none_or(|d| d.routing);
        if routing || cuts.contains(&s.layer) || matches!(s.geom, Geometry::Label { .. }) {
            continue;
        }
        out.add(s.layer, s.geom.clone());
        rep.shapes += 1;
    }
    let unmatched: Vec<&str> = tsch
        .instances
        .iter()
        .filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos))
        .filter(|i| !out.instances.iter().any(|o| o.name.as_deref().map(device_of) == Some(i.name.as_str())))
        .map(|i| i.name.as_str())
        .collect();
    if !unmatched.is_empty() {
        rep.notes.push(format!("not placed in the reference: {}", unmatched.join(", ")));
    }
    let c = project.cell_mut(target).ok_or("no target cell")?;
    c.layout = Some(out);
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples::demo_project;

    #[test]
    fn renamed_copy_corresponds() {
        let p = demo_project();
        let r = CellRef::new("work", "nand2");
        let sch = p.cell(&r).unwrap().schematic.clone().unwrap();
        let mut renamed = sch.clone();
        for i in &mut renamed.instances {
            if matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos) {
                i.name = format!("X{}", i.name);
            }
        }
        let c = correspond(&sch, &renamed).unwrap();
        assert!(c.devices.iter().all(|(a, b)| b == &format!("X{a}") || !a.starts_with('M')), "{:?}", c.devices);
        // A different circuit does not correspond.
        let other = p.cell(&CellRef::new("work", "cmos_inverter")).unwrap().schematic.clone().unwrap();
        assert!(correspond(&sch, &other).is_err());
    }

    #[test]
    fn place_like_reuses_the_reference_placement() {
        let mut p = demo_project();
        let reference = CellRef::new("work", "nand2");
        let mut copy = p.cell(&reference).unwrap().clone();
        copy.name = "nand2_wide".into();
        copy.layout = None;
        if let Some(s) = copy.schematic.as_mut() {
            for i in &mut s.instances {
                if matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos) {
                    i.name = format!("N{}", i.name);
                }
            }
        }
        p.libraries.iter_mut().find(|l| l.name == "work").unwrap().cells.push(copy);
        let target = CellRef::new("work", "nand2_wide");
        let rep = place_like(&mut p, &reference, &target).unwrap();
        let rl = p.cell(&reference).unwrap().layout.clone().unwrap();
        let tl = p.cell(&target).unwrap().layout.clone().unwrap();
        let mos = |l: &Layout| {
            l.instances.iter().filter(|i| i.name.as_deref().is_some_and(|n| n.starts_with('M') || n.starts_with("NM"))).count()
        };
        assert_eq!(rep.placed, mos(&rl), "{rep:?}");
        for i in tl.instances.iter().filter(|i| i.name.as_deref().is_some_and(|n| n.starts_with("NM"))) {
            let n = i.name.as_deref().unwrap();
            let orig = rl.instances.iter().find(|o| o.name.as_deref() == Some(&n[1..])).expect("counterpart");
            assert_eq!(orig.trans, i.trans, "{n}");
        }
        assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    }
}
