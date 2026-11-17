//! Schematic database: device instances, wires and net labels on an integer grid,
//! plus the built-in primitive symbol library.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::geom::{Orient, Pt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DeviceKind {
    Resistor,
    Capacitor,
    Inductor,
    VSource,
    ISource,
    Diode,
    Nmos,
    Pmos,
    Ground,
    Supply,
    /// NMOS without a body pin: the body connects to the net named by its `bulk` parameter.
    Nmos3,
    /// PMOS without a body pin: the body connects to the net named by its `bulk` parameter.
    Pmos3,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 12] = [
        DeviceKind::Resistor,
        DeviceKind::Capacitor,
        DeviceKind::Inductor,
        DeviceKind::VSource,
        DeviceKind::ISource,
        DeviceKind::Diode,
        DeviceKind::Nmos,
        DeviceKind::Pmos,
        DeviceKind::Nmos3,
        DeviceKind::Pmos3,
        DeviceKind::Ground,
        DeviceKind::Supply,
    ];

    /// The kinds of the device library: one NMOS and one PMOS, placed with or without a body pin
    /// as the user prefers ([`with_body_pin`](Self::with_body_pin)).
    pub const PALETTE: [DeviceKind; 10] = [
        DeviceKind::Resistor,
        DeviceKind::Capacitor,
        DeviceKind::Inductor,
        DeviceKind::VSource,
        DeviceKind::ISource,
        DeviceKind::Diode,
        DeviceKind::Nmos,
        DeviceKind::Pmos,
        DeviceKind::Ground,
        DeviceKind::Supply,
    ];

    /// This MOS kind with (`true`) or without a body pin; other kinds are returned unchanged.
    pub fn with_body_pin(self, body: bool) -> DeviceKind {
        match (self, body) {
            (DeviceKind::Nmos | DeviceKind::Nmos3, true) => DeviceKind::Nmos,
            (DeviceKind::Pmos | DeviceKind::Pmos3, true) => DeviceKind::Pmos,
            (DeviceKind::Nmos | DeviceKind::Nmos3, false) => DeviceKind::Nmos3,
            (DeviceKind::Pmos | DeviceKind::Pmos3, false) => DeviceKind::Pmos3,
            (k, _) => k,
        }
    }

    /// A MOS transistor, with or without a body pin.
    pub fn is_mos(self) -> bool {
        matches!(self, DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3)
    }

    /// A PMOS transistor, with or without a body pin.
    pub fn is_pmos(self) -> bool {
        matches!(self, DeviceKind::Pmos | DeviceKind::Pmos3)
    }

    /// A MOS transistor whose body is not a pin but the net of its `bulk` parameter.
    pub fn implicit_bulk(self) -> bool {
        matches!(self, DeviceKind::Nmos3 | DeviceKind::Pmos3)
    }

    /// The net the body of an [`implicit_bulk`](Self::implicit_bulk) device connects to when
    /// its `bulk` parameter is empty: ground for NMOS, `vdd` for PMOS.
    pub fn default_bulk(self) -> &'static str {
        if self.is_pmos() { "vdd" } else { "0" }
    }

    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Resistor => "Resistor",
            DeviceKind::Capacitor => "Capacitor",
            DeviceKind::Inductor => "Inductor",
            DeviceKind::VSource => "Voltage Source",
            DeviceKind::ISource => "Current Source",
            DeviceKind::Diode => "Diode",
            DeviceKind::Nmos => "NMOS",
            DeviceKind::Pmos => "PMOS",
            DeviceKind::Nmos3 => "NMOS (3-terminal)",
            DeviceKind::Pmos3 => "PMOS (3-terminal)",
            DeviceKind::Ground => "Ground",
            DeviceKind::Supply => "Supply Net",
        }
    }

    /// Keyboard shortcut used by the schematic editor to place this device.
    pub fn shortcut(self) -> Option<char> {
        Some(match self {
            DeviceKind::Nmos3 | DeviceKind::Pmos3 => return None,
            DeviceKind::Resistor => 'R',
            DeviceKind::Capacitor => 'C',
            DeviceKind::Inductor => 'L',
            DeviceKind::VSource => 'V',
            DeviceKind::ISource => 'I',
            DeviceKind::Diode => 'D',
            DeviceKind::Nmos => 'N',
            DeviceKind::Pmos => 'P',
            DeviceKind::Ground => 'G',
            DeviceKind::Supply => 'S',
        })
    }

    /// SPICE element letter (None for pure connectivity symbols).
    pub fn spice_prefix(self) -> Option<char> {
        match self {
            DeviceKind::Resistor => Some('R'),
            DeviceKind::Capacitor => Some('C'),
            DeviceKind::Inductor => Some('L'),
            DeviceKind::VSource => Some('V'),
            DeviceKind::ISource => Some('I'),
            DeviceKind::Diode => Some('D'),
            DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => Some('M'),
            DeviceKind::Ground | DeviceKind::Supply => None,
        }
    }

    pub fn name_prefix(self) -> &'static str {
        match self {
            DeviceKind::Resistor => "R",
            DeviceKind::Capacitor => "C",
            DeviceKind::Inductor => "L",
            DeviceKind::VSource => "V",
            DeviceKind::ISource => "I",
            DeviceKind::Diode => "D",
            DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => "M",
            DeviceKind::Ground => "GND",
            DeviceKind::Supply => "VDD",
        }
    }

    /// Parameter names and default values.
    pub fn default_params(self) -> Vec<(&'static str, &'static str)> {
        match self {
            DeviceKind::Resistor => vec![("r", "1k")],
            DeviceKind::Capacitor => vec![("c", "1p")],
            DeviceKind::Inductor => vec![("l", "1n")],
            DeviceKind::VSource => vec![("dc", "1"), ("ac", ""), ("tran", "")],
            DeviceKind::ISource => vec![("dc", "1m"), ("ac", ""), ("tran", "")],
            DeviceKind::Diode => vec![("model", "dmod")],
            DeviceKind::Nmos => vec![("model", "nmos"), ("w", "1u"), ("l", "180n"), ("m", "1")],
            DeviceKind::Pmos => vec![("model", "pmos"), ("w", "2u"), ("l", "180n"), ("m", "1")],
            DeviceKind::Nmos3 => vec![("model", "nmos"), ("w", "1u"), ("l", "180n"), ("m", "1"), ("bulk", "0")],
            DeviceKind::Pmos3 => vec![("model", "pmos"), ("w", "2u"), ("l", "180n"), ("m", "1"), ("bulk", "vdd")],
            DeviceKind::Ground => vec![],
            DeviceKind::Supply => vec![("net", "vdd")],
        }
    }

    /// The parameter displayed next to the symbol.
    pub fn primary_param(self) -> Option<&'static str> {
        match self {
            DeviceKind::Resistor => Some("r"),
            DeviceKind::Capacitor => Some("c"),
            DeviceKind::Inductor => Some("l"),
            DeviceKind::VSource | DeviceKind::ISource => Some("dc"),
            DeviceKind::Diode => Some("model"),
            DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => None,
            DeviceKind::Ground => None,
            DeviceKind::Supply => Some("net"),
        }
    }

    pub fn symbol(self) -> &'static Symbol {
        symbols::get(self)
    }
}

