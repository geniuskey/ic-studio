//! Built-in example designs used by "New project from template" and tests.

use std::collections::{HashMap, HashSet};

use crate::geom::{IRect, Orient, Pt};
use crate::layout::{ArraySpec, Geometry, Layout, Trans};
use crate::pcell::{PCell, via_at};
use crate::project::{Cell, CellRef, Project};
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

fn pin_at(s: &Schematic, id: u64, name: &str) -> Pt {
    s.instance(id).expect("instance").pin_positions().into_iter().find(|(p, _)| *p == name).expect("pin").1
}

/// An inverter with schematic pins `a` (input) and `y` (output), supplied by `vdd` and ground
/// symbols: a cell to place in other schematics.
pub fn inv() -> Cell {
    use crate::schematic::PortDir;
    let mut s = Schematic::default();
    let o = Orient::default();
    let mp = s.add_instance(DeviceKind::Pmos, Pt::new(0, 0), o);
    let mn = s.add_instance(DeviceKind::Nmos, Pt::new(0, 8), o);
    set(&mut s, mp, &[("w", "2u")]);
    set(&mut s, mn, &[("w", "1u")]);
    for (a, b) in [(pin_at(&s, mp, "g"), pin_at(&s, mn, "g")), (pin_at(&s, mp, "d"), pin_at(&s, mn, "d"))] {
        s.add_wire(a, Pt::new(a.x, b.y));
    }
    for pn in ["s", "b"] {
        let at = pin_at(&s, mp, pn);
        let v = s.add_instance(DeviceKind::Supply, at, o);
        set(&mut s, v, &[("net", "vdd")]);
        let at = pin_at(&s, mn, pn);
        s.add_instance(DeviceKind::Ground, at, o);
    }
    for (name, at, dir) in [("a", pin_at(&s, mp, "g"), PortDir::Input), ("y", pin_at(&s, mp, "d"), PortDir::Output)] {
        let id = s.add_label(name, at);
        if let Some(l) = s.labels.iter_mut().find(|l| l.id == id) {
            l.port = Some(dir);
        }
    }
    Cell {
        description: "Inverter with pins a / y: placed by buffer_tb (a hierarchical schematic)".into(),
        schematic: Some(s),
        ..Cell::new("inv")
    }
}

/// A test bench placing two `inv` cells as a buffer: a hierarchical schematic. Shift+X on an
/// instance descends into `inv`, Shift+B returns.
pub fn buffer_tb() -> Cell {
    let mut s = Schematic::default();
    let o = Orient::default();
    let inv = CellRef::new("work", "inv");
    let ports = inv_ports();
    let x1 = s.add_cell_instance(inv.clone(), ports.clone(), Pt::new(10, 0), o);
    let x2 = s.add_cell_instance(inv, ports, Pt::new(24, 0), o);
    s.add_wire(pin_at(&s, x1, "y"), pin_at(&s, x2, "a"));
    let a1 = pin_at(&s, x1, "a");
    let y2 = pin_at(&s, x2, "y");
    s.add_wire(y2, Pt::new(y2.x + 4, y2.y));
    s.add_label("out", Pt::new(y2.x + 4, y2.y));
    let c = s.add_instance(DeviceKind::Capacitor, Pt::new(y2.x + 4, y2.y + 2), o);
    // The load is part of the test bench, not of the circuit LVS compares.
    set(&mut s, c, &[("c", "10f"), (crate::schematic::LVS_IGNORE, "t")]);
    let at = pin_at(&s, c, "n");
    s.add_instance(DeviceKind::Ground, at, o);
    let vin = s.add_instance(DeviceKind::VSource, Pt::new(0, 2), o);
    set(&mut s, vin, &[("dc", "0"), ("tran", "PULSE(0 1.8 1n 100p 100p 2n 4n)")]);
    s.add_wire(pin_at(&s, vin, "p"), Pt::new(0, a1.y));
    s.add_wire(Pt::new(0, a1.y), a1);
    s.add_label("in", Pt::new(0, a1.y));
    let at = pin_at(&s, vin, "n");
    s.add_instance(DeviceKind::Ground, at, o);
    let vdd = s.add_instance(DeviceKind::VSource, Pt::new(-8, 2), o);
    set(&mut s, vdd, &[("dc", "1.8")]);
    let at = pin_at(&s, vdd, "p");
    let sup = s.add_instance(DeviceKind::Supply, at, o);
    set(&mut s, sup, &[("net", "vdd")]);
    let at = pin_at(&s, vdd, "n");
    s.add_instance(DeviceKind::Ground, at, o);
    let mut sim = SimSetup::default();
    sim.set_enabled("op", true);
    sim.set_enabled("tran", true);
    if let Some(Analysis::Tran { tstep, tstop, .. }) = sim.get_mut("tran") {
        *tstep = "10p".into();
        *tstop = "8n".into();
    }
    Cell {
        description: "Buffer of two inv instances (hierarchical schematic): descend with Shift+X, simulate flattened".into(),
        schematic: Some(s),
        sim,
        ..Cell::new("buffer_tb")
    }
}

/// A ΔVbe bandgap reference: two NPN of area 1 and 8 held at equal voltages by an ideal
/// amplifier (a VCVS); about 1.2 V, swept over temperature with a spec on the output.
pub fn bandgap() -> Cell {
    let deck = "* bandgap\nE1 out 0 a b 1e4\nR1 out a 51k\nQ1 a a 0 npn\nR2 out b 51k\nR3 b c2 5k\nQ2 c2 c2 0 npn 8\n.op\n.end\n";
    let imp = crate::spice_import::import(deck, &Tech::generic()).expect("the bandgap deck imports");
    let mut sim = SimSetup::default();
    imp.apply_to(&mut sim);
    sim.sweep.temps = "0 27 100".into();
    sim.sweep.specs = vec![crate::sweep::Spec {
        name: "vref".into(),
        analysis: "op".into(),
        expr: "v(out)".into(),
        min: "1.15".into(),
        max: "1.25".into(),
    }];
    Cell {
        description: "ΔVbe bandgap reference: NPN 1:8 and an ideal amplifier (VCVS), about 1.2 V over 0 to 100 °C".into(),
        schematic: Some(imp.schematic),
        sim,
        ..Cell::new("bandgap")
    }
}

