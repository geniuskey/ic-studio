//! A hierarchical schematic simulates as its flattened circuit: two inverter cells in a
//! chain pass the input level through.

use ics_core::geom::{Orient, Pt};
use ics_core::schematic::{DeviceKind, PortDir, Schematic};
use ics_core::{Analysis, Cell, CellRef, Project};

fn pin(s: &Schematic, id: u64, name: &str) -> Pt {
    s.instance(id).unwrap().pin_positions().into_iter().find(|(p, _)| *p == name).unwrap().1
}

fn port(s: &mut Schematic, name: &str, at: Pt, dir: PortDir) {
    let id = s.add_label(name, at);
    s.labels.iter_mut().find(|l| l.id == id).unwrap().port = Some(dir);
}

/// Inverter with pins a (input) and y (output), its supply from a `vdd` supply symbol.
fn inverter() -> Schematic {
    let mut s = Schematic::default();
    let p = s.add_instance(DeviceKind::Pmos, Pt::new(0, 0), Orient::default());
    let n = s.add_instance(DeviceKind::Nmos, Pt::new(0, 10), Orient::default());
    for (id, pin_name, net) in [(p, "g", "a"), (n, "g", "a"), (p, "d", "y"), (n, "d", "y")] {
        let at = pin(&s, id, pin_name);
        s.add_label(net, at);
    }
    for (id, pins) in [(p, ["s", "b"]), (n, ["s", "b"])] {
        for pn in pins {
            let at = pin(&s, id, pn);
            if id == p {
                let v = s.add_instance(DeviceKind::Supply, at, Orient::default());
                s.instance_mut(v).unwrap().params.insert("net".into(), "vdd".into());
            } else {
                s.add_instance(DeviceKind::Ground, at, Orient::default());
            }
        }
    }
    let (a, y) = (pin(&s, p, "g"), pin(&s, p, "d"));
    port(&mut s, "a", a, PortDir::Input);
    port(&mut s, "y", y, PortDir::Output);
    s
}

fn chain(vin: &str) -> (Project, CellRef) {
    let mut p = Project::new("h");
    let inv = CellRef::new("work", "inv");
    let mut c = Cell::new("inv");
    c.schematic = Some(inverter());
    p.add_cell("work", c).unwrap();
    let ports = p.cell(&inv).unwrap().schematic.as_ref().unwrap().ports();
    let mut top = Schematic::default();
    let x1 = top.add_cell_instance(inv.clone(), ports.clone(), Pt::new(0, 0), Orient::default());
    let x2 = top.add_cell_instance(inv.clone(), ports, Pt::new(20, 0), Orient::default());
    let (a1, y1, a2, y2) = (pin(&top, x1, "a"), pin(&top, x1, "y"), pin(&top, x2, "a"), pin(&top, x2, "y"));
    top.add_wire(y1, a2);
    top.add_label("out", y2);
    // Test bench: supply and input sources.
    let vdd = top.add_instance(DeviceKind::VSource, Pt::new(-20, 0), Orient::default());
    top.instance_mut(vdd).unwrap().params.insert("dc".into(), "1.8".into());
    let (vp, vn) = (pin(&top, vdd, "p"), pin(&top, vdd, "n"));
    let s = top.add_instance(DeviceKind::Supply, vp, Orient::default());
    top.instance_mut(s).unwrap().params.insert("net".into(), "vdd".into());
    top.add_instance(DeviceKind::Ground, vn, Orient::default());
    let vi = top.add_instance(DeviceKind::VSource, Pt::new(-10, 0), Orient::default());
    top.instance_mut(vi).unwrap().params.insert("dc".into(), vin.into());
    let (ip, inn) = (pin(&top, vi, "p"), pin(&top, vi, "n"));
    top.add_label("in", ip);
    top.add_label("in", a1);
    top.add_instance(DeviceKind::Ground, inn, Orient::default());
    let mut t = Cell::new("chain");
    t.schematic = Some(top);
    let r = p.add_cell("work", t).unwrap();
    (p, r)
}

#[test]
fn demo_buffer_test_bench() {
    let p = ics_core::examples::demo_project();
    let r = CellRef::new("work", "buffer_tb");
    let (circuit, warnings) = p.circuit_with_warnings(&r).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let nl = ics_core::netlist::netlist_for(&circuit, &p.tech, "");
    assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
    let deck = nl.deck("buffer_tb", &p.cell(&r).unwrap().sim);
    let res = ics_sim::simulate(&deck, &[], &ics_sim::SimOptions::default()).unwrap();
    let tran = res.datasets.iter().find(|d| d.analysis == "tran").expect("tran");
    let v = |n: &str| tran.vectors.iter().find(|v| v.name == n).unwrap_or_else(|| panic!("{n}"));
    let (t, out) = (&v("time").re, &v("v(out)").re);
    let at = |x: f64| out[t.iter().position(|&s| s >= x).unwrap()];
    assert!(at(0.5e-9) < 0.1 && at(2.5e-9) > 1.7 && at(4.5e-9) < 0.1, "a buffer follows its input");
}

#[test]
fn two_inverters_pass_the_level_through() {
    for (vin, high) in [("0", false), ("1.8", true)] {
        let (p, r) = chain(vin);
        let circuit = p.circuit(&r).unwrap();
        let nl = ics_core::netlist::netlist_for(&circuit, &p.tech, "");
        assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
        let deck = nl.deck("chain", &p.cell(&r).unwrap().sim);
        assert_eq!(deck.lines().filter(|l| l.starts_with("MX")).count(), 4, "{deck}");
        let res = ics_sim::simulate(&deck, &[Analysis::Op], &ics_sim::SimOptions::default()).unwrap();
        let out = res.datasets[0].vectors.iter().find(|v| v.name == "v(out)").expect("v(out)").re[0];
        assert!(if high { out > 1.7 } else { out < 0.1 }, "in={vin}: out={out}\n{deck}");
    }
    // A cell placing itself is refused.
    let (mut p, r) = chain("0");
    let inv = CellRef::new("work", "inv");
    let sch = p.cell_mut(&inv).unwrap().schematic.as_mut().unwrap();
    sch.add_cell_instance(r.clone(), vec![], Pt::new(50, 50), Orient::default());
    assert!(p.circuit(&r).unwrap_err().contains("places itself"));
}