/// A drawing primitive in symbol-local coordinates (grid units, y down).
#[derive(Debug, Clone)]
pub enum Prim {
    Line(f32, f32, f32, f32),
    Circle(f32, f32, f32),
    Poly(Vec<(f32, f32)>),
    /// Filled closed polygon.
    Fill(Vec<(f32, f32)>),
}

#[derive(Debug, Clone)]
pub struct SymPin {
    pub name: &'static str,
    pub at: Pt,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub pins: Vec<SymPin>,
    pub prims: Vec<Prim>,
    /// Local bounding box (min_x, min_y, max_x, max_y) used for hit testing.
    pub bbox: (f32, f32, f32, f32),
    /// Where the instance name / value labels are anchored (local coords).
    pub label_at: (f32, f32),
}

mod symbols {
    use super::*;
    use std::sync::OnceLock;

    fn two_pin(body: Vec<Prim>, half_w: f32) -> Symbol {
        let mut prims = body;
        prims.insert(0, Prim::Line(0.0, -2.0, 0.0, -1.0));
        prims.insert(1, Prim::Line(0.0, 1.0, 0.0, 2.0));
        Symbol {
            pins: vec![SymPin { name: "p", at: Pt::new(0, -2) }, SymPin { name: "n", at: Pt::new(0, 2) }],
            prims,
            bbox: (-half_w, -2.0, half_w, 2.0),
            label_at: (half_w + 0.3, -0.6),
        }
    }

