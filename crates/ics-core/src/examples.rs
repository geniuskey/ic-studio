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
    if let Some(Analysis::Tran { tstep, tstop }) = sim.get_mut("tran") {
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

/// 6T SRAM bit cell pitch: abutting cells share word lines, rails and bit lines.
const SRAM_X: (i64, i64) = (-400, 7600);
const SRAM_Y: (i64, i64) = (-1000, 6800);
const SRAM_T: Tracks = Tracks { wn: 1000, wp: 1000, mid: 3 };
const SRAM_ROWS: i64 = 8;
const SRAM_COLS: i64 = 8;

/// 6T SRAM bit cell: cross-coupled inverters (P1/N1, P2/N2) and access devices A1, A2. Word
/// line and rails run horizontally on metal2 edge to edge, the bit lines vertically on metal3.
pub fn sram6t(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    let devices = [
        (DeviceKind::Nmos, (0, 10), ["bl", "wl", "q"]),
        (DeviceKind::Pmos, (8, 4), ["q", "qb", "vdd"]),
        (DeviceKind::Nmos, (8, 10), ["q", "qb", "gnd"]),
        (DeviceKind::Pmos, (16, 4), ["qb", "q", "vdd"]),
        (DeviceKind::Nmos, (16, 10), ["qb", "q", "gnd"]),
        (DeviceKind::Nmos, (24, 10), ["blb", "wl", "qb"]),
    ];
    for (kind, at, nets) in devices {
        let id = mos(&mut s, kind, at, false, "1u", "180n", None);
        let pins = s.instance(id).expect("instance").pin_positions();
        for (pin, net) in ["d", "g", "s"].into_iter().zip(nets) {
            let at = pins.iter().find(|(n, _)| *n == pin).expect("MOS pin").1;
            match net {
                "vdd" => {
                    s.add_instance(DeviceKind::Supply, at, o);
                }
                "gnd" => {
                    s.add_instance(DeviceKind::Ground, at, o);
                }
                _ => {
                    s.add_label(net, at);
                }
            }
        }
    }
    source(&mut s, (-24, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-18, 8), "wl", &[("dc", "0"), ("tran", "PULSE(0 1.8 1n 50p 50p 1n 4n)")]);
    source(&mut s, (-12, 8), "bl", &[("dc", "0"), ("tran", "PULSE(0 1.8 2.5n 50p 50p 4n 8n)")]);
    source(&mut s, (-6, 8), "blb", &[("dc", "1.8"), ("tran", "PULSE(1.8 0 2.5n 50p 50p 4n 8n)")]);
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "10p", "8n");

    let (t, (x0, x1), (y0, y1)) = (SRAM_T, SRAM_X, SRAM_Y);
    let mut r = Router::new(&p.tech);
    place_columns(&mut r, p, t, GATE_COL, &[("", NW, "A1", NW), ("P1", NW, "N1", NW), ("P2", NW, "N2", NW), ("", NW, "A2", NW)]);
    let (m2, m3) = (r.layer("metal2"), r.layer("metal3"));
    let h = WIRE / 2;

    r.tap("A1", "G", t.mid(0));
    r.tap("A2", "G", t.mid(0));
    r.lay.add_rect(m2, x0, t.mid(0) - h, x1, t.mid(0) + h);
    r.label("wl", (x0 + 200, t.mid(0)));

    for (track, access, gates, drains, net) in
        [(1, "A1", ("N2", "P2"), ("N1", "P1"), "q"), (2, "A2", ("N1", "P1"), ("N2", "P2"), "qb")]
    {
        let y = t.mid(track);
        let a = r.centre_tap(access, "S");
        let g = r.tap(gates.0, "G", y);
        r.tap(gates.1, "G", y);
        r.route(&[a, (a.0, y), g]);
        let nd = r.centre_tap(drains.0, "D");
        let pd = r.centre_tap(drains.1, "D");
        let joint = (nd.0, y);
        r.route(&[nd, joint]);
        r.route(&[joint, pd]);
        r.label(net, joint);
    }

    for (dev, net) in [("A1", "bl"), ("A2", "blb")] {
        let d = r.centre_tap(dev, "D");
        r.via("via2", d);
        r.lay.add_rect(m3, d.0 - h, y0, d.0 + h, y1);
        r.label(net, d);
    }

    for (net, y, taps) in [("vdd", t.top(0), [("P1", "S"), ("P2", "S")]), ("gnd", t.bottom(0), [("N1", "S"), ("N2", "S")])] {
        for (dev, pin) in taps {
            r.tap(dev, pin, y);
        }
        r.lay.add_rect(m2, x0, y - h, x1, y + h);
        r.label(net, (x0 + 200, y));
    }

    r.nwell = r.nwell.map(|n| IRect::new(x0, n.y0, x1, n.y1));
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, x0, y0, x1, y1);
    full_flow_cell("sram6t", "6T SRAM bit cell, abutting pitch for arrays (DRC / LVS clean)", s, r.finish(), sim)
}