fn inv_ports() -> Vec<(String, crate::schematic::PortDir)> {
    inv().schematic.expect("schematic").ports()
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
    if let Some(Analysis::Tran { tstep, tstop, .. }) = sim.get_mut("tran") {
        *tstep = "20n".into();
        *tstop = "20u".into();
    }
    // Verification: the bandwidth over a ±10 % spread of R1 and C1.
    sim.sweep.vars = vec![
        crate::sweep::SweepVar { name: "R1.r".into(), values: "900 1k 1.1k".into() },
        crate::sweep::SweepVar { name: "C1.c".into(), values: "0.9n 1n 1.1n".into() },
    ];
    sim.sweep.specs = vec![
        crate::sweep::Spec {
            name: "bw".into(),
            analysis: "ac".into(),
            expr: "bandwidth(v(out), 3)".into(),
            min: "120k".into(),
            max: "200k".into(),
        },
        crate::sweep::Spec {
            name: "rise".into(),
            analysis: "tran".into(),
            expr: "delay(v(in), 0.5, v(out), 0.5)".into(),
            min: "".into(),
            max: "1.5u".into(),
        },
    ];
    Cell {
        name: "rc_lowpass".into(),
        description: "First-order RC low-pass filter (fc ≈ 159 kHz)".into(),
        schematic: Some(s),
        layout: None,
        sim,
        pcell: None,
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
        pcell: None,
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
    if let Some(Analysis::Tran { tstep, tstop, .. }) = sim.get_mut("tran") {
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
        pcell: None,
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
    lay.add_rect(l("poly"), 1010, 180, 1190, 4220);
    lay.add_rect(l("poly"), 400, 1790, 1190, 2210);
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
    lay.add_rect(m1, 240, 3230, 580, 5000);
    lay.add_rect(m1, 240, -200, 580, 1070);
    // Output (drains)
    lay.add(m1, Geometry::Path { points: vec![(1710, 900), (1710, 3400)], width: 340 });
    lay.add_rect(m1, 1550, 1900, 2600, 2220);
    // Input pin
    lay.add_rect(m1, -400, 1830, 800, 2170);
    // Labels
    let t = l("metal1");
    lay.add(t, Geometry::Label { text: "vdd".into(), at: (-400, 4950), size: 300 });
    lay.add(t, Geometry::Label { text: "gnd".into(), at: (-400, -250), size: 300 });
    lay.add(t, Geometry::Label { text: "in".into(), at: (-350, 1880), size: 250 });
    lay.add(t, Geometry::Label { text: "out".into(), at: (2000, 1930), size: 250 });
    lay.add_rect(l("boundary"), -600, -400, 2800, 5400);
    lay
}

fn layout_cell(name: &str, description: &str, layout: Layout) -> Cell {
    Cell {
        name: name.into(),
        description: description.into(),
        schematic: None,
        layout: Some(layout),
        sim: SimSetup::default(),
        pcell: None,
    }
}

/// Inverter cell pitch (its boundary) in the generic technology.
const INV_W: i64 = 3400;
const INV_H: i64 = 5800;
/// Array row pitch: the supply rails of stacked rows must not touch (vdd above, gnd below).
const INV_ROW: i64 = INV_H + 400;

/// Hierarchical demo cells: a chain of placed inverters, an inverter array and an array of
/// arrays (8192 inverters, about 250k flattened shapes).
pub fn hierarchy_cells(tech: &Tech, lib: &str) -> Vec<Cell> {
    let inv = CellRef::new(lib, "cmos_inverter");
    let text = tech.layer_index("text").expect("generic layer");
    let m1 = tech.layer_index("metal1").expect("generic layer");

    let mut chain = Layout::default();
    for k in 0..4 {
        chain.add_instance(inv.clone(), Trans::new(k * INV_W, 0), None);
    }
    for k in 0..3 {
        let x = k * INV_W + 2600;
        chain.add(m1, Geometry::Path { points: vec![(x, 2060), (x + 1000, 2060)], width: 320 });
    }
    chain.add(text, Geometry::Label { text: "inv_chain".into(), at: (-400, 5700), size: 400 });
    for (name, at) in [("vdd", (-400, 5000)), ("gnd", (-400, -200)), ("in", (-300, 2000)), ("out", (3 * INV_W + 2000, 2000))] {
        chain.add(text, Geometry::Label { text: name.into(), at, size: 250 });
    }

    let mut array = Layout::default();
    array.add_instance(inv.clone(), Trans::new(0, 0), Some(ArraySpec { cols: 16, rows: 8, col: (INV_W, 0), row: (0, INV_ROW) }));
    array.add_rect(tech.layer_index("boundary").expect("generic layer"), -600, -400, 16 * INV_W - 600, 8 * INV_ROW - 400);

    let (aw, ah) = (16 * INV_W + 2000, 8 * INV_ROW + 2000);
    let mut tiles = Layout::default();
    tiles.add_instance(
        CellRef::new(lib, "inv_array"),
        Trans::new(0, 0),
        Some(ArraySpec { cols: 8, rows: 8, col: (aw, 0), row: (0, ah) }),
    );
    tiles.add(text, Geometry::Label { text: "8192 inverters".into(), at: (0, 8 * ah), size: 8000 });

    vec![
        layout_cell("inv_chain", "Four placed inverter instances", chain),
        layout_cell("inv_array", "16 × 8 array of inverters (AREF)", array),
        layout_cell("inv_tiles", "8 × 8 array of inv_array: two levels of hierarchy", tiles),
    ]
}

/// MOS device. The bulk goes to a label `bulk` if given, else to the source (PMOS) or ground (NMOS).
fn mos(s: &mut Schematic, kind: DeviceKind, at: (i32, i32), mirror: bool, w: &str, l: &str, bulk: Option<&str>) -> u64 {
    let id = s.add_instance(kind, Pt::new(at.0, at.1), Orient { rot: 0, mirror });
    set(s, id, &[("w", w), ("l", l)]);
    let pins = s.instance(id).expect("instance").pin_positions();
    let pin = |name: &str| pins.iter().find(|(n, _)| *n == name).expect("MOS pin").1;
    let (b, src) = (pin("b"), pin("s"));
    match bulk {
        Some(net) => {
            s.add_label(net, b);
        }
        None if kind == DeviceKind::Pmos => wires(s, &[(b.x, b.y), (b.x, src.y), (src.x, src.y)]),
        None => {
            s.add_instance(DeviceKind::Ground, b, Orient::default());
        }
    }
    id
}

/// Test-bench voltage source from `net` to ground.
fn source(s: &mut Schematic, (x, y): (i32, i32), net: &str, kv: &[(&str, &str)]) {
    let o = Orient::default();
    let id = s.add_instance(DeviceKind::VSource, Pt::new(x, y), o);
    set(s, id, kv);
    s.add_instance(DeviceKind::Ground, Pt::new(x, y + 2), o);
    if net == "vdd" {
        s.add_instance(DeviceKind::Supply, Pt::new(x, y - 2), o);
    } else {
        s.add_label(net, Pt::new(x, y - 2));
    }
}

/// Horizontal wire stub from a pin with a net label at its far end.
fn stub(s: &mut Schematic, (x, y): (i32, i32), dx: i32, net: &str) {
    wires(s, &[(x, y), (x + dx, y)]);
    s.add_label(net, Pt::new(x + dx, y));
}

fn sim(enable: &[&str]) -> SimSetup {
    let mut sim = SimSetup::default();
    sim.set_enabled("op", false);
    for a in enable {
        sim.set_enabled(a, true);
    }
    sim
}

fn set_tran(sim: &mut SimSetup, step: &str, stop: &str) {
    if let Some(Analysis::Tran { tstep, tstop, .. }) = sim.get_mut("tran") {
        *tstep = step.into();
        *tstop = stop.into();
    }
}

fn full_flow_cell(name: &str, description: &str, schematic: Schematic, layout: Layout, sim: SimSetup) -> Cell {
    Cell {
        name: name.into(),
        description: description.into(),
        schematic: Some(schematic),
        layout: Some(layout),
        sim,
        pcell: None,
    }
}

/// Wire width on metal1-3: the via plate size, so wires and via plates merge without notches.
const WIRE: i64 = 380;
/// Pitch of horizontal routing tracks.
const TRACK: i64 = 700;

/// Routing tracks around a two-row placement: NMOS (gate pads up) at y = 0, PMOS mirrored
/// (gate pads down) above, `mid` tracks between the rows for gates.
#[derive(Clone, Copy)]
struct Tracks {
    wn: i64,
    wp: i64,
    mid: i64,
}

impl Tracks {
    fn mid(self, i: i64) -> i64 {
        self.wn + 1100 + i * TRACK
    }
    /// Top edge of the PMOS diffusion.
    fn p_row(self) -> i64 {
        self.mid(self.mid - 1) + self.wp + 1100
    }
    fn top(self, i: i64) -> i64 {
        self.p_row() + (i + 1) * TRACK
    }
    fn bottom(self, i: i64) -> i64 {
        -(i + 1) * TRACK
    }
}

/// Builds a flat layout of named MOS PCell instances wired with metal1 straps (vertical, from a
/// terminal to a track), metal2 (horizontal) and metal3 (vertical) wires.
struct Router {
    tech: Tech,
    lay: Layout,
    pins: HashMap<(String, String), IRect>,
    nwell: Option<IRect>,
    vias: HashSet<(&'static str, (i64, i64))>,
}

impl Router {
    fn new(tech: &Tech) -> Self {
        Router { tech: tech.clone(), lay: Layout::default(), pins: HashMap::new(), nwell: None, vias: HashSet::new() }
    }

    fn layer(&self, name: &str) -> usize {
        self.tech.layer_index(name).expect("generic layer")
    }

    /// Place a single-finger MOS PCell named `name`; PMOS is mirrored so its gate faces down.
    fn mos(&mut self, p: &mut Project, name: &str, pmos: bool, (w, l): (i64, i64), (x, y): (i64, i64)) {
        let cell = p.ensure_pcell(&PCell::mos(pmos, w, l, 1)).expect("MOS pcell");
        let trans = Trans::with(x, y, 0, pmos);
        let (m1, nwell) = (self.layer("metal1"), self.layer("nwell"));
        let child = p.cell(&cell).and_then(|c| c.layout.as_ref()).expect("pcell layout");
        for s in &child.shapes {
            match &s.geom {
                Geometry::Label { text, .. } => {
                    let b = s.geom.bbox();
                    let (cx, cy) = ((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2);
                    let plate = child
                        .shapes
                        .iter()
                        .find(|q| q.layer == m1 && matches!(q.geom, Geometry::Rect(_)) && q.bbox().contains(cx, cy))
                        .expect("terminal plate");
                    self.pins.insert((name.into(), text.clone()), trans.apply_rect(plate.bbox()));
                }
                Geometry::Rect(r) if s.layer == nwell => {
                    let r = trans.apply_rect(*r);
                    self.nwell = Some(self.nwell.map_or(r, |n| n.union(&r)));
                }
                _ => {}
            }
        }
        let id = self.lay.add_instance(cell, trans, None);
        self.lay.instance_mut(id).expect("placed").name = Some(name.into());
    }

    fn at(&self, dev: &str, pin: &str) -> (i64, i64) {
        let r = self.pins[&(dev.to_string(), pin.to_string())];
        ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
    }

    /// Horizontal extent of all placed terminals.
    fn span(&self) -> (i64, i64) {
        self.pins.values().fold((i64::MAX, i64::MIN), |(a, b), r| (a.min(r.x0), b.max(r.x1)))
    }

    fn via(&mut self, name: &'static str, at: (i64, i64)) {
        if self.vias.insert((name, at)) {
            let def = self.tech.via(name).expect("generic via");
            for (l, g) in via_at(&self.tech, def, at).expect("via") {
                self.lay.add(l, g);
            }
        }
    }

    fn bar(&mut self, layer: &str, a: (i64, i64), b: (i64, i64)) {
        let (l, h) = (self.layer(layer), WIRE / 2);
        self.lay.add_rect(l, a.0.min(b.0) - h, a.1.min(b.1) - h, a.0.max(b.0) + h, a.1.max(b.1) + h);
    }

    /// Bring a terminal up to metal2 at height `y` (its centre when `y` is inside it).
    fn tap(&mut self, dev: &str, pin: &str, y: i64) -> (i64, i64) {
        let (x, cy) = self.at(dev, pin);
        if y != cy {
            self.bar("metal1", (x, cy), (x, y));
        }
        self.via("via1", (x, y));
        (x, y)
    }

    fn centre_tap(&mut self, dev: &str, pin: &str) -> (i64, i64) {
        let y = self.at(dev, pin).1;
        self.tap(dev, pin, y)
    }

    /// Manhattan wire through `pts`: horizontal runs on metal2, vertical runs on metal3, with a
    /// via2 wherever a vertical run turns or ends.
    fn route(&mut self, pts: &[(i64, i64)]) {
        let vertical: Vec<bool> = pts.windows(2).map(|w| w[0].0 == w[1].0).collect();
        for (w, &v) in pts.windows(2).zip(&vertical) {
            self.bar(if v { "metal3" } else { "metal2" }, w[0], w[1]);
        }
        for (i, &p) in pts.iter().enumerate() {
            let before = i.checked_sub(1).map(|j| vertical[j]);
            let after = vertical.get(i).copied();
            let ends = before.is_none() || after.is_none();
            if (before == Some(true) || after == Some(true)) && (ends || before != after) {
                self.via("via2", p);
            }
        }
    }

    /// A supply rail across the whole placement at height `y`, tapping the given terminals.
    fn rail(&mut self, net: &str, y: i64, taps: &[(&str, &str)]) {
        for (dev, pin) in taps {
            self.tap(dev, pin, y);
        }
        let (x0, x1) = self.span();
        self.route(&[(x0, y), (x1, y)]);
        self.label(net, (x0, y));
    }

    fn label(&mut self, text: &str, at: (i64, i64)) {
        let t = self.layer("text");
        self.lay.add(t, Geometry::Label { text: text.into(), at, size: 200 });
    }

    fn finish(mut self) -> Layout {
        if let Some(r) = self.nwell {
            let l = self.layer("nwell");
            self.lay.add(l, Geometry::Rect(r));
        }
        self.lay
    }
}

const NW: (i64, i64) = (1000, 180);
const PW: (i64, i64) = (2000, 180);
const GATE_COL: i64 = 2000;

/// (PMOS name, PMOS (W, L), NMOS name, NMOS (W, L)); an empty name leaves the slot free.
type Column<'a> = (&'a str, (i64, i64), &'a str, (i64, i64));

/// Places one PMOS above one NMOS per column (`(pmos, nmos)` names, empty for none).
fn place_columns(r: &mut Router, p: &mut Project, t: Tracks, pitch: i64, cols: &[Column]) {
    for (k, (pm, pw, nm, nw)) in cols.iter().enumerate() {
        let x = k as i64 * pitch;
        if !pm.is_empty() {
            r.mos(p, pm, true, *pw, (x, t.p_row()));
        }
        if !nm.is_empty() {
            r.mos(p, nm, false, *nw, (x, 0));
        }
    }
}

/// Joins the PMOS and NMOS gates of a column on mid track `i` and labels the net there.
fn gate_pair(r: &mut Router, t: Tracks, pm: &str, nm: &str, i: i64, net: &str) {
    let g = r.tap(nm, "G", t.mid(i));
    r.tap(pm, "G", t.mid(i));
    r.label(net, g);
}

pub fn nand2(p: &mut Project) -> Cell {
    let mut s = Schematic::default();
    mos(&mut s, DeviceKind::Pmos, (10, 4), false, "2u", "180n", None);
    mos(&mut s, DeviceKind::Pmos, (20, 4), false, "2u", "180n", None);
    mos(&mut s, DeviceKind::Nmos, (14, 10), false, "1u", "180n", None);
    mos(&mut s, DeviceKind::Nmos, (14, 15), false, "1u", "180n", None);
    wires(&mut s, &[(11, 2), (21, 2)]);
    s.add_instance(DeviceKind::Supply, Pt::new(16, 2), Orient::default());
    wires(&mut s, &[(11, 6), (21, 6), (24, 6)]);
    wires(&mut s, &[(15, 6), (15, 8)]);
    wires(&mut s, &[(15, 12), (15, 13)]);
    s.add_instance(DeviceKind::Ground, Pt::new(15, 17), Orient::default());
    s.add_label("out", Pt::new(24, 6));
    stub(&mut s, (8, 4), -2, "a");
    stub(&mut s, (18, 4), -2, "b");
    stub(&mut s, (12, 10), -2, "a");
    stub(&mut s, (12, 15), -2, "b");
    source(&mut s, (-20, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-10, 8), "a", &[("dc", "1.8"), ("tran", "PULSE(0 1.8 1n 50p 50p 2n 4n)")]);
    source(&mut s, (0, 8), "b", &[("dc", "1.8"), ("tran", "PULSE(0 1.8 1n 50p 50p 4n 8n)")]);
    let mut sim = sim(&["op", "tran"]);
    set_tran(&mut sim, "10p", "10n");

    let t = Tracks { wn: NW.0, wp: PW.0, mid: 1 };
    let mut r = Router::new(&p.tech);
    place_columns(&mut r, p, t, GATE_COL, &[("M1", PW, "M3", NW), ("M2", PW, "M4", NW)]);
    gate_pair(&mut r, t, "M1", "M3", 0, "a");
    gate_pair(&mut r, t, "M2", "M4", 0, "b");
    let a = r.tap("M1", "D", t.top(0));
    let b = r.tap("M2", "D", t.top(0));
    r.route(&[a, b]);
    let n = r.centre_tap("M3", "D");
    r.route(&[n, a]);
    r.label("out", b);
    let s3 = r.tap("M3", "S", t.bottom(0));
    let d4 = r.tap("M4", "D", t.bottom(0));
    r.route(&[s3, d4]);
    r.rail("vdd", t.top(1), &[("M1", "S"), ("M2", "S")]);
    r.rail("gnd", t.bottom(1), &[("M4", "S")]);
    full_flow_cell("nand2", "2-input CMOS NAND, routed PCell layout (DRC / LVS clean)", s, r.finish(), sim)
}

pub fn nor2(p: &mut Project) -> Cell {
    let mut s = Schematic::default();
    mos(&mut s, DeviceKind::Pmos, (13, 4), false, "2u", "180n", None);
    mos(&mut s, DeviceKind::Pmos, (13, 9), false, "2u", "180n", Some("vdd"));
    mos(&mut s, DeviceKind::Nmos, (10, 15), false, "1u", "180n", None);
    mos(&mut s, DeviceKind::Nmos, (18, 15), false, "1u", "180n", None);
    s.add_instance(DeviceKind::Supply, Pt::new(14, 2), Orient::default());
    wires(&mut s, &[(14, 6), (14, 7)]);
    wires(&mut s, &[(14, 11), (14, 13)]);
    wires(&mut s, &[(11, 13), (19, 13), (22, 13)]);
    wires(&mut s, &[(11, 17), (19, 17)]);
    s.add_instance(DeviceKind::Ground, Pt::new(15, 17), Orient::default());
    s.add_label("out", Pt::new(22, 13));
    stub(&mut s, (11, 4), -2, "a");
    stub(&mut s, (11, 9), -2, "b");
    stub(&mut s, (8, 15), -2, "a");
    stub(&mut s, (16, 15), -2, "b");
    source(&mut s, (-20, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-10, 8), "a", &[("dc", "0"), ("tran", "PULSE(0 1.8 1n 50p 50p 2n 4n)")]);
    source(&mut s, (0, 8), "b", &[("dc", "0"), ("tran", "PULSE(0 1.8 1n 50p 50p 4n 8n)")]);
    let mut sim = sim(&["op", "tran"]);
    set_tran(&mut sim, "10p", "10n");

    let t = Tracks { wn: NW.0, wp: PW.0, mid: 1 };
    let mut r = Router::new(&p.tech);
    place_columns(&mut r, p, t, GATE_COL, &[("M1", PW, "M3", NW), ("M2", PW, "M4", NW)]);
    gate_pair(&mut r, t, "M1", "M3", 0, "a");
    gate_pair(&mut r, t, "M2", "M4", 0, "b");
    let y1 = r.tap("M1", "D", t.top(0));
    let y2 = r.tap("M2", "S", t.top(0));
    r.route(&[y1, y2]);
    let a = r.tap("M3", "D", t.bottom(0));
    let b = r.tap("M4", "D", t.bottom(0));
    r.route(&[a, b]);
    let up = r.centre_tap("M2", "D");
    r.route(&[up, b]);
    r.label("out", a);
    r.rail("vdd", t.top(1), &[("M1", "S")]);
    r.rail("gnd", t.bottom(1), &[("M3", "S"), ("M4", "S")]);
    full_flow_cell("nor2", "2-input CMOS NOR, routed PCell layout (DRC / LVS clean)", s, r.finish(), sim)
}

const RING_STAGES: usize = 5;

pub fn ring_osc5(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    for k in 0..RING_STAGES as i32 {
        let x = 8 * k;
        mos(&mut s, DeviceKind::Pmos, (x + 4, 4), false, "2u", "180n", None);
        mos(&mut s, DeviceKind::Nmos, (x + 4, 10), false, "1u", "180n", None);
        wires(&mut s, &[(x + 5, 6), (x + 5, 8)]);
        wires(&mut s, &[(x + 2, 4), (x + 2, 10)]);
        if k + 1 < RING_STAGES as i32 {
            wires(&mut s, &[(x + 5, 7), (x + 10, 7)]);
        }
        s.add_label(format!("n{}", k + 1), Pt::new(x + 7, 7));
    }
    let last = 8 * (RING_STAGES as i32 - 1) + 5;
    wires(&mut s, &[(last, 7), (last + 3, 7), (last + 3, 14), (0, 14), (0, 7), (2, 7)]);
    wires(&mut s, &[(5, 2), (last, 2)]);
    wires(&mut s, &[(5, 12), (last, 12)]);
    s.add_instance(DeviceKind::Supply, Pt::new(21, 2), o);
    s.add_instance(DeviceKind::Ground, Pt::new(21, 12), o);
    source(&mut s, (-6, 8), "vdd", &[("dc", "1.8")]);
    let kick = s.add_instance(DeviceKind::ISource, Pt::new(-2, 10), Orient { rot: 2, mirror: false });
    set(&mut s, kick, &[("dc", "0"), ("tran", "PULSE(0 200u 0 10p 10p 100p 1)")]);
    s.add_instance(DeviceKind::Ground, Pt::new(-2, 12), o);
    s.add_label("n1", Pt::new(-2, 8));
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "1p", "1500p");

    let t = Tracks { wn: NW.0, wp: PW.0, mid: 3 };
    let mut r = Router::new(&p.tech);
    let names: Vec<(String, String)> = (0..RING_STAGES).map(|k| (format!("M{}", 2 * k + 1), format!("M{}", 2 * k + 2))).collect();
    let cols: Vec<_> = names.iter().map(|(pm, nm)| (pm.as_str(), PW, nm.as_str(), NW)).collect();
    place_columns(&mut r, p, t, GATE_COL, &cols);
    for k in 0..RING_STAGES {
        let ((pm, nm), (next_p, next_n)) = (&names[k], &names[(k + 1) % RING_STAGES]);
        let track = if k + 1 == RING_STAGES { 2 } else { k as i64 % 2 };
        let nd = r.centre_tap(nm, "D");
        let pd = r.centre_tap(pm, "D");
        let joint = (nd.0, t.mid(track));
        r.route(&[nd, joint]);
        r.route(&[joint, pd]);
        let g = r.tap(next_n, "G", t.mid(track));
        r.tap(next_p, "G", t.mid(track));
        r.route(&[joint, g]);
        r.label(&format!("n{}", k + 1), joint);
    }
    let ps: Vec<(&str, &str)> = names.iter().map(|(pm, _)| (pm.as_str(), "S")).collect();
    let ns: Vec<(&str, &str)> = names.iter().map(|(_, nm)| (nm.as_str(), "S")).collect();
    r.rail("vdd", t.top(0), &ps);
    r.rail("gnd", t.bottom(0), &ns);
    full_flow_cell("ring_osc5", "5-stage ring oscillator, fully routed (DRC / LVS clean)", s, r.finish(), sim)
}

pub fn current_mirror(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    mos(&mut s, DeviceKind::Nmos, (10, 8), true, "2u", "1u", None);
    mos(&mut s, DeviceKind::Nmos, (16, 8), false, "2u", "1u", None);
    wires(&mut s, &[(12, 8), (14, 8)]);
    wires(&mut s, &[(9, 6), (9, 5), (13, 5), (13, 8)]);
    wires(&mut s, &[(9, 10), (17, 10)]);
    s.add_instance(DeviceKind::Ground, Pt::new(13, 10), o);
    s.add_label("ref", Pt::new(11, 5));
    let iref = s.add_instance(DeviceKind::ISource, Pt::new(9, 2), o);
    set(&mut s, iref, &[("dc", "20u")]);
    wires(&mut s, &[(9, 4), (9, 5)]);
    s.add_instance(DeviceKind::Supply, Pt::new(9, 0), o);
    wires(&mut s, &[(17, 6), (17, 3), (22, 3)]);
    s.add_label("out", Pt::new(20, 3));
    let vout = s.add_instance(DeviceKind::VSource, Pt::new(22, 5), o);
    set(&mut s, vout, &[("dc", "0.9")]);
    s.add_instance(DeviceKind::Ground, Pt::new(22, 7), o);
    source(&mut s, (2, 8), "vdd", &[("dc", "1.8")]);
    let mut sim = sim(&["op", "dc"]);
    if let Some(Analysis::Dc { source, .. }) = sim.get_mut("dc") {
        *source = "V1".into();
    }

    let w = (2000, 1000);
    let t = Tracks { wn: w.0, wp: 0, mid: 1 };
    let mut r = Router::new(&p.tech);
    place_columns(&mut r, p, t, 3000, &[("", w, "M1", w), ("", w, "M2", w)]);
    let g1 = r.tap("M1", "G", t.mid(0));
    let g2 = r.tap("M2", "G", t.mid(0));
    r.route(&[g1, g2]);
    let d1 = r.centre_tap("M1", "D");
    r.route(&[d1, (d1.0, t.mid(0))]);
    r.label("ref", g1);
    let out = r.tap("M2", "D", t.bottom(0));
    let pin = (r.span().1 + 600, out.1);
    r.route(&[out, pin]);
    r.label("out", pin);
    r.rail("gnd", t.bottom(1), &[("M1", "S"), ("M2", "S")]);
    full_flow_cell("current_mirror", "NMOS current mirror, 20 µA reference (DRC / LVS clean)", s, r.finish(), sim)
}

pub fn ota5t(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    mos(&mut s, DeviceKind::Nmos, (8, 10), false, "2u", "500n", None);
    mos(&mut s, DeviceKind::Nmos, (18, 10), true, "2u", "500n", None);
    mos(&mut s, DeviceKind::Pmos, (10, 4), true, "4u", "500n", None);
    mos(&mut s, DeviceKind::Pmos, (16, 4), false, "4u", "500n", None);
    mos(&mut s, DeviceKind::Nmos, (12, 15), false, "2u", "1u", None);
    wires(&mut s, &[(9, 2), (17, 2)]);
    s.add_instance(DeviceKind::Supply, Pt::new(13, 2), o);
    wires(&mut s, &[(12, 4), (14, 4)]);
    wires(&mut s, &[(13, 4), (13, 7), (9, 7)]);
    wires(&mut s, &[(9, 6), (9, 8)]);
    wires(&mut s, &[(17, 6), (17, 8)]);
    wires(&mut s, &[(17, 7), (22, 7)]);
    wires(&mut s, &[(9, 12), (9, 13), (17, 13), (17, 12)]);
    s.add_instance(DeviceKind::Ground, Pt::new(13, 17), o);
    s.add_label("x", Pt::new(11, 7));
    s.add_label("out", Pt::new(22, 7));
    s.add_label("tail", Pt::new(11, 13));
    stub(&mut s, (6, 10), -2, "inp");
    stub(&mut s, (20, 10), 2, "inn");
    stub(&mut s, (10, 15), -2, "vb");
    source(&mut s, (-10, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-6, 8), "inp", &[("dc", "0.9"), ("ac", "1")]);
    source(&mut s, (-2, 8), "inn", &[("dc", "0.9")]);
    source(&mut s, (-6, 15), "vb", &[("dc", "0.65")]);
    let mut sim = sim(&["op", "ac"]);
    if let Some(Analysis::Ac { fstart, fstop, .. }) = sim.get_mut("ac") {
        *fstart = "1k".into();
        *fstop = "10g".into();
    }

    // Verification: gain, unity-gain frequency and phase margin from -40 to 125 °C.
    sim.sweep.temps = "-40 27 125".into();
    let spec = |name: &str, expr: &str, min: &str, max: &str| crate::sweep::Spec {
        name: name.into(),
        analysis: "ac".into(),
        expr: expr.into(),
        min: min.into(),
        max: max.into(),
    };
    sim.sweep.specs = vec![
        spec("gain_db", "max(db(v(out)))", "30", ""),
        spec("ugf", "ugf(v(out))", "100meg", ""),
        spec("pm", "phasemargin(v(out))", "45", ""),
    ];

    let (n, pw, tail) = ((2000, 500), (4000, 500), (2000, 1000));
    let t = Tracks { wn: n.0, wp: pw.0, mid: 2 };
    let mut r = Router::new(&p.tech);
    place_columns(&mut r, p, t, 2600, &[("M3", pw, "M1", n), ("M4", pw, "M2", n), ("", pw, "M5", tail)]);
    for (dev, net) in [("M1", "inp"), ("M2", "inn"), ("M5", "vb")] {
        let g = r.tap(dev, "G", t.mid(0));
        r.label(net, g);
    }
    let a = r.tap("M3", "G", t.mid(1));
    let b = r.tap("M4", "G", t.mid(1));
    r.route(&[a, b]);
    let d1 = r.centre_tap("M1", "D");
    let d3 = r.centre_tap("M3", "D");
    let joint = (d1.0, t.mid(1));
    r.route(&[d1, joint]);
    r.route(&[joint, d3]);
    r.label("x", joint);
    let o2 = r.centre_tap("M2", "D");
    let o4 = r.centre_tap("M4", "D");
    r.route(&[o2, o4]);
    r.label("out", o2);
    let s1 = r.tap("M1", "S", t.bottom(0));
    r.tap("M2", "S", t.bottom(0));
    let d5 = r.tap("M5", "D", t.bottom(0));
    r.route(&[s1, d5]);
    r.label("tail", s1);
    r.rail("vdd", t.top(0), &[("M3", "S"), ("M4", "S")]);
    r.rail("gnd", t.bottom(1), &[("M5", "S")]);
    full_flow_cell("ota5t", "5-transistor OTA: NMOS pair, PMOS mirror load (DRC / LVS clean)", s, r.finish(), sim)
}

/// Thin-cell 6T SRAM bit cell (dbu): the lithography-friendly layout of 90 nm and later
/// processes. Diffusions run vertically (N | P P | N, the PMOS in a central nwell), every gate is
/// a straight horizontal poly on one of two tracks, and the cell is point-symmetric about its
/// centre: the left half (pass gate PG1, pull-down PD1, pull-up PU1) rotated by 180° is the
/// right half. The array mirrors cells in x and y so neighbours share the contacts on the cell
/// edges (bit line, supply, ground) and the word-line poly contacts.
const SRAM_W: i64 = 4620;
const SRAM_H: i64 = 1840;
/// x of the left NMOS diffusion (bit line), the left PMOS diffusion and the left ground line.
const SRAM_XA: i64 = 640;
const SRAM_XB: i64 = 1960;
const SRAM_XG: i64 = 1300;
/// y of the lower gate track; the upper track is at `SRAM_H - SRAM_LO`, the storage nodes at
/// `SRAM_H / 2`.
const SRAM_LO: i64 = 400;
const SRAM_ROWS: i64 = 8;
const SRAM_COLS: i64 = 8;

/// 6T SRAM bit cell: cross-coupled inverters (P1/N1, P2/N2) and access devices A1, A2, drawn as
/// a thin cell (see [`SRAM_W`]). Bit lines, supply and ground run vertically on metal2, the word
/// line and the ground rails (on both cell edges) horizontally on metal3.
pub fn sram6t(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    // Textbook drawing (Weste & Harris, CMOS VLSI Design, 4th ed., fig. 12.3): the inverters
    // P1 / N1 (output q on the left) and P2 / N2 (output qb on the right) between the bit lines,
    // pass gates A1, A2 on the outside with their gates on the word line. Pull-down 0.6 µm, pass
    // gate and pull-up 0.42 µm: cell ratio 1.4 for read stability.
    let l = "180n";
    let a1 = s.add_instance(DeviceKind::Nmos, Pt::new(1, 8), R90);
    let a2 = s.add_instance(DeviceKind::Nmos, Pt::new(29, 8), R90);
    for a in [a1, a2] {
        set(&mut s, a, &[("w", "420n"), ("l", l)]);
        let b = s.instance(a).expect("instance").pin_positions()[3].1;
        s.add_instance(DeviceKind::Ground, b, o);
    }
    mos(&mut s, DeviceKind::Pmos, (10, 6), true, "420n", l, None);
    mos(&mut s, DeviceKind::Nmos, (10, 12), true, "600n", l, None);
    mos(&mut s, DeviceKind::Pmos, (20, 6), false, "420n", l, None);
    mos(&mut s, DeviceKind::Nmos, (20, 12), false, "600n", l, None);
    wires(&mut s, &[(9, 4), (21, 4)]);
    s.add_instance(DeviceKind::Supply, Pt::new(15, 4), o);
    for x in [9, 21] {
        s.add_instance(DeviceKind::Ground, Pt::new(x, 14), o);
    }
    // Inverter gates and outputs.
    wires(&mut s, &[(12, 6), (12, 12)]);
    wires(&mut s, &[(18, 6), (18, 12)]);
    wires(&mut s, &[(9, 8), (9, 10)]);
    wires(&mut s, &[(21, 8), (21, 10)]);
    // Pass gates: bit line to q, qb to the complement bit line, gates on the word line.
    wires(&mut s, &[(3, 9), (9, 9)]);
    wires(&mut s, &[(21, 9), (27, 9)]);
    wires(&mut s, &[(1, 6), (1, 1), (29, 1), (29, 6)]);
    wires(&mut s, &[(-3, 0), (-3, 18)]);
    wires(&mut s, &[(-3, 9), (-1, 9)]);
    wires(&mut s, &[(33, 0), (33, 18)]);
    wires(&mut s, &[(31, 9), (33, 9)]);
    // Cross-coupling under the cell: q to the P2 / N2 gates, qb to the P1 / N1 gates.
    wires(&mut s, &[(6, 9), (6, 16), (18, 16), (18, 12)]);
    wires(&mut s, &[(24, 9), (24, 17), (12, 17), (12, 12)]);
    for (net, (x, y)) in [("wl", (15, 1)), ("bl", (-3, 0)), ("blb", (33, 0)), ("q", (8, 9)), ("qb", (22, 9))] {
        s.add_label(net, Pt::new(x, y));
    }
    source(&mut s, (-36, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-28, 8), "wl", &[("dc", "0"), ("tran", "PULSE(0 1.8 1n 50p 50p 1n 4n)")]);
    source(&mut s, (-20, 8), "bl", &[("dc", "0"), ("tran", "PULSE(0 1.8 2.5n 50p 50p 4n 8n)")]);
    source(&mut s, (-12, 8), "blb", &[("dc", "1.8"), ("tran", "PULSE(1.8 0 2.5n 50p 50p 4n 8n)")]);
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "10p", "8n");

    let (w, h, m) = (SRAM_W, SRAM_H, SRAM_H / 2);
    let (xa, xb, xg, lo) = (SRAM_XA, SRAM_XB, SRAM_XG, SRAM_LO);
    let (xd, hi, xv) = (w - xb, h - lo, w / 2);
    let mut r = Router::new(&p.tech);
    let l = |name: &str| p.tech.layer_index(name).expect("generic layer");
    let (active, poly, m1, m2, m3) = (l("active"), l("poly"), l("metal1"), l("metal2"), l("metal3"));
    let (nwell, nplus, pplus, boundary) = (l("nwell"), l("nplus"), l("pplus"), l("boundary"));
    // The left half; each shape and via is drawn again rotated by 180° about the centre.
    let half = [
        (active, IRect::new(xa - 300, -210, xa + 300, m)),       // PD1 (W 0.6)
        (active, IRect::new(xa - 210, m, xa + 210, h + 210)),    // PG1 (W 0.42)
        (active, IRect::new(xb - 210, -210, xb + 210, m + 210)), // PU1
        (poly, IRect::new(xa - 520, lo - 90, xd, lo + 90)),      // inverter 1 gate, to its contact at qb
        (poly, IRect::new(0, hi - 90, xa + 430, hi + 90)),       // PG1 gate, word-line contact shared
        (m1, IRect::new(xa, -170, xg, 170)),                     // ground
        (m1, IRect::new(xa, m - 170, xb, m + 170)),              // q
        (m1, IRect::new(xb - 170, m, xb + 170, hi)),             // q to the inverter 2 gate
        (m1, IRect::new(-170, m, 170, hi)),                      // word line
        (m2, IRect::new(xa - 150, 0, xa + 150, h)),              // bit line
        (m2, IRect::new(xg - 150, 0, xg + 150, h)),              // ground
        (m2, IRect::new(xb, -190, xv, 190)),                     // supply jog
        (nplus, IRect::new(0, -390, xb - 640, h + 390)),
    ];
    let vias: [(&'static str, (i64, i64)); 14] = [
        ("contact", (xa, 0)),
        ("contact", (xa, m)),
        ("contact", (xa, h)),
        ("contact", (xb, 0)),
        ("contact", (xb, m)),
        ("polycon", (0, hi)),
        ("polycon", (xd, lo)),
        ("via1", (0, m)),
        ("via2", (0, m)),
        ("via1", (xa, h)),
        ("via1", (xb, 0)),
        ("via1", (xg, 0)),
        ("via2", (xg, 0)),
        ("via2", (xg, h)),
    ];
    for (layer, b) in half {
        r.lay.add(layer, Geometry::Rect(b));
        r.lay.add(layer, Geometry::Rect(IRect::new(w - b.x1, h - b.y1, w - b.x0, h - b.y0)));
    }
    for (name, (x, y)) in vias {
        r.via(name, (x, y));
        r.via(name, (w - x, h - y));
    }
    r.lay.add_rect(nwell, xb - 640, -640, xd + 640, h + 640);
    r.lay.add_rect(pplus, xb - 640, -390, xd + 640, h + 390);
    r.lay.add_rect(m2, xv - 150, 0, xv + 150, h);
    for y in [0, m, h] {
        r.lay.add_rect(m3, 0, y - 190, w, y + 190);
    }
    let q = (m + hi) / 2;
    for (net, at) in [
        ("q", (xb, q)),
        ("qb", (xd, h - q)),
        ("bl", (xa, 460)),
        ("blb", (w - xa, h - 460)),
        ("wl", (xv, m)),
        ("vdd", (xv, 460)),
        ("gnd", (xv, 0)),
    ] {
        r.label(net, at);
    }
    r.lay.add_rect(boundary, 0, 0, w, h);
    full_flow_cell("sram6t", "6T SRAM thin cell, 4.62 x 1.84 µm, mirrored for arrays (DRC / LVS clean)", s, r.finish(), sim)
}

/// 8 x 8 array of `sram6t`, mirrored in x and y (four AREFs of 2 x 2 pitch) so neighbours share
/// edge contacts: labelled word lines and bit lines, the ground mesh and a supply rail joining
/// the supply columns.
pub fn sram_array(tech: &Tech, lib: &str) -> Cell {
    let (w, h) = (SRAM_W, SRAM_H);
    let mut r = Router::new(tech);
    let spec = Some(ArraySpec { cols: (SRAM_COLS / 2) as u32, rows: (SRAM_ROWS / 2) as u32, col: (2 * w, 0), row: (0, 2 * h) });
    for t in
        [Trans::new(0, 0), Trans::with(2 * w, 0, 2, true), Trans::with(0, 2 * h, 0, true), Trans::with(2 * w, 2 * h, 2, false)]
    {
        r.lay.add_instance(CellRef::new(lib, "sram6t"), t, spec);
    }
    let (m2, m3) = (r.layer("metal2"), r.layer("metal3"));
    let rail = -800;
    for col in 0..SRAM_COLS {
        let x = col * w + w / 2;
        r.lay.add_rect(m2, x - 150, rail, x + 150, 0);
        r.via("via2", (x, rail));
        let (bl, blb) = if col % 2 == 0 { (SRAM_XA, w - SRAM_XA) } else { (w - SRAM_XA, SRAM_XA) };
        r.label(&format!("bl{col}"), (col * w + bl, 460));
        r.label(&format!("blb{col}"), (col * w + blb, 460));
    }
    r.lay.add_rect(m3, w / 2 - 190, rail - 190, (SRAM_COLS - 1) * w + w / 2 + 190, rail + 190);
    r.label("vdd", (w / 2, rail));
    r.label("gnd", (SRAM_XG, 460));
    for row in 0..SRAM_ROWS {
        r.label(&format!("wl{row}"), (w / 2, row * h + h / 2));
    }
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, 0, rail - 400, SRAM_COLS * w, SRAM_ROWS * h);
    layout_cell("sram_array", "8 x 8 array of 6T SRAM thin cells, mirrored AREFs (37 x 15 µm)", r.finish())
}

/// Planar 1T1C DRAM cell (dbu): 1.28 x 2.0 µm (2.56 µm², 79 F² at F = 0.18 µm; the MoSys
/// 1T-SRAM planar cell of the same node is 1.97 µm²). The cell is mirrored in y for arrays, so
/// two cells share a bit-line contact and two share the plate.
const DRAM_W: i64 = 1280;
const DRAM_H: i64 = 2000;
/// Storage capacitor: the plate over 1.0 x 1.17 µm of diffusion, ≈ 10 fF at 8.6 fF/µm² (4 nm
/// gate oxide).
const DRAM_CAP: IRect = IRect { x0: 140, y0: 690, x1: 1140, y1: DRAM_H - 140 };
const DRAM_ROWS: i64 = 8;
const DRAM_COLS: i64 = 8;

/// Piecewise-linear source: `(time ns, value)` steps with `edge` ns transitions.
fn pwl(steps: &[(f64, f64)], edge: f64) -> String {
    let mut pts = vec![(0.0, steps[0].1)];
    for w in steps.windows(2) {
        pts.push((w[1].0, w[0].1));
        pts.push((w[1].0 + edge, w[1].1));
    }
    let body: Vec<String> = pts.iter().map(|(t, v)| format!("{t}n {v}")).collect();
    format!("PWL({})", body.join(" "))
}

/// Planar one-transistor one-capacitor DRAM cell (Dennard, US patent 3,387,286, 1968), as made
/// in a logic process without a dedicated capacitor (MoSys 1T-SRAM: Leung, Hsu and Jones, "The
/// ideal SoC memory: 1T-SRAM", IEEE ASIC/SOC Conf. 2000; US patent 6,468,855). The storage
/// capacitor MCS is a MOS capacitor: a poly plate over the diffusion beyond the access device
/// MA, the inversion layer under it joined to the storage node `sn`. The plate sits at VPP so
/// the capacitor stays inverted over the whole stored range, and the word line is boosted to
/// VPP = VDD + Vt(body) + margin so a full VDD is written.
///
/// The test bench writes a 1, precharges the bit line to VDD / 2 and reads it by charge
/// sharing (ΔV = (VDD / 2) · Cs / (Cs + CBL), about ±150 mV with Cs ≈ 10 fF and a short 50 fF
/// logic bit line), then does the same for a 0. The bit-line load CBL and the write / precharge
/// switch MPRE are test-bench parts (`lvsIgnore`). The read is destructive: a sense amplifier
/// would restore the cell.
pub fn dram1t1c(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    let (cw, cl) = (format!("{}n", DRAM_CAP.width()), format!("{}n", DRAM_CAP.height()));
    let ma = mos(&mut s, DeviceKind::Nmos, (10, 6), false, "420n", "180n", None);
    let cs = mos(&mut s, DeviceKind::Nmos, (12, 12), true, &cw, &cl, None);
    // Bit line to the access device; the storage node joins both capacitor terminals.
    wires(&mut s, &[(-1, 2), (11, 2), (11, 4)]);
    wires(&mut s, &[(11, 8), (11, 10)]);
    wires(&mut s, &[(11, 14), (11, 16), (4, 16), (4, 9), (11, 9)]);
    stub(&mut s, (8, 6), -2, "wl");
    stub(&mut s, (14, 12), 2, "plate");
    // Test bench: bit-line capacitance and the write / precharge switch from the driver.
    let cbl = s.add_instance(DeviceKind::Capacitor, Pt::new(4, 4), o);
    set(&mut s, cbl, &[("c", "50f"), (crate::schematic::LVS_IGNORE, "t")]);
    s.add_instance(DeviceKind::Ground, Pt::new(4, 6), o);
    let pre = s.add_instance(DeviceKind::Nmos, Pt::new(-3, 1), R90);
    set(&mut s, pre, &[("w", "1u"), ("l", "180n"), (crate::schematic::LVS_IGNORE, "t")]);
    s.add_instance(DeviceKind::Ground, Pt::new(-3, 3), o);
    for (id, name) in [(ma, "MA"), (cs, "MCS"), (pre, "MPRE"), (cbl, "CBL")] {
        s.instance_mut(id).expect("instance").name = name.into();
    }
    stub(&mut s, (-5, 2), -2, "drv");
    wires(&mut s, &[(-3, -1), (-3, -2)]);
    for (net, (x, y)) in [("pre", (-3, -2)), ("bl", (1, 2)), ("sn", (8, 9))] {
        s.add_label(net, Pt::new(x, y));
    }
    // Write 1 (1-5 ns), precharge to VDD / 2, read (12-16 ns); write 0 (21-25 ns), read (32-36 ns).
    let (vpp, vdd, half) = (2.7, 1.8, 0.9);
    let wl = pwl(
        &[(0.0, 0.0), (1.0, vpp), (5.0, 0.0), (12.0, vpp), (16.0, 0.0), (21.0, vpp), (25.0, 0.0), (32.0, vpp), (36.0, 0.0)],
        0.1,
    );
    let drv = pwl(&[(0.0, vdd), (6.0, half), (20.0, 0.0), (26.0, half)], 0.1);
    // The switch turns off slowly, so its channel charge flows back to the driver.
    let pre = pwl(&[(0.0, vpp), (9.0, 0.0), (20.0, vpp), (29.0, 0.0)], 2.0);
    source(&mut s, (-28, 8), "wl", &[("dc", "0"), ("tran", &wl)]);
    source(&mut s, (-20, 8), "drv", &[("dc", "1.8"), ("tran", &drv)]);
    source(&mut s, (-12, 8), "pre", &[("dc", "2.7"), ("tran", &pre)]);
    source(&mut s, (-36, 8), "plate", &[("dc", "2.7")]);
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "20p", "40n");

    let (w, h, xc) = (DRAM_W, DRAM_H, DRAM_W / 2);
    let mut r = Router::new(&p.tech);
    let l = |name: &str| p.tech.layer_index(name).expect("generic layer");
    let (active, poly, m1, nplus, boundary) = (l("active"), l("poly"), l("metal1"), l("nplus"), l("boundary"));
    for (layer, b) in [
        (active, IRect::new(xc - 210, -210, xc + 210, 570)), // access device, W 0.42
        (active, IRect::new(DRAM_CAP.x0, 570, DRAM_CAP.x1, DRAM_CAP.y1)), // storage node and capacitor
        (poly, IRect::new(0, 270, w, 450)),                  // word line, L 0.18
        (poly, IRect::new(-80, DRAM_CAP.y0, w + 80, h + 80)), // plate, shared with the mirrored cell
        (m1, IRect::new(xc - 170, -170, xc + 170, h)),       // bit line
        (nplus, IRect::new(0, -390, w, h)),
        (boundary, IRect::new(0, 0, w, h)),
    ] {
        r.lay.add(layer, Geometry::Rect(b));
    }
    r.via("contact", (xc, 0));
    for (net, at) in [("bl", (xc, 1000)), ("wl", (200, 360)), ("plate", (200, 1300)), ("sn", (300, 630))] {
        r.label(net, at);
    }
    full_flow_cell(
        "dram1t1c",
        "Planar 1T1C DRAM cell, 1.28 x 2.0 µm, MOS storage capacitor ≈ 10 fF (DRC / LVS clean)",
        s,
        r.finish(),
        sim,
    )
}

/// 8 x 8 array of `dram1t1c`: rows mirrored in y (two AREFs) so cell pairs share the bit-line
/// contact and the plate; bit lines on metal1, word lines in poly, the plates joined by a
/// metal1 strap at the left edge.
pub fn dram_array(tech: &Tech, lib: &str) -> Cell {
    let (w, h) = (DRAM_W, DRAM_H);
    let mut r = Router::new(tech);
    let spec = Some(ArraySpec { cols: DRAM_COLS as u32, rows: (DRAM_ROWS / 2) as u32, col: (w, 0), row: (0, 2 * h) });
    for t in [Trans::new(0, 0), Trans::with(0, 2 * h, 0, true)] {
        r.lay.add_instance(CellRef::new(lib, "dram1t1c"), t, spec);
    }
    let (poly, m1) = (r.layer("poly"), r.layer("metal1"));
    let strap = -600;
    for pair in 0..DRAM_ROWS / 2 {
        let y = (2 * pair + 1) * h;
        r.lay.add_rect(poly, strap - 210, y - 210, 0, y + 210);
        r.via("polycon", (strap, y));
    }
    r.lay.add_rect(m1, strap - 170, h - 170, strap + 170, (DRAM_ROWS - 1) * h + 170);
    r.label("plate", (strap, h));
    for row in 0..DRAM_ROWS {
        let y = if row % 2 == 0 { row * h + 360 } else { (row + 1) * h - 360 };
        r.label(&format!("wl{row}"), (200, y));
    }
    for col in 0..DRAM_COLS {
        r.label(&format!("bl{col}"), (col * w + w / 2, 1000));
    }
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, strap - 400, 0, DRAM_COLS * w, DRAM_ROWS * h);
    layout_cell("dram_array", "8 x 8 planar DRAM array, cell pairs sharing contacts and plates (AREF)", r.finish())
}