    fn resistor() -> Symbol {
        let mut pts = vec![(0.0, -1.0)];
        let n = 6;
        for i in 0..n {
            let y = -1.0 + (i as f32 + 0.5) * (2.0 / n as f32);
            let x = if i % 2 == 0 { 0.4 } else { -0.4 };
            pts.push((x, y));
        }
        pts.push((0.0, 1.0));
        two_pin(vec![Prim::Poly(pts)], 0.6)
    }

    fn capacitor() -> Symbol {
        let mut s = two_pin(
            vec![
                Prim::Line(0.0, -1.0, 0.0, -0.25),
                Prim::Line(0.0, 0.25, 0.0, 1.0),
                Prim::Line(-0.8, -0.25, 0.8, -0.25),
                Prim::Line(-0.8, 0.25, 0.8, 0.25),
            ],
            0.9,
        );
        s.label_at = (1.0, -0.6);
        s
    }

    fn inductor() -> Symbol {
        let mut prims = vec![];
        for k in 0..4 {
            let cy = -0.75 + k as f32 * 0.5;
            let mut arc = vec![];
            for i in 0..=8 {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 8.0 * std::f32::consts::PI;
                arc.push((0.25 * a.cos(), cy + 0.25 * a.sin()));
            }
            prims.push(Prim::Poly(arc));
        }
        two_pin(prims, 0.5)
    }

    fn vsource() -> Symbol {
        two_pin(
            vec![
                Prim::Circle(0.0, 0.0, 1.0),
                Prim::Line(0.0, -0.65, 0.0, -0.25),
                Prim::Line(-0.2, -0.45, 0.2, -0.45),
                Prim::Line(-0.2, 0.45, 0.2, 0.45),
            ],
            1.0,
        )
    }

    fn isource() -> Symbol {
        two_pin(
            vec![
                Prim::Circle(0.0, 0.0, 1.0),
                Prim::Line(0.0, -0.6, 0.0, 0.5),
                Prim::Fill(vec![(0.0, 0.65), (-0.22, 0.25), (0.22, 0.25)]),
            ],
            1.0,
        )
    }

    fn diode() -> Symbol {
        let mut s = two_pin(
            vec![
                Prim::Line(0.0, -1.0, 0.0, -0.35),
                Prim::Line(0.0, 0.35, 0.0, 1.0),
                Prim::Fill(vec![(-0.5, -0.35), (0.5, -0.35), (0.0, 0.35)]),
                Prim::Line(-0.5, 0.35, 0.5, 0.35),
            ],
            0.7,
        );
        s.pins[0].name = "a";
        s.pins[1].name = "k";
        s
    }