/// 8 x 8 array of `sram6t` with labelled word lines and bit lines and strapped supplies.
pub fn sram_array(tech: &Tech, lib: &str) -> Cell {
    let (t, (x0, x1), (y0, y1)) = (SRAM_T, SRAM_X, SRAM_Y);
    let (pitch_x, pitch_y) = (x1 - x0, y1 - y0);
    let mut r = Router::new(tech);
    r.lay.add_instance(
        CellRef::new(lib, "sram6t"),
        Trans::new(0, 0),
        Some(ArraySpec { cols: SRAM_COLS as u32, rows: SRAM_ROWS as u32, col: (pitch_x, 0), row: (0, pitch_y) }),
    );
    let last = (SRAM_ROWS - 1) * pitch_y;
    for (net, strap, y) in [("gnd", x0 - 1300, t.bottom(0)), ("vdd", x0 - 500, t.top(0))] {
        for row in 0..SRAM_ROWS {
            let at = (strap, y + row * pitch_y);
            r.route(&[at, (x0, at.1)]);
            r.via("via2", at);
        }
        r.route(&[(strap, y), (strap, y + last)]);
        r.label(net, (strap, y));
    }
    for row in 0..SRAM_ROWS {
        r.label(&format!("wl{row}"), (x0 + 200, t.mid(0) + row * pitch_y));
    }
    let drain = crate::pcell::mos_columns(tech, NW.1, 1).expect("MOS columns")[1];
    for col in 0..SRAM_COLS {
        let x = col * pitch_x + drain;
        r.label(&format!("bl{col}"), (x, y0 + 100));
        r.label(&format!("blb{col}"), (x + 3 * GATE_COL, y0 + 100));
    }
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, x0 - 2000, y0, x0 + SRAM_COLS * pitch_x, y0 + SRAM_ROWS * pitch_y);
    layout_cell("sram_array", "8 x 8 array of 6T SRAM bit cells (AREF)", r.finish())
}

/// 3T pixel pitch: abutting pixels share the reset / select lines, the supply and the column line.
const PIX_X: (i64, i64) = (-3700, 6500);
const PIX_Y: (i64, i64) = (-1000, 9200);
const PIX_T: Tracks = Tracks { wn: 1000, wp: 0, mid: 3 };
const PIX_COL: i64 = 2400;
const PIX_SF: (i64, i64) = (1000, 500);
/// n+ / p-substrate photodiode diffusion, merged with the reset source (`as` / `ps` of RST).
const PIX_PD: IRect = IRect { x0: -3300, y0: -400, x1: -900, y1: 8400 };
const PIX_ROWS: i64 = 8;
const PIX_COLS: i64 = 8;

