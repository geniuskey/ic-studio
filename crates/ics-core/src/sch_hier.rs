//! Hierarchical schematics: a schematic can place other cells' schematics
//! ([`DeviceKind::Cell`] instances). The tools that read a circuit (simulation netlists, LVS,
//! schematic-driven layout, ECO) work on its flattened form, [`flatten`]: every device of the
//! hierarchy as one flat schematic whose pins are tied by net labels.
//!
//! Names follow the instance path: device `M1` inside instance `X1` is `X1/M1`, its internal
//! net `n3` is `X1/n3`. A subcircuit's pins take the nets of the instance's pins. Ground and
//! the nets of supply symbols are global (shared by every level), as in the flat schematic.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::connectivity;
use crate::geom::{Orient, Pt};
use crate::project::CellRef;
use crate::schematic::{DeviceKind, PortDir, Schematic};

/// Deepest hierarchy flattened (a cell placing itself, directly or not, is an error anyway).
const MAX_DEPTH: usize = 32;

/// One device of the flattened circuit.
#[derive(Debug, Clone)]
struct FlatDev {
    kind: DeviceKind,
    name: String,
    params: BTreeMap<String, String>,
    /// (pin, net) in symbol pin order.
    pins: Vec<(&'static str, String)>,
}

/// Whether `sch` places other cells.
pub fn is_hierarchical(sch: &Schematic) -> bool {
    sch.instances.iter().any(|i| i.kind == DeviceKind::Cell)
}

/// The flattened schematic of `sch` and warnings (pins out of date, ...). `lookup` gives the
/// schematic of a placed cell. Errors: a missing cell or schematic, a recursive hierarchy.
pub fn flatten<'a>(
    sch: &Schematic,
    top: Option<&CellRef>,
    lookup: &dyn Fn(&CellRef) -> Option<&'a Schematic>,
) -> Result<(Schematic, Vec<String>), String> {
    let mut devs = vec![];
    let mut warnings = vec![];
    let mut stack: Vec<CellRef> = top.into_iter().cloned().collect();
    collect(sch, "", &HashMap::new(), lookup, &mut stack, &mut devs, &mut warnings)?;
    Ok((build(sch, &devs), warnings))
}

#[allow(clippy::too_many_arguments)]
fn collect<'a>(
    sch: &Schematic,
    path: &str,
    port_nets: &HashMap<String, String>,
    lookup: &dyn Fn(&CellRef) -> Option<&'a Schematic>,
    stack: &mut Vec<CellRef>,
    out: &mut Vec<FlatDev>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if stack.len() > MAX_DEPTH {
        return Err(format!("the hierarchy is deeper than {MAX_DEPTH} levels at {path}"));
    }
    let conn = connectivity::extract(sch);
    // Global nets: ground and the nets of supply symbols.
    let mut globals: HashSet<String> = HashSet::from(["0".to_string()]);
    for i in sch.instances.iter().filter(|i| i.kind == DeviceKind::Supply) {
        if let Some(n) = conn.net_of(i.id, "net") {
            globals.insert(n.to_string());
        }
    }
    let map = |net: &str| -> String {
        if path.is_empty() || globals.contains(net) || net.eq_ignore_ascii_case("gnd") {
            return net.to_string();
        }
        port_nets.get(net).cloned().unwrap_or_else(|| format!("{path}/{net}"))
    };
    let mut instances: Vec<_> = sch.instances.iter().collect();
    instances.sort_by(|a, b| a.name.cmp(&b.name));
    for inst in instances {
        let name = if path.is_empty() { inst.name.clone() } else { format!("{path}/{}", inst.name) };
        let pin_net = |pin: &str| conn.net_of(inst.id, pin).map(map).unwrap_or_else(|| map(&format!("{}_{pin}", inst.name)));
        match inst.kind {
            DeviceKind::Ground | DeviceKind::Supply => {}
            DeviceKind::Cell => {
                let cell = inst.cell.as_ref().ok_or_else(|| format!("{name}: a cell instance without a cell"))?;
                if stack.contains(cell) {
                    let chain: Vec<String> = stack.iter().chain([cell]).map(|c| c.to_string()).collect();
                    return Err(format!("{name}: the hierarchy places itself ({})", chain.join(" > ")));
                }
                let child = lookup(cell).ok_or_else(|| format!("{name}: {cell} has no schematic"))?;
                let ports = child.ports();
                let names: Vec<&str> = ports.iter().map(|(n, _)| n.as_str()).collect();
                let placed: Vec<&str> = inst.ports.iter().map(|(n, _)| n.as_str()).collect();
                if names != placed {
                    warnings.push(format!(
                        "{name}: the pins of {cell} changed ({}) since it was placed ({}): update the instance",
                        names.join(" "),
                        placed.join(" ")
                    ));
                }
                let nets: HashMap<String, String> =
                    ports.iter().filter(|(n, _)| placed.contains(&n.as_str())).map(|(n, _)| (n.clone(), pin_net(n))).collect();
                stack.push(cell.clone());
                collect(child, &name, &nets, lookup, stack, out, warnings)?;
                stack.pop();
            }
            kind => {
                let mut params = inst.params.clone();
                // A current-controlled source senses a voltage source of its own level.
                if matches!(kind, DeviceKind::Cccs | DeviceKind::Ccvs) && !path.is_empty() {
                    let v = inst.param("vctl").trim();
                    if !v.is_empty() {
                        params.insert("vctl".into(), format!("{path}/{v}"));
                    }
                }
                // The body net of a 3-terminal MOS is named by its `bulk` parameter.
                if kind.implicit_bulk() {
                    let b = inst.param("bulk").trim();
                    let b = if b.is_empty() { kind.default_bulk() } else { b };
                    params.insert("bulk".into(), map(b));
                }
                let pins = inst.pin_positions().into_iter().map(|(p, _)| (p, pin_net(p))).collect();
                out.push(FlatDev { kind, name, params, pins });
            }
        }
    }
    Ok(())
}