    fn mos(pmos: bool) -> Symbol {
        let mut prims = vec![
            // channel
            Prim::Line(0.0, -1.0, 0.0, 1.0),
            // gate plate
            Prim::Line(-0.45, -0.8, -0.45, 0.8),
            // drain
            Prim::Poly(vec![(0.0, -0.7), (1.0, -0.7), (1.0, -2.0)]),
            // source
            Prim::Poly(vec![(0.0, 0.7), (1.0, 0.7), (1.0, 2.0)]),
            // bulk
            Prim::Line(0.0, 0.0, 2.0, 0.0),
        ];
        if pmos {
            prims.push(Prim::Circle(-0.75, 0.0, 0.25));
            prims.push(Prim::Line(-2.0, 0.0, -1.0, 0.0));
            prims.push(Prim::Fill(vec![(0.75, 0.0), (0.4, -0.2), (0.4, 0.2)]));
        } else {
            prims.push(Prim::Line(-2.0, 0.0, -0.45, 0.0));
            prims.push(Prim::Fill(vec![(0.15, 0.0), (0.5, -0.2), (0.5, 0.2)]));
        }
        let (d, s) = if pmos { (Pt::new(1, 2), Pt::new(1, -2)) } else { (Pt::new(1, -2), Pt::new(1, 2)) };
        Symbol {
            pins: vec![
                SymPin { name: "d", at: d },
                SymPin { name: "g", at: Pt::new(-2, 0) },
                SymPin { name: "s", at: s },
                SymPin { name: "b", at: Pt::new(2, 0) },
            ],
            prims,
            bbox: (-2.0, -2.0, 2.0, 2.0),
            label_at: (2.3, -1.9),
        }
    }

    /// MOS without a body pin. The arrow sits on the source lead: out of the channel for NMOS,
    /// into it for PMOS.
    fn mos3(pmos: bool) -> Symbol {
        let mut s = mos(pmos);
        s.pins.retain(|p| p.name != "b");
        s.prims.retain(|p| {
            !matches!(p, Prim::Fill(_))
                && !matches!(p, Prim::Line(x0, y0, x1, y1) if *x0 == 0.0 && *y0 == 0.0 && *x1 == 2.0 && *y1 == 0.0)
        });
        s.prims.push(if pmos {
            Prim::Fill(vec![(0.15, -0.7), (0.5, -0.9), (0.5, -0.5)])
        } else {
            Prim::Fill(vec![(0.8, 0.7), (0.45, 0.5), (0.45, 0.9)])
        });
        s.bbox = (-2.0, -2.0, 1.2, 2.0);
        s.label_at = (1.5, -1.9);
        s
    }

    fn ground() -> Symbol {
        Symbol {
            pins: vec![SymPin { name: "gnd", at: Pt::new(0, 0) }],
            prims: vec![
                Prim::Line(0.0, 0.0, 0.0, 0.5),
                Prim::Line(-0.8, 0.5, 0.8, 0.5),
                Prim::Line(-0.5, 0.8, 0.5, 0.8),
                Prim::Line(-0.2, 1.1, 0.2, 1.1),
            ],
            bbox: (-0.8, 0.0, 0.8, 1.2),
            label_at: (1.0, 0.3),
        }
    }

    fn supply() -> Symbol {
        Symbol {
            pins: vec![SymPin { name: "net", at: Pt::new(0, 0) }],
            prims: vec![Prim::Line(0.0, 0.0, 0.0, -0.8), Prim::Line(-0.7, -0.8, 0.7, -0.8)],
            bbox: (-0.8, -1.0, 0.8, 0.0),
            label_at: (-0.6, -2.0),
        }
    }

    pub fn get(kind: DeviceKind) -> &'static Symbol {
        static LIB: OnceLock<Vec<(DeviceKind, Symbol)>> = OnceLock::new();
        let lib = LIB.get_or_init(|| {
            vec![
                (DeviceKind::Resistor, resistor()),
                (DeviceKind::Capacitor, capacitor()),
                (DeviceKind::Inductor, inductor()),
                (DeviceKind::VSource, vsource()),
                (DeviceKind::ISource, isource()),
                (DeviceKind::Diode, diode()),
                (DeviceKind::Nmos, mos(false)),
                (DeviceKind::Pmos, mos(true)),
                (DeviceKind::Nmos3, mos3(false)),
                (DeviceKind::Pmos3, mos3(true)),
                (DeviceKind::Ground, ground()),
                (DeviceKind::Supply, supply()),
            ]
        });
        &lib.iter().find(|(k, _)| *k == kind).expect("symbol defined").1
    }
}

