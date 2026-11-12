//! Built-in example designs used by "New project from template" and tests.

use crate::geom::{Orient, Pt};
use crate::layout::{Geometry, Layout};
use crate::project::{Cell, Project};
use crate::schematic::{DeviceKind, Schematic};
use crate::sim_setup::{Analysis, SimSetup};
use crate::tech::Tech;

const R90: Orient = Orient { rot: 1, mirror: false };

fn set(s: &mut Schematic, id: u64, kv: &[(&str, &str)]) {
    let inst = s.instance_mut(id).expect("instance");
    for (k, v) in kv {
        inst.params.insert((*k).into(), (*v).into());
    }
}

fn wires(s: &mut Schematic, pts: &[(i32, i32)]) {
    for w in pts.windows(2) {
        s.add_wire(Pt::new(w[0].0, w[0].1), Pt::new(w[1].0, w[1].1));
    }
}

pub fn rc_lowpass() -> Cell {
    let mut s = Schematic::default();
    let o = Orient::default();
    let v1 = s.add_instance(DeviceKind::VSource, Pt::new(0, 4), o);
    set(&mut s, v1, &[("dc", "0"), ("ac", "1"), ("tran", "PULSE(0 1 1u 10n 10n 5u 10u)")]);
    s.add_instance(DeviceKind::Ground, Pt::new(0, 6), o);
    wires(&mut s, &[(0, 2), (0, 0), (4, 0)]);
    let r1 = s.add_instance(DeviceKind::Resistor, Pt::new(6, 0), R90);
    set(&mut s, r1, &[("r", "1k")]);
    wires(&mut s, &[(8, 0), (12, 0)]);
    let c1 = s.add_instance(DeviceKind::Capacitor, Pt::new(12, 2), o);
    set(&mut s, c1, &[("c", "1n")]);
    s.add_instance(DeviceKind::Ground, Pt::new(12, 4), o);
    s.add_label("in", Pt::new(2, 0));
    s.add_label("out", Pt::new(12, 0));

    let mut sim = SimSetup::default();
    sim.set_enabled("op", true);
    sim.set_enabled("ac", true);
    sim.set_enabled("tran", true);
    if let Some(Analysis::Ac { fstart, fstop, .. }) = sim.get_mut("ac") {
        *fstart = "10".into();
        *fstop = "100meg".into();
    }
    if let Some(Analysis::Tran { tstep, tstop }) = sim.get_mut("tran") {
        *tstep = "20n".into();
        *tstop = "20u".into();
    }
    Cell {
        name: "rc_lowpass".into(),
        description: "First-order RC low-pass filter (fc ≈ 159 kHz)".into(),
        schematic: Some(s),
        layout: None,
        sim,
    }
}

pub fn cmos_inverter() -> Cell {
    let mut s = Schematic::default();
    let o = Orient::default();
    // Transistors
    let mp = s.add_instance(DeviceKind::Pmos, Pt::new(10, 6), o);
    let mn = s.add_instance(DeviceKind::Nmos, Pt::new(10, 12), o);
    set(&mut s, mp, &[("w", "2u"), ("l", "180n")]);
    set(&mut s, mn, &[("w", "1u"), ("l", "180n")]);
    // Supply / ground at sources, bulk ties
    let vddn = s.add_instance(DeviceKind::Supply, Pt::new(11, 4), o);
    set(&mut s, vddn, &[("net", "vdd")]);
    s.add_instance(DeviceKind::Ground, Pt::new(11, 14), o);
    wires(&mut s, &[(12, 6), (12, 4), (11, 4)]);
    wires(&mut s, &[(12, 12), (12, 14), (11, 14)]);
    // Drains -> output
    wires(&mut s, &[(11, 8), (11, 10)]);
    wires(&mut s, &[(11, 9), (16, 9)]);
    // Gates -> input
    wires(&mut s, &[(8, 6), (8, 12)]);
    wires(&mut s, &[(5, 9), (8, 9)]);
    s.add_label("in", Pt::new(5, 9));
    s.add_label("out", Pt::new(16, 9));
    // Load capacitor
    let cl = s.add_instance(DeviceKind::Capacitor, Pt::new(16, 11), o);
    set(&mut s, cl, &[("c", "20f")]);
    s.add_instance(DeviceKind::Ground, Pt::new(16, 13), o);
    // Sources
    let vdd = s.add_instance(DeviceKind::VSource, Pt::new(-6, 8), o);
    set(&mut s, vdd, &[("dc", "1.8")]);
    let vddn2 = s.add_instance(DeviceKind::Supply, Pt::new(-6, 6), o);
    set(&mut s, vddn2, &[("net", "vdd")]);
    s.add_instance(DeviceKind::Ground, Pt::new(-6, 10), o);
    let vin = s.add_instance(DeviceKind::VSource, Pt::new(0, 12), o);
    set(&mut s, vin, &[("dc", "0"), ("tran", "PULSE(0 1.8 1n 100p 100p 4n 10n)")]);
    s.add_instance(DeviceKind::Ground, Pt::new(0, 14), o);
    s.add_label("in", Pt::new(0, 10));

    let mut sim = SimSetup::default();
    sim.set_enabled("op", true);
    sim.set_enabled("dc", true);
    sim.set_enabled("tran", true);
    if let Some(Analysis::Dc { source, .. }) = sim.get_mut("dc") {
        *source = "V2".into();
    }

    Cell {
        name: "cmos_inverter".into(),
        description: "Static CMOS inverter with 20 fF load".into(),
        schematic: Some(s),
        layout: Some(inverter_layout(&Tech::generic())),
        sim,
    }
}