/// The flat schematic of `devs`: each device on its own spot with a label on every pin, plus
/// the top schematic's pins as port labels.
fn build(top: &Schematic, devs: &[FlatDev]) -> Schematic {
    let mut s = Schematic::default();
    let mut first_pin: HashMap<String, Pt> = HashMap::new();
    for (k, d) in devs.iter().enumerate() {
        let pos = Pt::new((k % 16) as i32 * 10, (k / 16) as i32 * 12);
        let id = s.add_instance(d.kind, pos, Orient::default());
        let inst = s.instance_mut(id).expect("just added");
        inst.name = d.name.clone();
        inst.params = d.params.clone();
        let at: HashMap<&str, Pt> = inst.pin_positions().into_iter().collect();
        for (pin, net) in &d.pins {
            let Some(&p) = at.get(pin) else { continue };
            if net == "0" || net.eq_ignore_ascii_case("gnd") {
                let g = s.add_instance(DeviceKind::Ground, p, Orient::default());
                let gname = s.next_name(DeviceKind::Ground);
                s.instance_mut(g).expect("just added").name = gname;
            } else {
                s.add_label(net.clone(), p);
                first_pin.entry(net.clone()).or_insert(p);
            }
        }
    }
    // The cell's own pins, so that the flat schematic has the same ports.
    let mut spare = Pt::new(0, -12);
    for (name, dir) in top.ports() {
        let at = first_pin.get(&name).copied().unwrap_or_else(|| {
            spare.x += 4;
            spare
        });
        let id = s.add_label(name, at);
        if let Some(l) = s.labels.iter_mut().find(|l| l.id == id) {
            l.port = Some(dir);
        }
    }
    s
}