/// NAND flash string (dbu): one column 0.9 µm wide and 4.64 µm long (bit-line contact to
/// source-line contact), mirrored in y for arrays so strings share both contacts. The cell is
/// 0.9 x 0.42 µm (0.38 µm², 11.7 F²): the generic rules (poly space 0.24 µm, poly past active
/// 0.22 µm) set it, where a flash process has core rules for its self-aligned 4 F² cell.
const NAND_W: i64 = 900;
/// Channel width of the string: 0.22 µm, widened to 0.42 µm at the contacts.
const NAND_ACT: i64 = 220;
const NAND_H: i64 = 4640;
const NAND_CELLS: i64 = 8;
/// Floating gate: 0.66 µm across the 0.22 µm channel (wings over the field for coupling) by
/// 0.18 µm, self-aligned under the poly2 word line.
const NAND_FG: (i64, i64) = (660, 180);
/// y of the lower edge of word line 0; word lines on a 0.42 µm pitch.
const NAND_WL0: i64 = 760;
const NAND_WL_PITCH: i64 = 420;
const NAND_ROWS: i64 = 4;
const NAND_COLS: i64 = 8;
/// Stored floating-gate voltage Q_FG / C_T (all terminals at 0 V) per cell, word line 0 first:
/// programmed cells (-0.8 V, threshold ≈ 2.4 V from the control gate) on word lines 1, 3 and 6,
/// the others erased (+1.2 V, threshold ≈ -0.8 V).
const NAND_FG_V: [f64; 8] = [1.2, -0.8, 1.2, -0.8, 1.2, 1.2, -0.8, 1.2];