/// Instance property marking a test-bench part ([`Instance::lvs_ignore`]).
pub const LVS_IGNORE: &str = "lvsIgnore";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: u64,
    pub kind: DeviceKind,
    pub name: String,
    pub pos: Pt,
    #[serde(default)]
    pub orient: Orient,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

impl Instance {
    pub fn param(&self, key: &str) -> &str {
        self.params.get(key).map(String::as_str).unwrap_or("")
    }

    /// Test-bench part without a layout counterpart: the `lvsIgnore` property (as in Virtuoso)
    /// is `t`, `true` or `1`. LVS and layout generation skip it; it is still simulated.
    pub fn lvs_ignore(&self) -> bool {
        matches!(self.param(LVS_IGNORE).trim().to_ascii_lowercase().as_str(), "t" | "true" | "1" | "yes")
    }

    /// World position of each pin, in symbol pin order.
    pub fn pin_positions(&self) -> Vec<(&'static str, Pt)> {
        self.kind.symbol().pins.iter().map(|p| (p.name, self.pos + self.orient.apply(p.at))).collect()
    }

    pub fn to_world_f(&self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = self.orient.apply_f(x, y);
        (x + self.pos.x as f32, y + self.pos.y as f32)
    }

    /// World space bounding box (min_x, min_y, max_x, max_y).
    pub fn bbox(&self) -> (f32, f32, f32, f32) {
        let b = self.kind.symbol().bbox;
        let corners = [(b.0, b.1), (b.2, b.1), (b.0, b.3), (b.2, b.3)];
        let mut r = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in corners {
            let (wx, wy) = self.to_world_f(x, y);
            r.0 = r.0.min(wx);
            r.1 = r.1.min(wy);
            r.2 = r.2.max(wx);
            r.3 = r.3.max(wy);
        }
        r
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    pub id: u64,
    pub a: Pt,
    pub b: Pt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetLabel {
    pub id: u64,
    pub name: String,
    pub pos: Pt,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Schematic {
    #[serde(default)]
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    #[serde(default)]
    pub labels: Vec<NetLabel>,
    #[serde(default)]
    pub next_id: u64,
}

/// Reference to any object in a schematic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SchObj {
    Instance(u64),
    Wire(u64),
    Label(u64),
}

impl Schematic {
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Next free instance name such as `R3`.
    pub fn next_name(&self, kind: DeviceKind) -> String {
        let prefix = kind.name_prefix();
        let mut n = 0;
        for inst in &self.instances {
            if let Some(rest) = inst.name.strip_prefix(prefix)
                && let Ok(k) = rest.parse::<u32>()
            {
                n = n.max(k);
            }
        }
        format!("{prefix}{}", n + 1)
    }

    pub fn add_instance(&mut self, kind: DeviceKind, pos: Pt, orient: Orient) -> u64 {
        let id = self.alloc_id();
        let name = self.next_name(kind);
        let params = kind.default_params().into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.instances.push(Instance { id, kind, name, pos, orient, params });
        id
    }

    /// Add a wire, splitting is not required: connectivity treats T-junctions on segments.
    pub fn add_wire(&mut self, a: Pt, b: Pt) -> Option<u64> {
        if a == b {
            return None;
        }
        let id = self.alloc_id();
        self.wires.push(Wire { id, a, b });
        Some(id)
    }

    pub fn add_label(&mut self, name: impl Into<String>, pos: Pt) -> u64 {
        let id = self.alloc_id();
        self.labels.push(NetLabel { id, name: name.into(), pos });
        id
    }

    pub fn instance(&self, id: u64) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// Draw MOS instance `id` with (`true`) or without its body pin, keeping its connections:
    /// hiding the pin moves the body's net into the `bulk` parameter (the default rail when the
    /// pin was not wired) and removes the wires left dangling from it; showing it drops the
    /// parameter, so the pin must then be wired (an unwired body is tied to the source). Returns
    /// whether the instance changed.
    pub fn set_body_pin(&mut self, id: u64, body: bool) -> bool {
        let Some(inst) = self.instance(id) else { return false };
        let kind = inst.kind.with_body_pin(body);
        if kind == inst.kind {
            return false;
        }
        let body_at = inst.pin_positions().into_iter().find(|(n, _)| *n == "b").map(|(_, p)| p);
        let bulk = (!body).then(|| {
            let conn = crate::connectivity::extract(self);
            let floating = body_at.is_some_and(|p| conn.unconnected_pins.contains(&p));
            match conn.net_of(id, "b") {
                Some(n) if !floating => n.to_string(),
                _ => kind.default_bulk().to_string(),
            }
        });
        let inst = self.instance_mut(id).expect("instance");
        inst.kind = kind;
        match bulk {
            Some(b) => inst.params.insert("bulk".into(), b),
            None => inst.params.remove("bulk"),
        };
        if let Some(p) = body_at.filter(|_| !body) {
            self.remove_dangling_from(p);
        }
        true
    }

    /// Remove the wire chain that ends at `p` with nothing else there, up to the first pin,
    /// label or junction.
    fn remove_dangling_from(&mut self, p: Pt) {
        let mut ends = vec![p];
        while let Some(p) = ends.pop() {
            let held = self.instances.iter().any(|i| i.pin_positions().iter().any(|(_, q)| *q == p))
                || self.labels.iter().any(|l| l.pos == p)
                || self.wires.iter().any(|w| w.a != p && w.b != p && crate::geom::on_segment(p, w.a, w.b));
            let touching: Vec<usize> =
                self.wires.iter().enumerate().filter(|(_, w)| w.a == p || w.b == p).map(|(k, _)| k).collect();
            if held || touching.len() != 1 {
                continue;
            }
            let w = self.wires.remove(touching[0]);
            ends.push(if w.a == p { w.b } else { w.a });
        }
    }

    pub fn instance_mut(&mut self, id: u64) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }

    pub fn instance_by_name(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name.eq_ignore_ascii_case(name))
    }