pub fn diode_clipper() -> Cell {
    let mut s = Schematic::default();
    let o = Orient::default();
    let v1 = s.add_instance(DeviceKind::VSource, Pt::new(0, 4), o);
    set(&mut s, v1, &[("dc", "0"), ("tran", "SIN(0 3 1k)")]);
    s.add_instance(DeviceKind::Ground, Pt::new(0, 6), o);
    wires(&mut s, &[(0, 2), (0, 0), (4, 0)]);
    let r1 = s.add_instance(DeviceKind::Resistor, Pt::new(6, 0), R90);
    set(&mut s, r1, &[("r", "1k")]);
    wires(&mut s, &[(8, 0), (12, 0)]);
    s.add_instance(DeviceKind::Diode, Pt::new(12, 2), o);
    s.add_instance(DeviceKind::Ground, Pt::new(12, 4), o);
    s.add_label("in", Pt::new(2, 0));
    s.add_label("out", Pt::new(12, 0));
    let mut sim = SimSetup::default();
    sim.set_enabled("tran", true);
    sim.set_enabled("dc", true);
    if let Some(Analysis::Tran { tstep, tstop }) = sim.get_mut("tran") {
        *tstep = "5u".into();
        *tstop = "3m".into();
    }
    if let Some(Analysis::Dc { source, start, stop, step }) = sim.get_mut("dc") {
        *source = "V1".into();
        *start = "-3".into();
        *stop = "3".into();
        *step = "20m".into();
    }
    Cell {
        name: "diode_clipper".into(),
        description: "Series resistor + shunt diode clipper".into(),
        schematic: Some(s),
        layout: None,
        sim,
    }
}

/// Hand-drawn inverter layout in the generic technology (dimensions in nm).
pub fn inverter_layout(tech: &Tech) -> Layout {
    let l = |n: &str| tech.layer_index(n).expect("generic layer");
    let mut lay = Layout::default();
    // Wells & implants
    lay.add_rect(l("nwell"), -600, 2000, 2800, 5200);
    lay.add_rect(l("pplus"), -200, 2600, 2400, 4200);
    lay.add_rect(l("nplus"), -200, 200, 2400, 1600);
    // Diffusion
    lay.add_rect(l("active"), 0, 2800, 2200, 4000);
    lay.add_rect(l("active"), 0, 400, 2200, 1400);
    // Poly gate + input contact pad
    lay.add_rect(l("poly"), 1010, 200, 1190, 4200);
    lay.add_rect(l("poly"), 400, 1820, 1190, 2180);
    lay.add_rect(l("contact"), 520, 1890, 740, 2110);
    // Source / drain contacts
    for (x, y) in [(300, 3290), (1600, 3290), (300, 790), (1600, 790)] {
        lay.add_rect(l("contact"), x, y, x + 220, y + 220);
    }
    let m1 = l("metal1");
    // Rails
    lay.add_rect(m1, -600, 4800, 2800, 5400);
    lay.add_rect(m1, -600, -400, 2800, 200);
    // Source straps
    lay.add_rect(m1, 250, 3240, 570, 5000);
    lay.add_rect(m1, 250, -200, 570, 1060);
    // Output (drains)
    lay.add(m1, Geometry::Path { points: vec![(1710, 900), (1710, 3400)], width: 320 });
    lay.add_rect(m1, 1550, 1900, 2600, 2220);
    // Input pin
    lay.add_rect(m1, -400, 1840, 790, 2160);
    // Labels
    let t = l("metal1");
    lay.add(t, Geometry::Label { text: "vdd".into(), at: (-400, 4950), size: 300 });
    lay.add(t, Geometry::Label { text: "gnd".into(), at: (-400, -250), size: 300 });
    lay.add(t, Geometry::Label { text: "in".into(), at: (-350, 1880), size: 250 });
    lay.add(t, Geometry::Label { text: "out".into(), at: (2000, 1930), size: 250 });
    lay.add_rect(l("boundary"), -600, -400, 2800, 5400);
    lay
}

pub fn demo_project() -> Project {
    let mut p = Project::new("demo");
    p.description = "Example designs shipped with IC Studio".into();
    for c in [cmos_inverter(), rc_lowpass(), diode_clipper()] {
        p.add_cell("work", c).expect("unique");
    }
    p
}