/// NAND flash string after Masuoka et al., "New ultra high density EPROM and flash EEPROM with
/// NAND structure cell", IEDM 1987: eight floating-gate cells in series between the string
/// select gate SSL (to the bit line) and the ground select gate GSL (to the common source line
/// `csl`). Each cell is drawn as its capacitive equivalent circuit (Pavan, Bez, Olivo and
/// Zanoni, "Flash memory cells: an overview", Proc. IEEE 85 (8), 1997): a transistor on the
/// floating gate `fgN` (9 nm tunnel oxide, model `nfg`) and the interpoly ONO capacitor from the
/// word line, both extracted from the layout (floating gate in poly over the channel, word line
/// in poly2 on top). The coupling ratio is α_G = C_ONO / C_T ≈ 0.6. The stored charge is the
/// initial floating-gate voltage (`.ic`, see [`NAND_FG_V`]); the cell model is BSIM4 with its
/// charge-conserving capacitances, so a floating gate keeps its charge through any sequence and
/// follows its terminals: V_FG = α_G V_CG + α_D V_D + α_S V_S + Q_FG / C_T.
///
/// The test bench reads two cells as a NAND page read does (selected word line at 0 V, the
/// others, SSL and GSL at V_read = 4.5 V, the bit line precharged to 1 V): word line 3 is
/// programmed, so the string stays off and the bit line high; word line 4 is erased, so the
/// string conducts and discharges it.
pub fn nand_string(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    let fg_area = NAND_FG.0 as f64 * NAND_FG.1 as f64 / 1e6;
    let cono = format!("{:.5}f", p.tech.capacitors[0].area * fg_area);
    // SSL on top, word lines 7 to 0, GSL at the bottom; the bulks share one wire.
    for i in 0..(NAND_CELLS + 2) as i32 {
        let y = 4 + 4 * i;
        let id = s.add_instance(DeviceKind::Nmos, Pt::new(10, y), o);
        let (name, l, gate) = match i {
            0 => ("MSSL".to_string(), "250n", "ssl".to_string()),
            9 => ("MGSL".to_string(), "250n", "gsl".to_string()),
            _ => (format!("MC{}", 8 - i), "180n", String::new()),
        };
        set(&mut s, id, &[("w", &format!("{NAND_ACT}n")), ("l", l)]);
        s.instance_mut(id).expect("instance").name = name;
        if gate.is_empty() {
            let k = 8 - i;
            set(&mut s, id, &[("model", "nfg")]);
            let c = s.add_instance(DeviceKind::Capacitor, Pt::new(6, y), R90);
            set(&mut s, c, &[("c", &cono)]);
            s.instance_mut(c).expect("instance").name = format!("CONO{k}");
            s.add_label(format!("fg{k}"), Pt::new(8, y));
            stub(&mut s, (4, y), -2, &format!("wl{k}"));
        } else {
            stub(&mut s, (8, y), -6, &gate);
        }
    }
    wires(&mut s, &[(12, 4), (12, 40)]);
    s.add_instance(DeviceKind::Ground, Pt::new(12, 40), o);
    wires(&mut s, &[(11, 42), (11, 44)]);
    s.add_label("csl", Pt::new(11, 44));
    // Test bench: bit-line capacitance and the precharge switch from a 1 V driver.
    wires(&mut s, &[(11, 2), (11, 0), (20, 0)]);
    let cbl = s.add_instance(DeviceKind::Capacitor, Pt::new(16, 2), o);
    set(&mut s, cbl, &[("c", "100f"), (crate::schematic::LVS_IGNORE, "t")]);
    s.add_instance(DeviceKind::Ground, Pt::new(16, 4), o);
    let pre = s.add_instance(DeviceKind::Nmos, Pt::new(22, -1), R90);
    set(&mut s, pre, &[("w", "1u"), ("l", "180n"), (crate::schematic::LVS_IGNORE, "t")]);
    s.add_instance(DeviceKind::Ground, Pt::new(22, 1), o);
    for (id, name) in [(cbl, "CBL"), (pre, "MPRE")] {
        s.instance_mut(id).expect("instance").name = name.into();
    }
    stub(&mut s, (24, 0), 2, "drv");
    wires(&mut s, &[(22, -3), (22, -4)]);
    for (net, (x, y)) in [("pre", (22, -4)), ("bl", (11, 0))] {
        s.add_label(net, Pt::new(x, y));
    }
    // Precharge (0-5 ns, 47-52 ns), read word line 3 (6-46 ns), read word line 4 (53-93 ns):
    // an erased cell's string carries a few µA, as in NAND, so a read takes tens of ns here
    // (µs on a real, pF bit line).
    let vread = 4.5;
    let both = pwl(&[(0.0, 0.0), (6.0, vread), (46.0, 0.0), (53.0, vread), (93.0, 0.0)], 0.2);
    let first = pwl(&[(0.0, 0.0), (6.0, vread), (46.0, 0.0)], 0.2);
    let second = pwl(&[(0.0, 0.0), (53.0, vread), (93.0, 0.0)], 0.2);
    let mut n = 0;
    let mut src = |s: &mut Schematic, net: &str, kv: &[(&str, &str)]| {
        source(s, (-54 + 7 * (n % 7), 10 + 14 * (n / 7)), net, kv);
        n += 1;
    };
    src(&mut s, "drv", &[("dc", "1")]);
    src(&mut s, "csl", &[("dc", "0")]);
    let pre = pwl(&[(0.0, 2.7), (3.0, 0.0), (47.0, 2.7), (50.0, 0.0)], 2.0);
    src(&mut s, "pre", &[("dc", "2.7"), ("tran", &pre)]);
    src(&mut s, "ssl", &[("dc", "0"), ("tran", &both)]);
    src(&mut s, "gsl", &[("dc", "0"), ("tran", &both)]);
    for k in 0..NAND_CELLS {
        let wave = match k {
            3 => &second,
            4 => &first,
            _ => &both,
        };
        src(&mut s, &format!("wl{k}"), &[("dc", "0"), ("tran", wave)]);
    }
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "50p", "96n");
    let ic: Vec<String> = NAND_FG_V.iter().enumerate().map(|(k, v)| format!("v(fg{k})={v}")).collect();
    sim.extra = format!(
        "* Floating-gate cell: 9 nm tunnel oxide, BSIM4 with its charge-conserving capacitance model\n\
         .model nfg nmos (level=54 version=4.8 capmod=2 toxe=9n toxp=9n toxm=9n epsrox=3.9 vth0=0.7 k1=0.5 k2=0 \
         ndep=3e17 xj=1.5e-7 u0=0.04 vsat=8e4 rdsw=200 cgso=1e-10 cgdo=1e-10 cgbo=0)\n\
         * Stored charge: floating-gate voltage with every terminal at 0 V\n.ic {}\n",
        ic.join(" ")
    );

    let (w, h, xc) = (NAND_W, NAND_H, NAND_W / 2);
    let mut r = Router::new(&p.tech);
    let l = |name: &str| p.tech.layer_index(name).expect("generic layer");
    let (active, poly, poly2, m1, m2) = (l("active"), l("poly"), l("poly2"), l("metal1"), l("metal2"));
    let (nplus, boundary) = (l("nplus"), l("boundary"));
    let ssl = NAND_WL0 + (NAND_CELLS - 1) * NAND_WL_PITCH + NAND_FG.1 + 240;
    for (layer, b) in [
        (active, IRect::new(xc - NAND_ACT / 2, 0, xc + NAND_ACT / 2, h)),
        (active, IRect::new(xc - 210, -210, xc + 210, 210)), // contacts
        (active, IRect::new(xc - 210, h - 210, xc + 210, h + 210)),
        (poly, IRect::new(0, 270, w, 520)),         // ground select gate, L 0.25
        (poly, IRect::new(0, ssl, w, ssl + 250)),   // string select gate
        (m1, IRect::new(0, -170, w, 170)),          // common source line
        (m2, IRect::new(xc - 190, 0, xc + 190, h)), // bit line
        (nplus, IRect::new(0, -390, w, h + 390)),
        (boundary, IRect::new(0, 0, w, h)),
    ] {
        r.lay.add(layer, Geometry::Rect(b));
    }
    for k in 0..NAND_CELLS {
        let y = NAND_WL0 + k * NAND_WL_PITCH;
        r.lay.add_rect(poly, xc - NAND_FG.0 / 2, y, xc + NAND_FG.0 / 2, y + NAND_FG.1);
        r.lay.add_rect(poly2, 0, y, w, y + NAND_FG.1);
        r.label(&format!("wl{k}"), (60, y + NAND_FG.1 / 2));
    }
    for (name, at) in [("contact", (xc, 0)), ("contact", (xc, h)), ("via1", (xc, h))] {
        r.via(name, at);
    }
    for (net, at) in [("bl", (xc, h / 2)), ("csl", (60, 0)), ("gsl", (60, 395)), ("ssl", (60, ssl + 125))] {
        r.label(net, at);
    }
    full_flow_cell(
        "nand_string",
        "NAND flash string: 8 floating-gate cells with select gates, 0.9 x 4.64 µm (DRC / LVS clean)",
        s,
        r.finish(),
        sim,
    )
}