    pub fn remove(&mut self, obj: SchObj) {
        match obj {
            SchObj::Instance(id) => self.instances.retain(|i| i.id != id),
            SchObj::Wire(id) => self.wires.retain(|w| w.id != id),
            SchObj::Label(id) => self.labels.retain(|l| l.id != id),
        }
    }

    pub fn translate(&mut self, obj: SchObj, d: Pt) {
        match obj {
            SchObj::Instance(id) => {
                if let Some(i) = self.instance_mut(id) {
                    i.pos = i.pos + d;
                }
            }
            SchObj::Wire(id) => {
                if let Some(w) = self.wires.iter_mut().find(|w| w.id == id) {
                    w.a = w.a + d;
                    w.b = w.b + d;
                }
            }
            SchObj::Label(id) => {
                if let Some(l) = self.labels.iter_mut().find(|l| l.id == id) {
                    l.pos = l.pos + d;
                }
            }
        }
    }

    /// Merge collinear, overlapping wires and drop zero-length ones.
    pub fn cleanup_wires(&mut self) {
        self.wires.retain(|w| w.a != w.b);
        let mut seen = std::collections::HashSet::new();
        self.wires.retain(|w| {
            let key = if w.a <= w.b { (w.a, w.b) } else { (w.b, w.a) };
            seen.insert(key)
        });
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.wires.is_empty() && self.labels.is_empty()
    }
}