/// 3T active pixel (image sensor): reset RST, source follower SF and row select SEL over an
/// n+ / p-substrate photodiode. The schematic models the photodiode by its photocurrent; the
/// integrating node `pd` is the SF gate. Reset and select run horizontally on metal2 edge to
/// edge, the column line vertically on metal3.
pub fn aps3t(p: &mut Project) -> Cell {
    let o = Orient::default();
    let mut s = Schematic::default();
    let devices =
        [((0, 10), "180n", ["vdd", "rst", "pd"]), ((8, 10), "500n", ["vdd", "pd", "x"]), ((16, 10), "180n", ["col", "sel", "x"])];
    for (k, (at, l, nets)) in devices.into_iter().enumerate() {
        let id = mos(&mut s, DeviceKind::Nmos, at, false, "1u", l, None);
        if k == 0 {
            set(&mut s, id, &[("as", "21p"), ("ps", "22u")]);
        }
        let pins = s.instance(id).expect("instance").pin_positions();
        for (pin, net) in ["d", "g", "s"].into_iter().zip(nets) {
            let at = pins.iter().find(|(n, _)| *n == pin).expect("MOS pin").1;
            if net == "vdd" {
                s.add_instance(DeviceKind::Supply, at, o);
            } else {
                s.add_label(net, at);
            }
        }
    }
    source(&mut s, (-24, 8), "vdd", &[("dc", "1.8")]);
    source(&mut s, (-18, 8), "rst", &[("dc", "1.8"), ("tran", "PULSE(1.8 0 50n 1n 1n 400n 600n)")]);
    source(&mut s, (-12, 8), "sel", &[("dc", "1.8")]);
    for (x, net, i) in [(24, "pd", "30n"), (30, "col", "2u")] {
        let id = s.add_instance(DeviceKind::ISource, Pt::new(x, 8), o);
        set(&mut s, id, &[("dc", i)]);
        s.add_label(net, Pt::new(x, 6));
        s.add_instance(DeviceKind::Ground, Pt::new(x, 10), o);
    }
    let mut sim = sim(&["tran"]);
    set_tran(&mut sim, "1n", "600n");

    let (t, (x0, x1), (y0, y1)) = (PIX_T, PIX_X, PIX_Y);
    let mut r = Router::new(&p.tech);
    place_columns(&mut r, p, t, PIX_COL, &[("", NW, "RST", NW), ("", NW, "SF", PIX_SF), ("", NW, "SEL", NW)]);
    let (m1, m2, m3) = (r.layer("metal1"), r.layer("metal2"), r.layer("metal3"));
    let h = WIRE / 2;

    for (dev, track, net) in [("RST", 0, "rst"), ("SEL", 1, "sel")] {
        let y = t.mid(track);
        r.tap(dev, "G", y);
        r.lay.add_rect(m2, x0, y - h, x1, y + h);
        r.label(net, (x0 + 200, y));
    }

    let a = r.centre_tap("RST", "S");
    let g = r.tap("SF", "G", t.mid(2));
    r.route(&[a, (a.0, t.mid(2)), g]);
    r.label("pd", g);

    let sf = r.centre_tap("SF", "S");
    let sel = r.centre_tap("SEL", "S");
    r.route(&[sf, sel]);
    r.label("x", sf);

    let d = r.centre_tap("SEL", "D");
    r.via("via2", d);
    r.lay.add_rect(m3, d.0 - h, y0, d.0 + h, y1);
    r.label("col", d);

    let y = t.bottom(0);
    r.tap("RST", "D", y);
    r.tap("SF", "D", y);
    r.lay.add_rect(m2, x0, y - h, x1, y + h);
    r.label("vdd", (x0 + 200, y));

    let (active, nplus) = (r.layer("active"), r.layer("nplus"));
    let enc = p.tech.rules.implant_enc;
    r.lay.add(active, Geometry::Rect(PIX_PD));
    r.lay.add_rect(nplus, PIX_PD.x0 - enc, PIX_PD.y0 - enc, PIX_PD.x1 + enc, PIX_PD.y1 + enc);
    let contact = (PIX_PD.x1 - 300, a.1);
    r.via("contact", contact);
    r.lay.add_rect(m1, contact.0 - h, a.1 - h, a.0, a.1 + h);

    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, x0, y0, x1, y1);
    full_flow_cell("aps3t", "3T active pixel with n+ / p-sub photodiode, abutting pitch (DRC / LVS clean)", s, r.finish(), sim)
}