/// 8 columns x 4 strings of `nand_string`, strings mirrored in y (two AREFs) so pairs share the
/// bit-line and source-line contacts; bit lines on metal2, the source lines joined by a metal2
/// strap at the left edge. Each string row is one block with its own word and select lines.
pub fn nand_array(tech: &Tech, lib: &str) -> Cell {
    let (w, h) = (NAND_W, NAND_H);
    let mut r = Router::new(tech);
    let spec = Some(ArraySpec { cols: NAND_COLS as u32, rows: (NAND_ROWS / 2) as u32, col: (w, 0), row: (0, 2 * h) });
    for t in [Trans::new(0, 0), Trans::with(0, 2 * h, 0, true)] {
        r.lay.add_instance(CellRef::new(lib, "nand_string"), t, spec);
    }
    let (m1, m2) = (r.layer("metal1"), r.layer("metal2"));
    let strap = -600;
    for k in 0..=NAND_ROWS / 2 {
        let y = 2 * k * h;
        r.lay.add_rect(m1, strap - 190, y - 170, 0, y + 170);
        r.via("via1", (strap, y));
    }
    r.lay.add_rect(m2, strap - 190, -190, strap + 190, NAND_ROWS * h + 190);
    r.label("csl", (strap, h));
    let ssl = NAND_WL0 + (NAND_CELLS - 1) * NAND_WL_PITCH + NAND_FG.1 + 240 + 125;
    for row in 0..NAND_ROWS {
        let at = |y: i64| if row % 2 == 0 { row * h + y } else { (row + 1) * h - y };
        for k in 0..NAND_CELLS {
            r.label(&format!("wl{k}_{row}"), (60, at(NAND_WL0 + k * NAND_WL_PITCH + NAND_FG.1 / 2)));
        }
        r.label(&format!("gsl{row}"), (60, at(395)));
        r.label(&format!("ssl{row}"), (60, at(ssl)));
    }
    for col in 0..NAND_COLS {
        r.label(&format!("bl{col}"), (col * w + w / 2, h / 2));
    }
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, strap - 400, 0, NAND_COLS * w, NAND_ROWS * h);
    layout_cell("nand_array", "NAND flash array: 8 bit lines x 4 strings of 8 cells, mirrored (AREF)", r.finish())
}

/// 4T pixel pitch (dbu): 5.6 µm, a common pixel size of 0.18 µm image sensor processes.
const PIX: i64 = 5600;
/// Photodiode: n+ / p-substrate diffusion below the transistor strip, 41 % fill factor.
const PIX_PD: IRect = IRect { x0: 500, y0: 200, x1: 5250, y1: 2900 };
/// y of the transistor strip (column contact, SEL, SF, supply contact, RST, floating diffusion).
const PIX_STRIP: i64 = 4100;
const PIX_ROWS: i64 = 8;
const PIX_COLS: i64 = 8;

/// 4T active pixel (image sensor): transfer gate TX from the photodiode to the floating
/// diffusion `fd`, reset RST, source follower SF and row select SEL. The photodiode fills the
/// lower part of the pixel; TX sits on a neck of diffusion at its top edge, and the other three
/// transistors share one diffusion strip above it (SEL and SF in series, SF and RST sharing the
/// supply contact). The schematic models the photodiode by its photocurrent and its junction
/// (`as` / `ps` of TX). Row lines TX, SEL and RST run horizontally on metal2, the supply on
/// metal3 over the transistors, and the column line vertically on metal1 at the pixel edge.
pub fn aps4t(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    // The 4T pixel of Fossum & Hondongwa, "A review of the pinned photodiode for CCD and CMOS
    // image sensors", IEEE J. Electron Devices Soc. 2 (3), 2014, fig. 6: photodiode, transfer
    // gate TX to the floating diffusion fd, reset RST from the supply, source follower SF and
    // row select SEL onto the column line with its current-source load. The photodiode is its
    // photocurrent and its junction (`as` / `ps` of TX, whose source it is).
    let tx = s.add_instance(DeviceKind::Nmos, Pt::new(6, 10), R90);
    let (a, pe) = (PIX_PD.width() * PIX_PD.height(), 2 * (PIX_PD.width() + PIX_PD.height()));
    let (pd_as, pd_ps) = (format!("{}p", a as f64 / 1e6), format!("{}u", pe as f64 / 1e3));
    set(&mut s, tx, &[("w", "420n"), ("l", "400n"), ("as", &pd_as), ("ps", &pd_ps)]);
    s.add_instance(DeviceKind::Ground, Pt::new(6, 12), o);
    let rst = mos(&mut s, DeviceKind::Nmos, (12, 6), false, "420n", "180n", None);
    // The floating diffusion with its metal: a few fF, so the reset edge kicks it little.
    set(&mut s, rst, &[("as", "3p"), ("ps", "8u")]);
    mos(&mut s, DeviceKind::Nmos, (18, 10), false, "420n", "350n", None);
    mos(&mut s, DeviceKind::Nmos, (18, 15), false, "420n", "180n", None);
    // Photodiode node, transfer to the floating diffusion, which drives the follower's gate.
    wires(&mut s, &[(4, 11), (1, 11), (1, 13)]);
    let ipd = s.add_instance(DeviceKind::ISource, Pt::new(1, 15), o);
    set(&mut s, ipd, &[("dc", "10n")]);
    s.add_instance(DeviceKind::Ground, Pt::new(1, 17), o);
    wires(&mut s, &[(8, 11), (13, 11), (13, 8)]);
    wires(&mut s, &[(13, 10), (16, 10)]);
    // Supply to the reset and follower drains; follower, select and the column line.
    wires(&mut s, &[(13, 4), (19, 4), (19, 8)]);
    s.add_instance(DeviceKind::Supply, Pt::new(16, 4), o);
    wires(&mut s, &[(19, 12), (19, 13)]);
    wires(&mut s, &[(19, 17), (19, 19), (24, 19)]);
    let icol = s.add_instance(DeviceKind::ISource, Pt::new(24, 21), o);
    set(&mut s, icol, &[("dc", "2u")]);
    s.add_instance(DeviceKind::Ground, Pt::new(24, 23), o);
    wires(&mut s, &[(6, 8), (6, 6)]);
    stub(&mut s, (10, 6), -2, "rst");
    stub(&mut s, (16, 15), -2, "sel");
    for (net, (x, y)) in [("tx", (6, 6)), ("pd", (2, 11)), ("fd", (10, 11)), ("x", (19, 12)), ("col", (22, 19))] {
        s.add_label(net, Pt::new(x, y));
    }
    // Reset photodiode and floating diffusion (TX and RST high), integrate, release the reset
    // (reset level on the column), then transfer the charge (signal level).
    source(&mut s, (-30, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-22, 8), "rst", &[("dc", "1.8"), ("tran", "PULSE(1.8 0 300n 1n 1n 400n 800n)")]);
    source(&mut s, (-14, 8), "tx", &[("dc", "1.8"), ("tran", "PULSE(1.8 0 50n 1n 1n 350n 800n)")]);
    source(&mut s, (-6, 8), "sel", &[("dc", "1.8")]);
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "1n", "600n");

    let mut r = Router::new(&p.tech);
    let l = |name: &str| p.tech.layer_index(name).expect("generic layer");
    let (active, poly, m1, m2, m3) = (l("active"), l("poly"), l("metal1"), l("metal2"), l("metal3"));
    let (nplus, boundary) = (l("nplus"), l("boundary"));
    let y = PIX_STRIP;
    let rows = [("tx", y - 870), ("sel", y + 620), ("rst", y + 1240)];
    for (layer, (x0, y0, x1, y1)) in [
        (active, (290, y - 210, 2810, y + 210)),    // transistor strip
        (active, (2390, PIX_PD.y1, 2810, y + 210)), // neck: TX channel and floating diffusion
        (poly, (770, y - 430, 950, y + 580)),       // SEL
        (poly, (1190, y - 600, 1540, y + 430)),     // SF, gate contact below the strip
        (poly, (2080, y - 430, 2260, y + 600)),     // RST
        (poly, (2170, y - 1070, 3200, y - 670)),    // TX, L 0.4 µm
        (m1, (0, 0, 300, PIX)),                     // column line
        (m1, (150, y - 170, 500, y + 170)),
        (m1, (2430, y - 770, 2770, y)), // floating diffusion to the SF gate
        (m1, (1230, y - 770, 2770, y - 430)),
        (m1, (570, y + 580, 910, y + 620)),
        (m1, (2000, y + 600, 2340, y + 1240)),
        (m3, (0, y - 150, PIX, y + 150)), // supply, shielding the transistors
        (nplus, (0, 0, PIX, PIX)),
        (boundary, (0, 0, PIX, PIX)),
    ] {
        r.lay.add_rect(layer, x0, y0, x1, y1);
    }
    r.lay.add(active, Geometry::Rect(PIX_PD));
    for (name, at) in [
        ("contact", (500, y)),
        ("contact", (1810, y)),
        ("contact", (2600, y)),
        ("polycon", (740, y + 580)),
        ("polycon", (1400, y - 600)),
        ("polycon", (2170, y + 600)),
        ("polycon", (3200, y - 870)),
        ("via1", (1810, y)),
        ("via2", (1810, y)),
        ("via1", (740, y + 620)),
        ("via1", (2170, y + 1240)),
        ("via1", (3200, y - 870)),
    ] {
        r.via(name, at);
    }
    for (net, ry) in rows {
        r.lay.add_rect(m2, 0, ry - 150, PIX, ry + 150);
        r.label(net, (3800, ry));
    }
    for (net, at) in [("vdd", (3800, y)), ("col", (150, 1000)), ("fd", (2600, y - 600)), ("pd", (2800, 1500))] {
        r.label(net, at);
    }
    full_flow_cell(
        "aps4t",
        "4T pixel, 5.6 µm pitch: photodiode with transfer gate, floating diffusion, RST / SF / SEL (DRC / LVS clean)",
        s,
        r.finish(),
        sim,
    )
}

/// Height (dbu) of the 2-shared pixel pair: two 5.6 µm pixels.
const PIX2: i64 = 2 * PIX;

/// Labels at the pins of instance `id`: net names, or a ground symbol for "0".
fn pin_labels(s: &mut Schematic, id: u64, pins: &[(&str, &str)]) {
    for (pin, net) in pins {
        let at = pin_at(s, id, pin);
        if *net == "0" {
            s.add_instance(DeviceKind::Ground, at, Orient::default());
        } else {
            s.add_label(*net, at);
        }
    }
}