/// Pins of a cell for placing it: its schematic's ports.
pub fn ports_of(sch: &Schematic) -> Vec<(String, PortDir)> {
    sch.ports()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::netlist;

    /// An inverter cell with pins a (in), y (out) and a supply symbol.
    fn inverter() -> Schematic {
        let mut s = Schematic::default();
        let p = s.add_instance(DeviceKind::Pmos, Pt::new(0, 0), Orient::default());
        let n = s.add_instance(DeviceKind::Nmos, Pt::new(0, 10), Orient::default());
        let pin = |s: &Schematic, id: u64, name: &str| {
            s.instance(id).unwrap().pin_positions().into_iter().find(|(p, _)| *p == name).unwrap().1
        };
        let (pg, pd, ps, pb) = (pin(&s, p, "g"), pin(&s, p, "d"), pin(&s, p, "s"), pin(&s, p, "b"));
        let (ng, nd, ns, nb) = (pin(&s, n, "g"), pin(&s, n, "d"), pin(&s, n, "s"), pin(&s, n, "b"));
        for (at, net) in [(pg, "a"), (ng, "a"), (pd, "y"), (nd, "y"), (ps, "vdd"), (pb, "vdd"), (ns, "0"), (nb, "0")] {
            if net == "0" {
                s.add_instance(DeviceKind::Ground, at, Orient::default());
            } else if net == "vdd" {
                let id = s.add_instance(DeviceKind::Supply, at, Orient::default());
                s.instance_mut(id).unwrap().params.insert("net".into(), "vdd".into());
            } else {
                s.add_label(net, at);
            }
        }
        for (name, dir) in [("a", PortDir::Input), ("y", PortDir::Output)] {
            let at = if name == "a" { pg } else { pd };
            let id = s.add_label(name, at);
            s.labels.iter_mut().find(|l| l.id == id).unwrap().port = Some(dir);
        }
        s
    }

    #[test]
    fn two_inverters_in_a_chain() {
        let inv = inverter();
        let cell = CellRef::new("work", "inv");
        let mut top = Schematic::default();
        let ports = inv.ports();
        assert_eq!(ports, [("a".to_string(), PortDir::Input), ("y".to_string(), PortDir::Output)]);
        let x1 = top.add_cell_instance(cell.clone(), ports.clone(), Pt::new(0, 0), Orient::default());
        let x2 = top.add_cell_instance(cell.clone(), ports.clone(), Pt::new(20, 0), Orient::default());
        let at = |s: &Schematic, id: u64, p: &str| {
            s.instance(id).unwrap().pin_positions().into_iter().find(|(n, _)| *n == p).unwrap().1
        };
        let (a1, y1, a2, y2) = (at(&top, x1, "a"), at(&top, x1, "y"), at(&top, x2, "a"), at(&top, x2, "y"));
        top.add_label("in", a1);
        top.add_wire(y1, a2);
        top.add_label("out", y2);
        let lookup = |r: &CellRef| (r == &cell).then_some(&inv);
        let (flat, warnings) = flatten(&top, None, &lookup).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let names: Vec<&str> = flat.instances.iter().filter(|i| i.kind.is_mos()).map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["X1/M1", "X1/M2", "X2/M1", "X2/M2"]);
        let nl = netlist(&flat).body;
        // X1's output drives X2's input: one internal net between them; vdd and ground are global.
        let line = |n: &str| nl.lines().find(|l| l.starts_with(n)).unwrap_or_else(|| panic!("{n} in\n{nl}")).to_string();
        let mid = line("MX1/M1").split_whitespace().nth(1).unwrap().to_string();
        assert!(line("MX2/M1").split_whitespace().nth(2) == Some(mid.as_str()), "{nl}");
        assert!(
            line("MX1/M1").contains(" in vdd vdd ")
                && line("MX2/M2").contains(" out ")
                && line("MX2/M2").ends_with(" 0 0 nmos w=1u l=180n")
                || line("MX2/M2").contains(" 0 0 "),
            "{nl}"
        );
    }

    #[test]
    fn recursion_and_stale_pins() {
        let cell = CellRef::new("work", "loop");
        let mut me = Schematic::default();
        me.add_cell_instance(cell.clone(), vec![], Pt::new(0, 0), Orient::default());
        let lookup = |r: &CellRef| (r == &cell).then_some(&me);
        let e = flatten(&me, Some(&cell), &lookup).unwrap_err();
        assert!(e.contains("places itself"), "{e}");
        let inv = inverter();
        let other = CellRef::new("work", "inv");
        let mut top = Schematic::default();
        top.add_cell_instance(other.clone(), vec![("a".into(), PortDir::Input)], Pt::new(0, 0), Orient::default());
        let lookup = |r: &CellRef| (r == &other).then_some(&inv);
        let (_, w) = flatten(&top, None, &lookup).unwrap();
        assert!(w.iter().any(|w| w.contains("changed")), "{w:?}");
        let missing = |_: &CellRef| None;
        assert!(flatten(&top, None, &missing).unwrap_err().contains("has no schematic"));
    }
}