/// 8 x 8 image sensor pixel array of `aps3t`: labelled reset / select rows and column lines.
pub fn pixel_array(tech: &Tech, lib: &str) -> Cell {
    let (t, (x0, x1), (y0, y1)) = (PIX_T, PIX_X, PIX_Y);
    let (pitch_x, pitch_y) = (x1 - x0, y1 - y0);
    let mut r = Router::new(tech);
    r.lay.add_instance(
        CellRef::new(lib, "aps3t"),
        Trans::new(0, 0),
        Some(ArraySpec { cols: PIX_COLS as u32, rows: PIX_ROWS as u32, col: (pitch_x, 0), row: (0, pitch_y) }),
    );
    let (strap, y) = (x0 - 500, t.bottom(0));
    for row in 0..PIX_ROWS {
        let at = (strap, y + row * pitch_y);
        r.route(&[at, (x0, at.1)]);
        r.via("via2", at);
    }
    r.route(&[(strap, y), (strap, y + (PIX_ROWS - 1) * pitch_y)]);
    r.label("vdd", (strap, y));
    for row in 0..PIX_ROWS {
        r.label(&format!("rst{row}"), (x0 + 200, t.mid(0) + row * pitch_y));
        r.label(&format!("sel{row}"), (x0 + 200, t.mid(1) + row * pitch_y));
    }
    let drain = 2 * PIX_COL + crate::pcell::mos_columns(tech, NW.1, 1).expect("MOS columns")[1];
    for col in 0..PIX_COLS {
        r.label(&format!("col{col}"), (col * pitch_x + drain, y0 + 100));
    }
    let boundary = r.layer("boundary");
    r.lay.add_rect(boundary, x0 - 1000, y0, x0 + PIX_COLS * pitch_x, y0 + PIX_ROWS * pitch_y);
    layout_cell("pixel_array", "8 x 8 image sensor array of 3T pixels (AREF)", r.finish())
}

pub fn demo_project() -> Project {
    let mut p = Project::new("demo");
    p.description = "Example designs shipped with IC Studio".into();
    let hier = hierarchy_cells(&p.tech, "work");
    for c in [cmos_inverter(), rc_lowpass(), diode_clipper()].into_iter().chain(hier) {
        p.add_cell("work", c).expect("unique");
    }
    for make in [nand2, nor2, ring_osc5, current_mirror, ota5t, sram6t, aps3t] {
        let c = make(&mut p);
        p.add_cell("work", c).expect("unique");
    }
    for c in [sram_array(&p.tech, "work"), pixel_array(&p.tech, "work")] {
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
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::extract::extract;
    use crate::lvs::{LvsOptions, compare};
    use crate::sdl::ratsnest;

    fn full_flow(name: &str) {
        let p = demo_project();
        let cell = p.cell(&CellRef::new("work", name)).expect("demo cell");
        let (sch, lay) = (cell.schematic.as_ref().unwrap(), cell.layout.as_ref().unwrap());
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{name}: {:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{name}: {:?}", x.warnings);
        let r = compare(sch, &x, &LvsOptions::for_tech(&p.tech));
        assert!(r.matched, "{name}: {:#?}", r.errors);
        assert_eq!(
            r.layout_devices,
            sch.instances.iter().filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos)).count()
        );
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
    fn aps3t_full_flow() {
        full_flow("aps3t");
    }

    #[test]
    fn pixel_array_drc_and_extraction() {
        let p = demo_project();
        let lay = p.cell(&CellRef::new("work", "pixel_array")).and_then(|c| c.layout.as_ref()).expect("array layout");
        let v = run_tech_deck(lay, &p, &p.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().take(10).map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract(lay, &p, &p.tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_eq!(x.devices.len() as i64, 3 * PIX_ROWS * PIX_COLS);
        let labelled = x.nets.iter().filter(|n| n.labelled).count() as i64;
        assert_eq!(labelled, 1 + 2 * PIX_ROWS + PIX_COLS);
    }
}