/// 2-shared 4T pixel (1 x 2, 5.6 x 11.2 µm): two photodiodes, one above the other, transfer
/// through TX1 and TX2 onto one floating diffusion `fd` read by a shared RST / SF / SEL. A
/// vertical neck of diffusion joins the two photodiodes through the shared strip; the gates
/// cross it next to each photodiode, and the neck between them is FD with the strip's end.
/// The devices and the photodiodes are those of `aps4t`; the readout strip sits in the middle,
/// and TX1 / TX2 get rows of their own. The bench reads pixel 1, resets FD, then reads pixel 2.
pub fn aps2s(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    let (a, pe) = (PIX_PD.width() * PIX_PD.height(), 2 * (PIX_PD.width() + PIX_PD.height()));
    let (pd_as, pd_ps) = (format!("{}p", a as f64 / 1e6), format!("{}u", pe as f64 / 1e3));
    for (k, y) in [(1, 12), (2, 22)] {
        let tx = s.add_instance(DeviceKind::Nmos, Pt::new(8, y), R90);
        set(&mut s, tx, &[("w", "420n"), ("l", "400n"), ("as", &pd_as), ("ps", &pd_ps)]);
        let (pd, txn) = (format!("pd{k}"), format!("tx{k}"));
        pin_labels(&mut s, tx, &[("d", "fd"), ("g", &txn), ("s", &pd), ("b", "0")]);
        let iph = s.add_instance(DeviceKind::ISource, Pt::new(1, y + 6), o);
        set(&mut s, iph, &[("dc", if k == 1 { "2n" } else { "0.5n" })]);
        pin_labels(&mut s, iph, &[("p", &pd), ("n", "0")]);
    }
    // The floating diffusion with its metal on the reset drain: a few fF.
    let rst = mos(&mut s, DeviceKind::Nmos, (16, 4), false, "420n", "180n", None);
    set(&mut s, rst, &[("ad", "3p"), ("pd", "8u")]);
    pin_labels(&mut s, rst, &[("d", "fd"), ("g", "rst"), ("s", "vdd")]);
    let sf = mos(&mut s, DeviceKind::Nmos, (24, 12), false, "420n", "350n", None);
    pin_labels(&mut s, sf, &[("d", "vdd"), ("g", "fd"), ("s", "x")]);
    let sel = mos(&mut s, DeviceKind::Nmos, (24, 20), false, "420n", "180n", None);
    pin_labels(&mut s, sel, &[("d", "x"), ("g", "sel"), ("s", "col")]);
    let icol = s.add_instance(DeviceKind::ISource, Pt::new(32, 26), o);
    set(&mut s, icol, &[("dc", "2u")]);
    pin_labels(&mut s, icol, &[("p", "col"), ("n", "0")]);
    // Reset both photodiodes and FD; release the reset (reset level 1), transfer pixel 1
    // (signal 1); reset FD again (reset level 2), transfer pixel 2 (signal 2).
    source(&mut s, (-40, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-32, 8), "rst", &[("dc", "1.8"), ("tran", "PWL(0 1.8 300n 1.8 301n 0 600n 0 601n 1.8 700n 1.8 701n 0)")]);
    source(&mut s, (-24, 8), "tx1", &[("dc", "1.8"), ("tran", "PWL(0 1.8 50n 1.8 51n 0 400n 0 401n 1.8 450n 1.8 451n 0)")]);
    source(&mut s, (-16, 8), "tx2", &[("dc", "1.8"), ("tran", "PWL(0 1.8 50n 1.8 51n 0 800n 0 801n 1.8 850n 1.8 851n 0)")]);
    source(&mut s, (-8, 8), "sel", &[("dc", "1.8")]);
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "1n", "1000n");

    let mut r = Router::new(&p.tech);
    let l = |name: &str| p.tech.layer_index(name).expect("generic layer");
    let (active, poly, m1, m2, m3) = (l("active"), l("poly"), l("metal1"), l("metal2"), l("metal3"));
    let (nplus, boundary) = (l("nplus"), l("boundary"));
    // The strip in the middle; each photodiode 1.6 µm from it, as far as the gates need.
    let y = PIX;
    let pd1 = IRect { x0: PIX_PD.x0, y0: y - 1600 - PIX_PD.height(), x1: PIX_PD.x1, y1: y - 1600 };
    let pd2 = IRect { x0: PIX_PD.x0, y0: y + 1600, x1: PIX_PD.x1, y1: y + 1600 + PIX_PD.height() };
    let rows = [("tx1", y - 1270), ("sel", y + 620), ("rst", y + 1240), ("tx2", y + 1870)];
    for (layer, (x0, y0, x1, y1)) in [
        (active, (290, y - 210, 2810, y + 210)),  // transistor strip
        (active, (2390, pd1.y1, 2810, pd2.y0)),   // neck: TX1, FD, TX2
        (poly, (770, y - 430, 950, y + 580)),     // SEL
        (poly, (1190, y - 600, 1540, y + 430)),   // SF, gate contact below the strip
        (poly, (2080, y - 430, 2260, y + 600)),   // RST
        (poly, (2170, y - 1470, 3200, y - 1070)), // TX1, L 0.4 µm
        (poly, (2170, y + 1070, 3200, y + 1470)), // TX2
        (m1, (0, 0, 300, PIX2)),                  // column line
        (m1, (150, y - 170, 500, y + 170)),
        (m1, (2430, y - 770, 2770, y)), // floating diffusion to the SF gate
        (m1, (1230, y - 770, 2770, y - 430)),
        (m1, (570, y + 580, 910, y + 620)),
        (m1, (2000, y + 600, 2340, y + 1240)),
        (m1, (3030, y + 1270, 3370, y + 1870)), // TX2 up to its row
        (m3, (0, y - 150, PIX, y + 150)),       // supply, shielding the transistors
        (nplus, (0, 0, PIX, PIX2)),
        (boundary, (0, 0, PIX, PIX2)),
    ] {
        r.lay.add_rect(layer, x0, y0, x1, y1);
    }
    r.lay.add(active, Geometry::Rect(pd1));
    r.lay.add(active, Geometry::Rect(pd2));
    for (name, at) in [
        ("contact", (500, y)),
        ("contact", (1810, y)),
        ("contact", (2600, y)),
        ("polycon", (740, y + 580)),
        ("polycon", (1400, y - 600)),
        ("polycon", (2170, y + 600)),
        ("polycon", (3200, y - 1270)),
        ("polycon", (3200, y + 1270)),
        ("via1", (1810, y)),
        ("via2", (1810, y)),
        ("via1", (740, y + 620)),
        ("via1", (2170, y + 1240)),
        ("via1", (3200, y - 1270)),
        ("via1", (3200, y + 1870)),
    ] {
        r.via(name, at);
    }
    for (net, ry) in rows {
        r.lay.add_rect(m2, 0, ry - 150, PIX, ry + 150);
        r.label(net, (3800, ry));
    }
    let mid = |pd: IRect| (pd.y0 + pd.y1) / 2;
    for (net, at) in
        [("vdd", (3800, y)), ("col", (150, 1000)), ("fd", (2600, y - 600)), ("pd1", (2800, mid(pd1))), ("pd2", (2800, mid(pd2)))]
    {
        r.label(net, at);
    }
    full_flow_cell(
        "aps2s",
        "2-shared 4T pixel, 5.6 x 11.2 µm: two photodiodes with TX1 / TX2 onto one floating diffusion, shared RST / SF / SEL (DRC / LVS clean)",
        s,
        r.finish(),
        sim,
    )
}

/// Pitch (dbu) of the 2 x 2 Bayer pixel: 2.4 µm. With the deep trenches (0.15 µm, active kept
/// 0.1 µm clear) a pixel has 2.04 µm of silicon: the photodiode 1.05 µm, a wide transfer gate
/// (L 0.45 µm) on its edge with the floating diffusion after it, and beside them a readout strip
/// whose gates keep clear of the transfer gate. 2.2 µm leaves no room for both.
const BAYER: i64 = 2400;

/// 2 x 2 Bayer 4-shared back-side illuminated pixel with full-depth deep trench isolation
/// (4.8 x 4.8 µm, 2.4 µm pitch). Pixel (i, j) has its photodiode in its outer corner and its
/// transfer gate TX(1 + i + 2 j) along the photodiode's edge facing the horizontal centre line:
/// the channel (W 0.89 µm, L 0.45 µm) continues the photodiode, the floating diffusion after it
/// is as wide, and the gate contact sits past the gate's inner end. R (pd1) is bottom left, Gr
/// (pd2) bottom right, Gb (pd3) top left, B (pd4) top right. The trenches cut the silicon, so
/// the four floating diffusions are joined in metal (`fd`): metal1 on each side, metal2 across.
/// The readout strips lie in the left pixels next to the vertical trench: SEL and SF in series in
/// R, RST in Gb, with 0.3 µm gates whose poly crosses the trench to contacts in the right
/// pixels. The deep photodiode implant `pdn` lies under each whole pixel, transistors included,
/// as it does in a BSI sensor. Rows (SEL, TX2, TX1, TX3, TX4, RST) run on metal2, the column line
/// on metal1 at the left edge.
pub fn bayer_fdti(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    let pd = IRect { x0: 180, y0: 180, x1: 1230, y1: 1230 };
    let (a, pe) = (pd.width() * pd.height(), 2 * (pd.width() + pd.height()));
    let (pd_as, pd_ps) = (format!("{}p", a as f64 / 1e6), format!("{}u", pe as f64 / 1e3));
    // Photocurrents under the colour filters: R, Gr, Gb, B.
    for (k, iph_dc) in [(1, "1n"), (2, "2n"), (3, "2n"), (4, "0.6n")] {
        let y = 2 + 10 * k;
        let tx = s.add_instance(DeviceKind::Nmos, Pt::new(8, y), R90);
        set(&mut s, tx, &[("w", "890n"), ("l", "450n"), ("as", &pd_as), ("ps", &pd_ps)]);
        let (pdk, txk) = (format!("pd{k}"), format!("tx{k}"));
        pin_labels(&mut s, tx, &[("d", "fd"), ("g", &txk), ("s", &pdk), ("b", "0")]);
        let iph = s.add_instance(DeviceKind::ISource, Pt::new(1, y + 6), o);
        set(&mut s, iph, &[("dc", iph_dc)]);
        pin_labels(&mut s, iph, &[("p", &pdk), ("n", "0")]);
    }
    // The floating diffusions with their metal on the reset source: a few fF.
    let rst = mos(&mut s, DeviceKind::Nmos, (16, 4), false, "300n", "180n", None);
    set(&mut s, rst, &[("ad", "3p"), ("pd", "8u")]);
    pin_labels(&mut s, rst, &[("d", "fd"), ("g", "rst"), ("s", "vdd")]);
    let sf = mos(&mut s, DeviceKind::Nmos, (24, 12), false, "300n", "350n", None);
    pin_labels(&mut s, sf, &[("d", "vdd"), ("g", "fd"), ("s", "x")]);
    let sel = mos(&mut s, DeviceKind::Nmos, (24, 20), false, "300n", "180n", None);
    pin_labels(&mut s, sel, &[("d", "x"), ("g", "sel"), ("s", "col")]);
    let icol = s.add_instance(DeviceKind::ISource, Pt::new(32, 26), o);
    set(&mut s, icol, &[("dc", "2u")]);
    pin_labels(&mut s, icol, &[("p", "col"), ("n", "0")]);
    // Reset the photodiodes and FD (all TX and RST high) and integrate; then for each pixel in
    // turn reset FD, release the reset (reset level) and transfer (signal level), 400 ns apart.
    let hi = 2.8;
    let rst_pwl = format!(
        "PWL(0 {hi} 300n {hi} 301n 0 600n 0 601n {hi} 700n {hi} 701n 0 1000n 0 1001n {hi} 1100n {hi} 1101n 0 1400n 0 1401n {hi} 1500n {hi} 1501n 0)"
    );
    let tx_pwl = |k: i32| {
        let t = 400 * k;
        format!("PWL(0 {hi} 50n {hi} 51n 0 {t}n 0 {}n {hi} {}n {hi} {}n 0)", t + 1, t + 50, t + 51)
    };
    source(&mut s, (-56, 8), "vdd", &[("dc", "2.8")]);
    source(&mut s, (-48, 8), "rst", &[("dc", "2.8"), ("tran", &rst_pwl)]);
    for k in 1..=4 {
        source(&mut s, (-48 + 8 * k, 8), &format!("tx{k}"), &[("dc", "2.8"), ("tran", &tx_pwl(k))]);
    }
    source(&mut s, (-8, 8), "sel", &[("dc", "2.8")]);
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "1n", "1800n");

    let mut r = Router::new(&p.tech);
    let l = |name: &str| p.tech.layer_index(name).expect("generic layer");
    let (active, poly, m1, m2) = (l("active"), l("poly"), l("metal1"), l("metal2"));
    let (nplus, pdn, dti, boundary) = (l("nplus"), l("pdn"), l("dti"), l("boundary"));
    let c = 2 * BAYER;
    // The four pixels are mirror images of the bottom-left one about the centre lines.
    let mut rects: Vec<(usize, IRect)> = Vec::new();
    for (mx, my) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut add = |layer: usize, x0: i64, y0: i64, x1: i64, y1: i64| {
            let (x0, x1) = if mx { (c - x1, c - x0) } else { (x0, x1) };
            let (y0, y1) = if my { (c - y1, c - y0) } else { (y0, y1) };
            rects.push((layer, IRect { x0, y0, x1, y1 }));
        };
        add(active, pd.x0, pd.y0, pd.x1, pd.y1); // photodiode
        // The transfer gate along the photodiode's inner edge, its channel the photodiode's
        // continuation, and the floating diffusion after it, as wide as the channel.
        add(active, 340, 1230, 1230, 1680);
        add(active, 340, 1680, 1230, 2100);
        add(poly, 120, 1230, 1450, 1680); // TX, W 0.89 µm, L 0.45 µm; its contact past the end
        add(pdn, 200, 200, 2200, 2200);
    }
    for (layer, r_) in rects {
        r.lay.add(layer, Geometry::Rect(r_));
    }
    for (layer, (x0, y0, x1, y1)) in [
        // R: column contact, SEL, SF, supply contact (gates on a 0.3 µm strip, clear of TX1).
        (active, (1800, 180, 2220, 600)),
        (active, (1920, 600, 2220, 1410)),
        (active, (1800, 1410, 2220, 1830)),
        // Gb: supply contact, RST, FD contact.
        (active, (1800, 2970, 2220, 3390)),
        (active, (1920, 3390, 2220, 4200)),
        (active, (1800, 4200, 2220, 4620)),
        (poly, (1700, 620, 2810, 800)),   // SEL, its contact in Gr
        (poly, (1700, 1040, 2810, 1390)), // SF, its contact in Gr
        (poly, (1700, 4000, 2810, 4180)), // RST, its contact in B
        (m1, (110, 0, 340, c)),           // column line
        (m1, (110, 220, 2180, 560)),
        (m1, (1840, 1450, 2180, 3350)), // supply
        (m1, (600, 1720, 980, 4580)),   // floating diffusions, left, and the reset source
        (m1, (600, 4240, 2180, 4580)),
        (m1, (3820, 1720, 4200, 3080)), // floating diffusions, right
        (m2, (790, 2250, 4010, 2550)),  // floating diffusions across, and the SF gate
        (nplus, (0, 0, c, c)),
        (boundary, (0, 0, c, c)),
    ] {
        r.lay.add_rect(layer, x0, y0, x1, y1);
    }
    // Deep trenches on the pixel edges, centred on the cell edges so that tiled cells share them.
    let t = 75;
    for (x0, y0, x1, y1) in [
        (-t, -t, t, c + t),
        (c - t, -t, c + t, c + t),
        (-t, -t, c + t, t),
        (-t, c - t, c + t, c + t),
        (BAYER - t, -t, BAYER + t, c + t),
        (-t, BAYER - t, c + t, BAYER + t),
    ] {
        r.lay.add_rect(dti, x0, y0, x1, y1);
    }
    for (name, at) in [
        ("contact", (790, 1890)),
        ("contact", (4010, 1890)),
        ("contact", (790, 2910)),
        ("contact", (4010, 2910)),
        ("contact", (2010, 390)),
        ("contact", (2010, 1620)),
        ("contact", (2010, 3180)),
        ("contact", (2010, 4410)),
        ("polycon", (1440, 1860)),
        ("polycon", (3360, 1860)),
        ("polycon", (1440, 2940)),
        ("polycon", (3360, 2940)),
        ("polycon", (2600, 590)),
        ("polycon", (2600, 1250)),
        ("polycon", (2600, 4210)),
        ("via1", (790, 2400)),
        ("via1", (4010, 2400)),
    ] {
        r.via(name, at);
    }
    // The SF gate up to the floating diffusions' metal2.
    r.bar("metal1", (2600, 1250), (2600, 2400));
    r.via("via1", (2600, 2400));
    // Rows: each gate contact up to its metal2 row.
    let rows = [
        ("sel", 500, 590, 2600),
        ("tx2", 1120, 1860, 3360),
        ("tx1", 1740, 1860, 1420),
        ("tx3", 3060, 2940, 1420),
        ("tx4", 3680, 2940, 3360),
        ("rst", 4300, 4210, 2600),
    ];
    for (net, ry, gy, vx) in rows {
        r.bar("metal1", (vx, gy), (vx, ry));
        r.via("via1", (vx, ry));
        r.lay.add_rect(m2, 0, ry - 150, c, ry + 150);
        r.label(net, (4700, ry));
    }
    let labels = [
        ("pd1", (700, 800)),
        ("pd2", (4100, 800)),
        ("pd3", (400, 4100)),
        ("pd4", (4100, 4000)),
        ("vdd", (2010, 2750)),
        ("col", (225, 2750)),
        ("fd", (4010, 2750)),
    ];
    for (net, at) in labels {
        r.label(net, at);
    }
    full_flow_cell(
        "bayer_fdti",
        "2 x 2 Bayer 4-shared BSI pixel with full-depth DTI, 2.4 µm pitch: four photodiodes with TX1-TX4 onto one floating diffusion, shared RST / SF / SEL (DRC / LVS clean)",
        s,
        r.finish(),
        sim,
    )
}

/// Effective pixel pitch (µm) of the `bayer_fdti_pc` PCell: the `pitch` parameter, or more when
/// the photodiode and the transfer gate need it (the readout strip beside the photodiode takes
/// 1.35 µm across the pixel, the floating diffusion and its clearance to the trench 0.9 µm
/// along it).
const BAYER_PC_PITCH: &str = "max(pitch, pd + 1.35, pd + tx_l + 0.9)";

/// The `bayer_fdti` layout as a PCell Designer template (`bayer_fdti_pc`) with parameters
/// `pitch` (pixel pitch), `pd` (photodiode side; the transfer gate's width follows, W = pd −
/// 0.16 µm) and `tx_l` (transfer gate length). The four pixels stay mirror images: each
/// parameter has a stretch line in every pixel, placed where no contact is cut and nothing is
/// at its minimum spacing across the line:
///
/// - `pd`: x through the photodiode and the transfer gate past the floating diffusion's
///   contact, y through the photodiode between the SEL and SF gates.
/// - `tx_l`: y through the transfer gate, above the SF gate.
/// - pitch: x between the transfer gate's contact and the readout strip, y between the
///   floating diffusion and the trench; they take what the pitch leaves.
///
/// The drawing is at its minimum spacings in the other direction, so the parameters only grow
/// from the drawn values (pitch 2.4, pd 1.05, tx_l 0.45 µm).
pub fn bayer_fdti_template(source: &Layout) -> crate::pcell_design::Template {
    use crate::pcell_design::{Axis, Op, Param, ParamKind, Side, Template};
    let mut t = Template::from_layout("bayer_fdti_pc", source);
    t.description =
        "2 x 2 Bayer 4-shared BSI pixel with full-depth DTI (bayer_fdti) by pitch, photodiode and transfer gate".into();
    let param = |name: &str, default: f64, max: f64, description: &str| Param {
        min: Some(default),
        max: Some(max),
        description: description.into(),
        ..Param::new(name, ParamKind::Length, default)
    };
    t.params = vec![
        param("pitch", 2.4, 4.0, "pixel pitch (raised to fit pd and tx_l)"),
        param("pd", 1.05, 2.4, "photodiode side; the transfer gate is pd - 0.16 wide"),
        param("tx_l", 0.45, 0.8, "transfer gate length"),
        Param {
            description: "45° corners (0.12 µm) on the photodiodes and the floating diffusions".into(),
            ..Param::new("chamfer", ParamKind::Bool, 1.0)
        },
    ];
    chamfer_diffusions(&mut t);
    let c = 2 * BAYER;
    let gap_x = format!("{BAYER_PC_PITCH} - pd");
    let gap_y = format!("{BAYER_PC_PITCH} - pd - tx_l");
    let line =
        |axis: Axis, at: i64, amount: &str| Op::Stretch { axis, at, side: Side::High, amount: amount.into(), layers: vec![] };
    // Lines in the bottom-left pixel; the other pixels take the mirror images.
    for (axis, at, amount) in [
        (Axis::X, 1100, "pd"),
        (Axis::X, 1665, gap_x.as_str()),
        (Axis::Y, 900, "pd"),
        (Axis::Y, 1470, "tx_l"),
        (Axis::Y, 2150, &gap_y),
    ] {
        t.ops.push(line(axis, at, amount));
        t.ops.push(line(axis, c - at, amount));
    }
    // Lines in increasing position, so that each one moves the later ones by its stretch.
    t.ops.sort_by_key(|o| match o {
        Op::Stretch { axis, at, .. } => (*axis == Axis::Y, *at),
        _ => (true, i64::MAX),
    });
    t
}

/// Cut of the 45° corners of `bayer_fdti_pc` (dbu): below the 130 between the photodiode's edge
/// and its stretch line, so that no line crosses a diagonal edge.
const BAYER_CHAMFER: i64 = 120;

/// Give the photodiodes and the floating diffusions of a `bayer_fdti` drawing 45° corners
/// wherever no other diffusion meets the corner (the transfer gate's channel continues the
/// photodiode at one corner and starts the floating diffusion at two). Each diffusion becomes
/// the chamfered polygon plus a triangle per corner, kept when `chamfer` is off, so the
/// rectangles come back exactly.
fn chamfer_diffusions(t: &mut crate::pcell_design::Template) {
    use crate::pcell_design::Op;
    let Some(active) = t
        .source
        .shapes
        .iter()
        .find_map(|s| matches!(s.geom, Geometry::Rect(r) if r.width() == 1050 && r.height() == 1050).then_some(s.layer))
    else {
        return;
    };
    let rects: Vec<IRect> = t
        .source
        .shapes
        .iter()
        .filter(|s| s.layer == active)
        .filter_map(|s| if let Geometry::Rect(r) = s.geom { Some(r) } else { None })
        .collect();
    // Photodiodes (1.05 µm squares) and floating diffusions (0.89 x 0.42 µm, beyond a channel).
    let targets: Vec<IRect> = rects
        .iter()
        .copied()
        .filter(|r| (r.width(), r.height()) == (1050, 1050) || (r.width(), r.height()) == (890, 420))
        .collect();
    let c = BAYER_CHAMFER;
    let mut shapes = vec![];
    for r in &targets {
        let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)];
        let free: Vec<bool> = corners
            .iter()
            .map(|&(x, y)| !rects.iter().any(|o| o != r && o.x0 <= x && x <= o.x1 && o.y0 <= y && y <= o.y1))
            .collect();
        if !free.iter().any(|&f| f) {
            continue;
        }
        let mut pts = vec![];
        for (k, &(x, y)) in corners.iter().enumerate() {
            if !free[k] {
                pts.push((x, y));
                continue;
            }
            // Towards the previous and the next corner, `c` along each edge.
            let (px, py) = corners[(k + 3) % 4];
            let (nx, ny) = corners[(k + 1) % 4];
            let step = |a: i64, b: i64| (b - a).signum() * c;
            let a = (x + step(x, px), y + step(y, py));
            let b = (x + step(x, nx), y + step(y, ny));
            pts.push(a);
            pts.push(b);
            shapes.push((
                Geometry::Polygon { points: vec![a, (x, y), b] },
                IRect::new(x.min(a.0).min(b.0), y.min(a.1).min(b.1), x.max(a.0).max(b.0), y.max(a.1).max(b.1)),
            ));
        }
        t.source.shapes.retain(|s| !(s.layer == active && s.geom == Geometry::Rect(*r)));
        t.source.add(active, Geometry::Polygon { points: pts });
    }
    for (tri, region) in shapes {
        t.source.add(active, tri);
        t.ops.push(Op::Condition { region, when: "!chamfer".into(), layers: vec![active] });
    }
}

/// 8 x 8 image sensor array of `aps4t`: labelled TX / reset / select rows and column lines, and
/// a supply strap joining the supply rows.
pub fn pixel_array(tech: &Tech, lib: &str) -> Cell {
    let mut r = Router::new(tech);
    r.lay.add_instance(
        CellRef::new(lib, "aps4t"),
        Trans::new(0, 0),
        Some(ArraySpec { cols: PIX_COLS as u32, rows: PIX_ROWS as u32, col: (PIX, 0), row: (0, PIX) }),
    );
    let m3 = r.layer("metal3");
    let (strap, y) = (-500, PIX_STRIP);
    for row in 0..PIX_ROWS {
        r.lay.add_rect(m3, strap, y + row * PIX - 150, 0, y + row * PIX + 150);
        for (net, dy) in [("tx", -870), ("sel", 620), ("rst", 1240)] {
            r.label(&format!("{net}{row}"), (3800, y + dy + row * PIX));
        }
    }
    r.lay.add_rect(m3, strap - 190, y - 150, strap + 190, y + (PIX_ROWS - 1) * PIX + 150);
    r.label("vdd", (strap, y));
    for col in 0..PIX_COLS {
        r.label(&format!("col{col}"), (col * PIX + 150, 1000));
    }
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, -1000, 0, PIX_COLS * PIX, PIX_ROWS * PIX);
    layout_cell("pixel_array", "8 x 8 image sensor array of 5.6 µm 4T pixels (AREF)", r.finish())
}

/// A cell whose layout is made by the automatic flow from a SPICE deck: the deck is imported as a
/// schematic, the devices are placed from it (`sdl`) and every net is maze-routed (`route`).
fn auto_pnr_cell(p: &mut Project, name: &str, description: &str, deck: &str) -> Cell {
    let imported = crate::spice_import::import(deck, &p.tech).expect("demo deck");
    let mut sim = SimSetup::default();
    imported.apply_to(&mut sim);
    let sch = imported.schematic;
    let plan = crate::sdl::plan(&sch, &p.tech);
    let mut lay = Layout::default();
    crate::sdl::apply(p, &mut lay, &plan).expect("placement");
    let rep = crate::route::route(&sch, &mut lay, p, &p.tech).expect("routing");
    assert!(rep.failed.is_empty(), "{name}: {}", rep.summary());
    full_flow_cell(name, description, sch, lay, sim)
}

/// 28-transistor mirror full adder with inverted outputs buffered, placed and routed
/// automatically from its netlist.
pub fn full_adder(p: &mut Project) -> Cell {
    const DECK: &str = "\
* carry: cob = !(a b + ci (a + b))
MP1 n1 a vdd vdd pmos w=2u l=180n
MP2 n1 b vdd vdd pmos w=2u l=180n
MP3 cob ci n1 vdd pmos w=2u l=180n
MP4 n2 a vdd vdd pmos w=2u l=180n
MP5 cob b n2 vdd pmos w=2u l=180n
MN1 n3 a 0 0 nmos w=1u l=180n
MN2 n3 b 0 0 nmos w=1u l=180n
MN3 cob ci n3 0 nmos w=1u l=180n
MN4 n4 a 0 0 nmos w=1u l=180n
MN5 cob b n4 0 nmos w=1u l=180n
* sum: sb = !(a b ci + cob (a + b + ci))
MP6 n5 a vdd vdd pmos w=2u l=180n
MP7 n5 b vdd vdd pmos w=2u l=180n
MP8 n5 ci vdd vdd pmos w=2u l=180n
MP9 sb cob n5 vdd pmos w=2u l=180n
MP10 n6 a vdd vdd pmos w=2u l=180n
MP11 n7 b n6 vdd pmos w=2u l=180n
MP12 sb ci n7 vdd pmos w=2u l=180n
MN6 n8 a 0 0 nmos w=1u l=180n
MN7 n8 b 0 0 nmos w=1u l=180n
MN8 n8 ci 0 0 nmos w=1u l=180n
MN9 sb cob n8 0 nmos w=1u l=180n
MN10 n9 a 0 0 nmos w=1u l=180n
MN11 n10 b n9 0 nmos w=1u l=180n
MN12 sb ci n10 0 nmos w=1u l=180n
* output inverters
MP13 co cob vdd vdd pmos w=2u l=180n
MN13 co cob 0 0 nmos w=1u l=180n
MP14 s sb vdd vdd pmos w=2u l=180n
MN14 s sb 0 0 nmos w=1u l=180n
VDD vdd 0 dc 1.8
VA a 0 pulse(0 1.8 0 50p 50p 0.95n 2n)
VB b 0 pulse(0 1.8 0 50p 50p 1.95n 4n)
VCI ci 0 pulse(0 1.8 0 50p 50p 3.95n 8n)
.tran 10p 8n
";
    auto_pnr_cell(p, "full_adder", "28T mirror full adder, placed and routed automatically (DRC / LVS clean)", DECK)
}

/// Two-stage OTA: NMOS input pair with PMOS mirror load and a PMOS common-source second stage.
/// Placement matches the pair and the mirror (common centroid); every net is routed automatically.
pub fn ota2s(p: &mut Project) -> Cell {
    const DECK: &str = "\
M1 x inp tail 0 nmos w=4u l=500n
M2 o1 inn tail 0 nmos w=4u l=500n
M3 x x vdd vdd pmos w=8u l=500n
M4 o1 x vdd vdd pmos w=8u l=500n
M5 tail vb 0 0 nmos w=4u l=1u
M6 out o1 vdd vdd pmos w=16u l=500n
M7 out vb 0 0 nmos w=4u l=1u
VDD vdd 0 dc 1.8
VINP inp 0 dc 0.9 ac 1
VINN inn 0 dc 0.9
VB vb 0 dc 0.65
.op
.ac dec 20 1k 10g
";
    auto_pnr_cell(p, "ota2s", "Two-stage OTA, matched placement and automatic routing (DRC / LVS clean)", DECK)
}

pub fn demo_project() -> Project {
    let mut p = Project::new("demo");
    p.description = "Example designs shipped with IC Studio".into();
    let hier = hierarchy_cells(&p.tech, "work");
    for c in [cmos_inverter(), rc_lowpass(), diode_clipper(), inv(), buffer_tb(), bandgap()].into_iter().chain(hier) {
        p.add_cell("work", c).expect("unique");
    }
    for make in [
        nand2,
        nor2,
        ring_osc5,
        current_mirror,
        ota5t,
        sram6t,
        dram1t1c,
        nand_string,
        aps4t,
        aps2s,
        bayer_fdti,
        full_adder,
        ota2s,
    ] {
        let c = make(&mut p);
        p.add_cell("work", c).expect("unique");
    }
    for c in [sram_array(&p.tech, "work"), dram_array(&p.tech, "work"), nand_array(&p.tech, "work"), pixel_array(&p.tech, "work")]
    {
        p.add_cell("work", c).expect("unique");
    }
    let mut devices = Layout::default();
    let mut x = 0;
    for pc in crate::pcell::PCell::defaults() {
        let r = p.ensure_pcell(&pc).expect("default pcells generate");
        let w = p.cell(&r).and_then(|c| c.layout.as_ref()).and_then(Layout::shapes_bbox).map_or(5000, |b| b.width());
        devices.add_instance(r, Trans::new(x, 0), None);
        x += w + 3000;
    }
    p.add_cell("work", layout_cell("devices", "Parametric devices placed from the pcells library", devices)).expect("unique");
    let bayer = p.cell(&CellRef::new("work", "bayer_fdti")).and_then(|c| c.layout.clone()).expect("bayer_fdti layout");
    p.publish_template("work", bayer_fdti_template(&bayer)).expect("bayer_fdti_pc generates");
    p
}

/// A themed example project: a subset of the demo cells (with the cells they place).
#[derive(Debug, Clone, Copy)]
pub struct Topic {
    pub name: &'static str,
    pub title: &'static str,
    /// A few words for a card.
    pub summary: &'static str,
    pub description: &'static str,
    pub cells: &'static [&'static str],
}

pub const TOPICS: [Topic; 5] = [
    Topic {
        name: "getting_started",
        title: "Getting started",
        summary: "Simulation, layout, PCells",
        description: "Schematic entry and simulation (op, DC, AC, transient), a hand-drawn inverter layout, the parametric devices",
        cells: &["cmos_inverter", "rc_lowpass", "diode_clipper", "inv", "buffer_tb", "devices"],
    },
    Topic {
        name: "analog",
        title: "Analog",
        summary: "Current mirror and OTAs",
        description: "Current mirror and OTAs: AC gain, matched placement with dummies, DRC, LVS and post-layout simulation",
        cells: &["current_mirror", "ota5t", "ota2s", "bandgap"],
    },
    Topic {
        name: "digital",
        title: "Digital",
        summary: "Gates, ring osc, auto P&R",
        description: "Gates, a ring oscillator and a full adder placed and routed from its netlist",
        cells: &["nand2", "nor2", "ring_osc5", "full_adder"],
    },
    Topic {
        name: "memory_and_pixels",
        title: "Memory and pixels",
        summary: "SRAM, DRAM, flash, pixels",
        description: "6T SRAM, planar 1T1C DRAM, NAND flash string and 4T pixel, each with an array (AREF) of word / bit or row / column lines, and a 2-shared 4T pixel",
        cells: &[
            "sram6t",
            "sram_array",
            "dram1t1c",
            "dram_array",
            "nand_string",
            "nand_array",
            "aps4t",
            "pixel_array",
            "aps2s",
            "bayer_fdti",
            "bayer_fdti_pc",
        ],
    },
    Topic {
        name: "hierarchy",
        title: "Hierarchy",
        summary: "8192 inverters, 2 levels",
        description: "Placed inverters up to 8192 instances in two levels of arrays, for hierarchy browsing and large-layout display",
        cells: &["inv_chain", "inv_array", "inv_tiles"],
    },
];

/// The example project of topic `name`: its cells of the demo project in library `work`, plus
/// every cell their layouts place (from any library). Libraries left empty are dropped.
pub fn topic_project(name: &str) -> Option<Project> {
    topic_from(&demo_project(), name)
}

fn topic_from(demo: &Project, name: &str) -> Option<Project> {
    let topic = TOPICS.iter().find(|t| t.name == name)?;
    let mut p = demo.clone();
    p.name = topic.name.into();
    p.description = format!("{}: {}", topic.title, topic.description);
    let mut keep: HashSet<CellRef> = HashSet::new();
    for c in topic.cells {
        keep.extend(crate::hier::descendants(&p, &CellRef::new("work", *c)));
    }
    for lib in &mut p.libraries {
        let name = lib.name.clone();
        lib.cells.retain(|c| keep.contains(&CellRef::new(&name, &c.name)));
    }
    p.libraries.retain(|l| !l.cells.is_empty());
    Some(p)
}

/// Writes every topic project to `dir/<topic>`; existing ones are skipped unless `overwrite`.
/// Returns the topics written.
pub fn write_topics(dir: &std::path::Path, overwrite: bool) -> crate::project::Result<Vec<&'static str>> {
    let mut written = Vec::new();
    let mut demo = None;
    for t in &TOPICS {
        let root = dir.join(t.name);
        if root.join(crate::project::MANIFEST).exists() && !overwrite {
            continue;
        }
        let demo = demo.get_or_insert_with(demo_project);
        let mut p = topic_from(demo, t.name).expect("known topic");
        p.save_as(&root)?;
        written.push(t.name);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_cover_the_demo_and_keep_dependencies() {
        let demo = demo_project();
        let work: HashSet<String> =
            demo.libraries.iter().find(|l| l.name == "work").unwrap().cells.iter().map(|c| c.name.clone()).collect();
        let listed: HashSet<String> = TOPICS.iter().flat_map(|t| t.cells.iter().map(|c| c.to_string())).collect();
        assert_eq!(listed, work, "every demo cell is in exactly the topics");
        let h = topic_project("hierarchy").unwrap();
        assert!(h.cell(&CellRef::new("work", "cmos_inverter")).is_some(), "placed cells come along");
        assert!(h.libraries.iter().all(|l| l.name != "pcells"));
        let d = topic_project("digital").unwrap();
        assert!(d.libraries.iter().any(|l| l.name == "pcells"));
        assert!(d.cell(&CellRef::new("work", "sram6t")).is_none());
        for t in &TOPICS {
            let p = topic_project(t.name).unwrap();
            for (r, c) in p.all_cells() {
                for i in c.layout.iter().flat_map(|l| &l.instances) {
                    assert!(p.cell(&i.cell).is_some(), "{}: {r} places missing {}", t.name, i.cell);
                }
            }
        }
        assert!(topic_project("nope").is_none());
    }

    #[test]
    fn topics_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("ics-topics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(write_topics(&dir, false).unwrap().len(), TOPICS.len());
        assert!(write_topics(&dir, false).unwrap().is_empty(), "existing examples are kept");
        let p = Project::open(&dir.join("analog")).unwrap();
        assert!(p.cell(&CellRef::new("work", "ota2s")).and_then(|c| c.layout.as_ref()).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod flow_tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::extract::extract;
    use crate::lvs::{LvsOptions, compare};
    use crate::sdl::ratsnest;

    fn full_flow(name: &str) {
        flow(name, true);
    }

    /// DRC, extraction, LVS, flylines and netlist checks; `whole` when every device is placed as
    /// one layout device (no matched units or dummies).
    fn flow(name: &str, whole: bool) {
        let p = demo_project();
        let cell = p.cell(&CellRef::new("work", name)).expect("demo cell");
        let (sch, lay) = (cell.schematic.as_ref().unwrap(), cell.layout.as_ref().unwrap());
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{name}: {:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{name}: {:?}", x.warnings);
        let r = compare(sch, &x, &LvsOptions::for_tech(&p.tech));
        assert!(r.matched, "{name}: {:#?}", r.errors);
        let mos = sch
            .instances
            .iter()
            .filter(|i| {
                matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Capacitor | DeviceKind::Resistor)
                    && !i.lvs_ignore()
            })
            .count();
        if whole {
            assert_eq!(r.layout_devices, mos);
        } else {
            assert!(r.layout_devices > mos, "{name}: {} layout devices", r.layout_devices);
        }
        let fl = ratsnest(sch, lay, &p, &p.tech);
        assert!(fl.is_empty(), "{name}: {fl:?}");
        let nl = crate::netlist::netlist_for(sch, &p.tech, "");
        assert!(nl.warnings.is_empty(), "{name}: {:?}", nl.warnings);
    }

    #[test]
    fn nand2_full_flow() {
        full_flow("nand2");
    }

    #[test]
    fn nor2_full_flow() {
        full_flow("nor2");
    }

    #[test]
    fn ring_osc5_full_flow() {
        full_flow("ring_osc5");
    }

    #[test]
    fn current_mirror_full_flow() {
        full_flow("current_mirror");
    }

    #[test]
    fn ota5t_full_flow() {
        full_flow("ota5t");
    }

    #[test]
    fn full_adder_full_flow() {
        full_flow("full_adder");
    }

    #[test]
    fn ota2s_full_flow() {
        flow("ota2s", false);
    }

    #[test]
    fn sram6t_full_flow() {
        full_flow("sram6t");
    }

    #[test]
    fn sram_array_drc_and_extraction() {
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "sram_array")).and_then(|c| c.layout.as_ref()).expect("array layout");
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().take(10).map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_eq!(x.devices.len() as i64, 6 * SRAM_ROWS * SRAM_COLS);
        let labelled = x.nets.iter().filter(|n| n.labelled).count() as i64;
        assert_eq!(labelled, 2 + SRAM_ROWS + 2 * SRAM_COLS);
    }

    #[test]
    fn dram1t1c_full_flow() {
        full_flow("dram1t1c");
    }

    #[test]
    fn dram_array_drc_and_extraction() {
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "dram_array")).and_then(|c| c.layout.as_ref()).expect("array layout");
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().take(10).map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_eq!(x.devices.len() as i64, 2 * DRAM_ROWS * DRAM_COLS);
        let labelled = x.nets.iter().filter(|n| n.labelled).count() as i64;
        assert_eq!(labelled, 1 + DRAM_ROWS + DRAM_COLS);
        assert_eq!(x.nets.len() as i64, 1 + DRAM_ROWS + DRAM_COLS + DRAM_ROWS * DRAM_COLS + 1, "storage nodes and sub");
    }

    #[test]
    fn nand_string_full_flow() {
        full_flow("nand_string");
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "nand_string")).and_then(|c| c.layout.as_ref()).unwrap();
        let x = extract(lay, &p, &p.tech);
        assert_eq!(x.passives.len() as i64, NAND_CELLS, "one interpoly capacitor per cell");
    }

    #[test]
    fn nand_array_drc_and_extraction() {
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "nand_array")).and_then(|c| c.layout.as_ref()).expect("array layout");
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().take(10).map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        let strings = NAND_ROWS * NAND_COLS;
        assert_eq!(x.devices.len() as i64, strings * (NAND_CELLS + 2));
        assert_eq!(x.passives.len() as i64, strings * NAND_CELLS);
        let labelled = x.nets.iter().filter(|n| n.labelled).count() as i64;
        assert_eq!(labelled, NAND_COLS + 1 + NAND_ROWS * (NAND_CELLS + 2));
    }

    #[test]
    fn aps4t_full_flow() {
        full_flow("aps4t");
    }

    #[test]
    fn aps2s_full_flow() {
        full_flow("aps2s");
    }

    #[test]
    fn bayer_fdti_full_flow() {
        full_flow("bayer_fdti");
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "bayer_fdti")).and_then(|c| c.layout.as_ref()).unwrap();
        let x = extract(lay, &p, &p.tech);
        assert_eq!(x.photodiodes.len(), 4, "{:?}", x.photodiodes);
        assert!(x.photodiodes.iter().all(|d| d.area == 1050 * 1050), "{:?}", x.photodiodes);
    }

    #[test]
    fn bayer_fdti_pcell_stretches_drc_clean() {
        use crate::pcell_design::{Axis, Op, canonical};
        use crate::polybool::PolySet;
        use crate::region::BoolOp;
        let p = demo_project();
        let master = p.cell(&CellRef::new("work", "bayer_fdti_pc")).expect("master");
        let Some(PCell::Template { def, .. }) = &master.pcell else { panic!("not a designer master") };
        let drawn = p.cell(&CellRef::new("work", "bayer_fdti")).and_then(|c| c.layout.as_ref()).unwrap();
        // Without the 45° corners it is the drawn cell: the same shapes, the diffusion as a
        // chamfered polygon and its corner triangles.
        let active = p.tech.layer_index("active").unwrap();
        let square = def.generate(&p.tech, &[("chamfer".into(), 0.0)]).unwrap();
        let others = |l: &Layout| {
            let mut l = l.clone();
            l.shapes.retain(|s| s.layer != active);
            canonical(&l)
        };
        assert_eq!(others(&square), others(drawn));
        let diff = |l: &Layout| PolySet::from_geometries(l.shapes.iter().filter(|s| s.layer == active).map(|s| &s.geom));
        assert_eq!(diff(&square).boolean(&diff(drawn), BoolOp::Xor).area(), 0.0);
        // With them (the default) the photodiodes lose three corners and the diffusions two.
        let cut = (BAYER_CHAMFER * BAYER_CHAMFER / 2) as f64;
        let lost = diff(drawn).area() - diff(&def.generate(&p.tech, &[]).unwrap()).area();
        assert_eq!(lost, 4.0 * 5.0 * cut);
        // No stretch line crosses a diagonal edge, which would bend it.
        for op in &def.ops {
            let Op::Stretch { axis, at, .. } = op else { continue };
            for sh in &def.source.shapes {
                let Geometry::Polygon { points } = &sh.geom else { continue };
                for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
                    if a.0 != b.0 && a.1 != b.1 {
                        let (lo, hi) = if *axis == Axis::X { (a.0.min(b.0), a.0.max(b.0)) } else { (a.1.min(b.1), a.1.max(b.1)) };
                        assert!(!(lo < *at && *at < hi), "line {axis:?} {at} crosses {a:?}-{b:?}");
                    }
                }
            }
        }
        assert!(def.lint(&p.tech).iter().all(|f| f.level != crate::pcell_design::Level::Error), "{:?}", def.lint(&p.tech));
        // No stretch line cuts a contact or via.
        let cuts: Vec<usize> = p.tech.vias.iter().filter_map(|v| p.tech.layer_index(&v.cut)).collect();
        for op in &def.ops {
            let Op::Stretch { axis, at, .. } = op else { continue };
            for sh in drawn.shapes.iter().filter(|s| cuts.contains(&s.layer)) {
                let b = sh.geom.bbox();
                let (lo, hi) = if *axis == Axis::X { (b.x0, b.x1) } else { (b.y0, b.y1) };
                assert!(!(lo < *at && *at < hi), "line {axis:?} {at} cuts {b:?}");
            }
        }
        // Every corner is DRC clean.
        let deck = crate::drc::parse_any(&p.tech.deck_text(), &p.tech).unwrap().deck;
        for run in def.check_corners(&p.tech, &deck, &p) {
            let r = run.result.unwrap_or_else(|e| panic!("{}: {e}", run.label));
            assert!(
                r.violations.is_empty(),
                "{}: {:?}",
                run.label,
                r.violations.iter().take(8).map(|v| (&v.rule, v.marker)).collect::<Vec<_>>()
            );
        }
        // A 3 µm pixel with a 1.6 µm photodiode: the frame, the photodiodes, the transfer gates.
        let lay = def.generate(&p.tech, &[("pitch".into(), 3.0), ("pd".into(), 1.6), ("tx_l".into(), 0.5)]).unwrap();
        let b = p.tech.layer_index("boundary").unwrap();
        let frame = lay.shapes.iter().find(|s| s.layer == b).unwrap().geom.bbox();
        assert_eq!((frame.width(), frame.height()), (6000, 6000));
        let x = extract(&lay, &p, &p.tech);
        assert_eq!(x.photodiodes.len(), 4, "{:?}", x.photodiodes);
        // The extraction's staircase of the 45° corners is within a few % of the exact area.
        let exact = (1600 * 1600) as f64 - 3.0 * cut;
        assert!(x.photodiodes.iter().all(|d| (d.area as f64 - exact).abs() < 0.01 * exact), "{:?}", x.photodiodes);
        let tx: Vec<_> = x.devices.iter().filter(|d| d.l == 500).collect();
        assert_eq!(tx.len(), 4, "{:?}", x.devices.iter().map(|d| (d.w, d.l)).collect::<Vec<_>>());
        assert!(tx.iter().all(|d| d.w == 1440));
        // A photodiode too big for the pitch raises it.
        let big = def.generate(&p.tech, &[("pd".into(), 2.0)]).unwrap();
        let f = big.shapes.iter().find(|s| s.layer == b).unwrap().geom.bbox();
        assert_eq!(f.width(), 2 * (2000 + 1350));
    }

    #[test]
    fn pixel_array_drc_and_extraction() {
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "pixel_array")).and_then(|c| c.layout.as_ref()).expect("array layout");
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().take(10).map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_eq!(x.devices.len() as i64, 4 * PIX_ROWS * PIX_COLS);
        let labelled = x.nets.iter().filter(|n| n.labelled).count() as i64;
        assert_eq!(labelled, 1 + 3 * PIX_ROWS + PIX_COLS);
    }
}
