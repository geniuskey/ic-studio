//! Image sensor pixel circuits from a layout extraction, and a schematic drawn from it.
//!
//! [`classify`] finds the pixel transistors around each extracted photodiode
//! ([`Extraction::photodiodes`]) by connectivity alone, so it works for any drawing of the
//! devices:
//! - **TG** (transfer gate): a MOS with its source or drain on the photodiode. The other side is
//!   the floating diffusion **FD**. When several gates sit on the photodiode, the one whose other
//!   side drives a gate is the TG; the rest (anti-blooming, overflow gates) are `other`.
//!   Without a TG, a photodiode that drives a gate itself is a 3T pixel: FD is the photodiode.
//! - Photodiodes whose TGs share one FD are one shared pixel.
//! - **SF** (source follower): the MOS gated by FD. **RST**: the MOS from FD to one side of SF,
//!   which is the supply; the other side of SF is its output. Other devices on FD (a dual
//!   conversion gain switch) are `other`.
//! - **SEL** (row select): a MOS from the follower output to the column line.
//!
//! [`schematic`] draws the extraction: each pixel in the textbook arrangement (TG, RST, SF and
//! SEL with net labels on their pins, the photodiode as its photocurrent), the other devices in
//! rows below, and optionally a test bench (supply, reset / transfer pulses, row select, column
//! current load) with a transient analysis. The photodiode junction is the TG's source / drain
//! junction (`as` / `ps`), as extracted.

use std::collections::{BTreeMap, BTreeSet};

use crate::extract::{Extraction, Junction, PassiveKind};
use crate::geom::{Orient, Pt};
use crate::pex::NetCap;
use crate::schematic::{DeviceKind, Schematic};
use crate::sim_setup::{Analysis, SimSetup};

/// One pixel's transistors (device indices into [`Extraction::devices`]) and nets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pixel {
    /// Photodiodes (indices into [`Extraction::photodiodes`]) with their transfer gates; `None`
    /// for a 3T pixel.
    pub photodiodes: Vec<(usize, Option<usize>)>,
    /// The floating diffusion (sense node) net.
    pub fd: usize,
    pub rst: Option<usize>,
    pub sf: Option<usize>,
    pub sel: Option<usize>,
    /// The supply (reset and follower drain), the follower output and the column line.
    pub supply: Option<usize>,
    pub sf_out: Option<usize>,
    pub column: Option<usize>,
    /// Other gates on a photodiode or on FD.
    pub other: Vec<usize>,
}

impl Pixel {
    /// Every device of the pixel with its role ("TG", "RST", "SF", "SEL", "other").
    pub fn roles(&self) -> Vec<(usize, &'static str)> {
        let mut v: Vec<(usize, &'static str)> = self.photodiodes.iter().filter_map(|p| p.1).map(|d| (d, "TG")).collect();
        v.extend(self.rst.map(|d| (d, "RST")));
        v.extend(self.sf.map(|d| (d, "SF")));
        v.extend(self.sel.map(|d| (d, "SEL")));
        v.extend(self.other.iter().map(|&d| (d, "other")));
        v
    }
}

#[derive(Debug, Clone, Default)]
pub struct PixelCircuit {
    pub pixels: Vec<Pixel>,
    pub warnings: Vec<String>,
}

impl PixelCircuit {
    /// The role of device `d`, if it belongs to a pixel.
    pub fn role(&self, d: usize) -> Option<&'static str> {
        self.pixels.iter().flat_map(|p| p.roles()).find(|r| r.0 == d).map(|r| r.1)
    }

    /// A readable report: per pixel its nets, the photodiodes and every transistor with W / L
    /// and junctions, in µm, and the wire capacitance of FD and the photodiodes from `caps`
    /// ([`crate::pex::net_caps`]).
    pub fn report(&self, x: &Extraction, caps: &[NetCap]) -> String {
        let u = x.dbu_per_um.max(1) as f64;
        let net = |i: usize| x.nets.get(i).map_or("?", |n| n.name.as_str());
        let opt = |i: Option<usize>| i.map_or("-", net);
        let mut s = String::new();
        if self.pixels.is_empty() {
            s += "no pixel found (no photodiode with a transfer gate or follower)\n";
        }
        for (k, p) in self.pixels.iter().enumerate() {
            s += &format!(
                "pixel {}: fd={} supply={} sf_out={} column={}\n",
                k + 1,
                net(p.fd),
                opt(p.supply),
                opt(p.sf_out),
                opt(p.column)
            );
            for &(d, tg) in &p.photodiodes {
                let pd = &x.photodiodes[d];
                s += &format!(
                    "  {} node={} well={} area={:.4} um2 perimeter={:.4} um  bbox=({:.3}, {:.3})-({:.3}, {:.3}) um  tg={}\n",
                    pd.name,
                    net(pd.node()),
                    net(pd.well()),
                    pd.area as f64 / (u * u),
                    pd.perimeter as f64 / u,
                    pd.body.x0 as f64 / u,
                    pd.body.y0 as f64 / u,
                    pd.body.x1 as f64 / u,
                    pd.body.y1 as f64 / u,
                    tg.map_or("none (3T)", |t| x.devices[t].name.as_str())
                );
            }
            for (d, role) in p.roles() {
                let m = &x.devices[d];
                s += &format!(
                    "  {:<5} {:<4} {} w={:.4}u l={:.4}u  d={} g={} s={} b={}  as={:.4}p ps={:.4}u ad={:.4}p pd={:.4}u\n",
                    role,
                    m.name,
                    m.model,
                    m.w as f64 / u,
                    m.l as f64 / u,
                    net(m.d),
                    net(m.g),
                    net(m.s),
                    net(m.b),
                    m.js.area / (u * u),
                    m.js.perimeter / u,
                    m.jd.area / (u * u),
                    m.jd.perimeter / u
                );
            }
            let (a, pe) = fd_junction(x, p);
            s += &format!("  FD junction (diffusion on {}): area={a:.4} um2 perimeter={pe:.4} um\n", net(p.fd));
            let nodes = std::iter::once(p.fd).chain(p.photodiodes.iter().map(|q| x.photodiodes[q.0].node()));
            for n in nodes.collect::<BTreeSet<_>>() {
                if let Some(c) = wire_cap(x, caps, n) {
                    s += &format!("  wire capacitance of {}: {:.3} fF\n", net(n), c * 1e15);
                }
            }
        }
        s
    }
}

/// Total source / drain junction on the floating diffusion of `p` (µm², µm). A 3T pixel's FD
/// is its photodiode.
pub fn fd_junction(x: &Extraction, p: &Pixel) -> (f64, f64) {
    let u = x.dbu_per_um.max(1) as f64;
    let mut j = Junction::default();
    for m in &x.devices {
        for (n, jn) in [(m.s, m.js), (m.d, m.jd)] {
            if n == p.fd {
                j.area += jn.area;
                j.perimeter += jn.perimeter;
            }
        }
    }
    (j.area / (u * u), j.perimeter / u)
}

fn wire_cap(x: &Extraction, caps: &[NetCap], net: usize) -> Option<f64> {
    let name = &x.nets.get(net)?.name;
    caps.iter().find(|c| &c.net == name).map(|c| c.farads).filter(|f| *f > 0.0)
}

/// The other source / drain net of device `d` when one side is on `net`.
fn other_side(x: &Extraction, d: usize, net: usize) -> Option<usize> {
    let m = &x.devices[d];
    match () {
        _ if m.s == net && m.d != net => Some(m.d),
        _ if m.d == net && m.s != net => Some(m.s),
        _ => None,
    }
}

fn looks_like_supply(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["vdd", "vpix", "vcc", "avdd", "pvdd", "vrst"].iter().any(|k| n.contains(k))
}

/// Find the pixels around the extracted photodiodes.
pub fn classify(x: &Extraction) -> PixelCircuit {
    let mut out = PixelCircuit::default();
    let gates: BTreeSet<usize> = x.devices.iter().map(|m| m.g).collect();
    let mut by_fd: BTreeMap<usize, Pixel> = BTreeMap::new();
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    for (k, pd) in x.photodiodes.iter().enumerate() {
        let node = pd.node();
        let on: Vec<(usize, usize)> = (0..x.devices.len())
            .filter(|&d| x.devices[d].g != node)
            .filter_map(|d| other_side(x, d, node).map(|o| (d, o)))
            .collect();
        let (tg, fd) = match on.iter().find(|(_, o)| gates.contains(o)) {
            Some(&(d, o)) => (Some(d), o),
            // 3T: the photodiode drives the follower itself; the devices on it are the reset.
            None if gates.contains(&node) => (None, node),
            None => match on.first() {
                Some(&(d, o)) => {
                    out.warnings.push(format!(
                        "{}: the far side of transfer gate {} ({}) drives no gate (no source follower)",
                        pd.name, x.devices[d].name, x.nets[o].name
                    ));
                    (Some(d), o)
                }
                None => {
                    out.warnings
                        .push(format!("{}: no transfer gate or follower on net {}; not a pixel", pd.name, x.nets[node].name));
                    continue;
                }
            },
        };
        let px = by_fd.entry(fd).or_insert_with(|| Pixel { fd, ..Pixel::default() });
        px.photodiodes.push((k, tg));
        taken.extend(tg);
        for &(d, _) in on.iter().filter(|_| tg.is_some()) {
            if Some(d) != tg && !px.other.contains(&d) {
                px.other.push(d);
            }
        }
    }
    taken.extend(by_fd.values().flat_map(|p| p.other.iter().copied()));
    for (fd, mut p) in by_fd {
        let sf = (0..x.devices.len()).find(|&d| x.devices[d].g == fd && !taken.contains(&d));
        if let Some(d) = sf {
            taken.insert(d);
        }
        let sides = sf.map(|d| (x.devices[d].d, x.devices[d].s));
        // Devices from FD elsewhere (not the TGs, not gated by FD): RST and others.
        let on_fd: Vec<(usize, usize)> = (0..x.devices.len())
            .filter(|d| !taken.contains(d) && x.devices[*d].g != fd)
            .filter_map(|d| other_side(x, d, fd).map(|o| (d, o)))
            .collect();
        let rst = on_fd
            .iter()
            .find(|(_, o)| sides.is_some_and(|(a, b)| *o == a || *o == b))
            .or_else(|| on_fd.iter().find(|(_, o)| looks_like_supply(&x.nets[*o].name)))
            .or(on_fd.first())
            .copied();
        let supply = match (rst, sides) {
            (Some((_, o)), Some((a, b))) if o == a || o == b => Some(o),
            (_, Some((a, b))) if looks_like_supply(&x.nets[b].name) && !looks_like_supply(&x.nets[a].name) => Some(b),
            (_, Some((a, _))) => Some(a),
            (Some((_, o)), None) => Some(o),
            (None, None) => None,
        };
        let sf_out = sides.map(|(a, b)| if Some(a) == supply { b } else { a });
        p.rst = rst.map(|r| r.0);
        p.sf = sf;
        p.supply = supply;
        p.sf_out = sf_out;
        for &(d, _) in &on_fd {
            if Some(d) != p.rst {
                p.other.push(d);
            }
        }
        taken.extend(p.rst);
        taken.extend(p.other.iter().copied());
        if let Some(out_net) = sf_out {
            let sel = (0..x.devices.len())
                .filter(|d| !taken.contains(d) && x.devices[*d].g != out_net)
                .find_map(|d| other_side(x, d, out_net).map(|o| (d, o)));
            p.sel = sel.map(|s| s.0);
            p.column = Some(sel.map_or(out_net, |s| s.1));
            taken.extend(p.sel);
        }
        if p.sf.is_none() {
            out.warnings.push(format!("pixel with fd {}: no source follower (no MOS gated by it)", x.nets[fd].name));
        }
        if p.rst.is_none() {
            out.warnings.push(format!("pixel with fd {}: no reset transistor", x.nets[fd].name));
        }
        out.pixels.push(p);
    }
    out.pixels.sort_by_key(|p| p.photodiodes.first().map(|q| (x.photodiodes[q.0].body.y0, x.photodiodes[q.0].body.x0)));
    out
}

/// Test bench of a generated pixel schematic.
#[derive(Debug, Clone, PartialEq)]
pub struct Testbench {
    /// Supply and gate high level (V).
    pub vdd: f64,
    /// Photocurrent of every photodiode (A).
    pub iph: f64,
    /// Column current load (A).
    pub icol: f64,
    /// Wire capacitance to substrate per layout net ([`crate::pex::net_caps`]): FD and the
    /// photodiode nodes get theirs as SPICE lines of the setup (simulated, not drawn, so LVS
    /// still compares the devices alone).
    pub wire_caps: Vec<NetCap>,
    pub stimulus: Stimulus,
}

impl Default for Testbench {
    fn default() -> Self {
        Testbench { vdd: 1.8, iph: 10e-9, icol: 2e-6, wire_caps: Vec::new(), stimulus: Stimulus::ResetTransfer }
    }
}

/// What the test bench drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stimulus {
    /// Reset photodiode and FD, integrate the photocurrent, release the reset (reset level on
    /// the column), then transfer (signal level).
    #[default]
    ResetTransfer,
    /// Characterisation in the dark: reset FD and every photodiode, turn RST and TX off, then
    /// take [`INJECT_ELECTRONS`] out of the first pixel's FD (in [`FD_WINDOW`]) and of its
    /// first photodiode (in [`PD_WINDOW`]). The voltage steps give the node capacitances,
    /// the conversion gain and the follower gain.
    ChargeInjection,
}

/// Electrons taken out of a node by the charge injection stimulus.
pub const INJECT_ELECTRONS: f64 = 500.0;
/// Times (s) before and after the FD injection (10 ns from 200 ns).
pub const FD_WINDOW: (f64, f64) = (190e-9, 290e-9);
/// Times (s) before and after the photodiode injection (10 ns from 400 ns).
pub const PD_WINDOW: (f64, f64) = (390e-9, 490e-9);
/// Elementary charge (C).
pub const Q_E: f64 = 1.602_176_634e-19;

/// The extraction cut down to pixel `k` of `pc`: its devices and photodiodes alone (nets
/// unchanged), with the pixel circuit renumbered to match. For simulating one pixel of an array.
pub fn pixel_only(x: &Extraction, pc: &PixelCircuit, k: usize) -> Option<(Extraction, PixelCircuit)> {
    let p = pc.pixels.get(k)?;
    let devs: Vec<usize> = p.roles().into_iter().map(|r| r.0).collect();
    let dev_ix = |d: usize| devs.iter().position(|&e| e == d);
    let mut sub = Extraction {
        nets: x.nets.clone(),
        devices: devs.iter().map(|&d| x.devices[d].clone()).collect(),
        photodiodes: p.photodiodes.iter().map(|q| x.photodiodes[q.0].clone()).collect(),
        layers: x.layers.clone(),
        dbu_per_um: x.dbu_per_um,
        ..Extraction::default()
    };
    sub.warnings.clone_from(&x.warnings);
    let q = Pixel {
        photodiodes: p.photodiodes.iter().enumerate().map(|(i, q)| (i, q.1.and_then(dev_ix))).collect(),
        rst: p.rst.and_then(dev_ix),
        sf: p.sf.and_then(dev_ix),
        sel: p.sel.and_then(dev_ix),
        other: p.other.iter().filter_map(|&d| dev_ix(d)).collect(),
        ..p.clone()
    };
    Some((sub, PixelCircuit { pixels: vec![q], warnings: pc.warnings.clone() }))
}

/// A SPICE number with up to 6 significant digits and a scale suffix (`12.825p`, `0.42u`).
fn spice_num(v: f64, suffix: &str) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{}{suffix}", if s.is_empty() || s == "-" { "0" } else { s })
}

/// Number for a source value (no suffix): `1.8`, `1e-08`.
fn val(v: f64) -> String {
    if v != 0.0 && (v.abs() < 1e-3 || v.abs() >= 1e6) {
        let s = format!("{v:.4e}");
        match s.split_once('e') {
            Some((m, e)) => format!("{}e{e}", m.trim_end_matches('0').trim_end_matches('.')),
            None => s,
        }
    } else {
        spice_num(v, "")
    }
}

const GROUND_NETS: [&str; 5] = ["0", "gnd", "sub", "vss", "agnd"];

struct Drawer<'a> {
    s: Schematic,
    x: &'a Extraction,
}

impl Drawer<'_> {
    fn net_name(&self, i: usize) -> String {
        self.x.nets.get(i).map_or("?".to_string(), |n| n.name.split_whitespace().collect::<Vec<_>>().join("_"))
    }

    /// A label (or ground symbol) named `net` at every pin of `id` in `pins` order.
    fn connect(&mut self, id: u64, pins: &[(&str, String)]) {
        let at = self.s.instance(id).expect("instance").pin_positions();
        for (pin, net) in pins {
            let Some(&(_, p)) = at.iter().find(|(n, _)| n == pin) else { continue };
            if GROUND_NETS.iter().any(|g| net.eq_ignore_ascii_case(g)) {
                let g = self.s.add_instance(DeviceKind::Ground, p, Orient::default());
                let name = self.s.next_name(DeviceKind::Ground);
                self.s.instance_mut(g).expect("added").name = name;
            } else {
                self.s.add_label(net.clone(), p);
            }
        }
    }

    fn mos(&mut self, d: usize, at: (i32, i32), orient: Orient, role: Option<&str>) -> u64 {
        let m = &self.x.devices[d];
        let u = self.x.dbu_per_um.max(1) as f64;
        let kind = if m.kind == crate::extract::DevKind::Pmos { DeviceKind::Pmos } else { DeviceKind::Nmos };
        let id = self.s.add_instance(kind, Pt::new(at.0, at.1), orient);
        let um = |v: f64| spice_num(v / u, "u");
        let um2 = |v: f64| spice_num(v / (u * u), "p");
        let params = [
            ("model", m.model.clone()),
            ("w", um(m.w as f64)),
            ("l", um(m.l as f64)),
            ("as", um2(m.js.area)),
            ("ps", um(m.js.perimeter)),
            ("ad", um2(m.jd.area)),
            ("pd", um(m.jd.perimeter)),
        ];
        let name = match role {
            Some(r) if r != "other" => format!("{}_{r}", m.name),
            _ => m.name.clone(),
        };
        let inst = self.s.instance_mut(id).expect("added");
        inst.name = name;
        for (k, v) in params {
            inst.params.insert(k.into(), v);
        }
        let pins = [("d", self.net_name(m.d)), ("g", self.net_name(m.g)), ("s", self.net_name(m.s)), ("b", self.net_name(m.b))];
        self.connect(id, &pins);
        id
    }

    fn two_pin(&mut self, kind: DeviceKind, name: &str, at: (i32, i32), p: String, n: String, kv: &[(&str, String)]) -> u64 {
        let id = self.s.add_instance(kind, Pt::new(at.0, at.1), Orient::default());
        let inst = self.s.instance_mut(id).expect("added");
        inst.name = name.to_string();
        for (k, v) in kv {
            inst.params.insert((*k).into(), v.clone());
        }
        self.connect(id, &[("p", p), ("n", n)]);
        id
    }
}

const R90: Orient = Orient { rot: 1, mirror: false };
/// Horizontal pitch of pixels in the drawing (grid points).
const PIXEL_PITCH: i32 = 44;
/// Vertical pitch of the extra photodiodes of a shared pixel.
const SHARED_PITCH: i32 = 8;

/// A schematic of the extraction: pixels in the textbook arrangement, other devices in rows
/// below, and with `bench` a test bench and its simulation setup (reset and transfer pulses,
/// transient analysis).
pub fn schematic(x: &Extraction, pc: &PixelCircuit, bench: Option<&Testbench>) -> (Schematic, SimSetup) {
    let mut dr = Drawer { s: Schematic::default(), x };
    let mut placed: BTreeSet<usize> = BTreeSet::new();
    let mut rows: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    let mut max_y = 28;
    for (k, p) in pc.pixels.iter().enumerate() {
        let ox = k as i32 * PIXEL_PITCH;
        for (j, &(pd, tg)) in p.photodiodes.iter().enumerate() {
            let oy = j as i32 * SHARED_PITCH;
            let d = &x.photodiodes[pd];
            if let Some(t) = tg {
                dr.mos(t, (ox + 8, 12 + oy), R90, Some("TG"));
                placed.insert(t);
                rows.entry("tx").or_default().insert(dr.net_name(x.devices[t].g));
            }
            // The photodiode as its photocurrent from the collecting node into its well.
            let (node, well) = (dr.net_name(d.node()), dr.net_name(d.well()));
            let (pn, nn) = if d.n_type { (node, well) } else { (well, node) };
            let iph = bench.map_or("0".to_string(), |b| val(b.iph));
            dr.two_pin(DeviceKind::ISource, &format!("IPH_{}", d.name), (ox + 1, 18 + oy), pn, nn, &[("dc", iph)]);
            max_y = max_y.max(28 + oy);
        }
        let mut put = |dev: Option<usize>, at: (i32, i32), role: &'static str, row: Option<&'static str>| {
            if let Some(d) = dev {
                dr.mos(d, (ox + at.0, at.1), Orient::default(), Some(role));
                placed.insert(d);
                if let Some(r) = row {
                    let g = dr.net_name(x.devices[d].g);
                    rows.entry(r).or_default().insert(g);
                }
            }
        };
        put(p.rst, (16, 4), "RST", Some("rst"));
        put(p.sf, (24, 12), "SF", None);
        put(p.sel, (24, 20), "SEL", Some("sel"));
        for (i, &d) in p.other.iter().enumerate() {
            put(Some(d), (34, 4 + 8 * i as i32), "other", None);
            max_y = max_y.max(12 + 8 * i as i32);
        }
        if let Some(s) = p.supply {
            rows.entry("supply").or_default().insert(dr.net_name(s));
        }
        if let Some(c) = p.column {
            rows.entry("column").or_default().insert(dr.net_name(c));
        }
    }
    // Everything else in rows: MOS, passives, bipolar transistors.
    let row_y = max_y + 8;
    for (col, d) in (0..x.devices.len()).filter(|d| !placed.contains(d)).enumerate() {
        dr.mos(d, (col as i32 * 10, row_y), Orient::default(), None);
    }
    for (i, p) in x.passives.iter().enumerate() {
        let (kind, key) = match p.kind {
            PassiveKind::Resistor => (DeviceKind::Resistor, "r"),
            PassiveKind::Capacitor => (DeviceKind::Capacitor, "c"),
        };
        dr.two_pin(kind, &p.name, (i as i32 * 8, row_y + 10), dr.net_name(p.a), dr.net_name(p.b), &[(key, p.spice_value())]);
    }
    for (i, q) in x.bjts.iter().enumerate() {
        let kind = if q.pnp { DeviceKind::Pnp } else { DeviceKind::Npn };
        let id = dr.s.add_instance(kind, Pt::new(i as i32 * 8, row_y + 20), Orient::default());
        let inst = dr.s.instance_mut(id).expect("added");
        inst.name = q.name.clone();
        inst.params.insert("model".into(), q.model.clone());
        inst.params.insert("area".into(), format!("{}", q.area));
        let pins = [("c", dr.net_name(q.c)), ("b", dr.net_name(q.b)), ("e", dr.net_name(q.e))];
        dr.connect(id, &pins);
    }

    let mut sim = SimSetup::default();
    if let Some(b) = bench {
        // Reset photodiode and FD (TX and RST high), integrate, release the reset (reset level
        // on the column), then transfer (signal level). Only the first TX row transfers; the
        // other photodiodes of a shared pixel stay isolated.
        let v = val(b.vdd);
        let inject = b.stimulus == Stimulus::ChargeInjection;
        let (rst_pulse, tx_pulse) = if inject {
            (format!("PULSE({v} 0 100n 1n 1n 1 2)"), format!("PULSE({v} 0 50n 1n 1n 1 2)"))
        } else {
            (format!("PULSE({v} 0 300n 1n 1n 400n 800n)"), format!("PULSE({v} 0 50n 1n 1n 350n 800n)"))
        };
        let first_tx: Option<String> =
            pc.pixels.iter().find_map(|p| p.photodiodes.iter().find_map(|q| q.1)).map(|t| dr.net_name(x.devices[t].g));
        let mut sources: Vec<(String, Vec<(&'static str, String)>)> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut add = |net: &String, kv: Vec<(&'static str, String)>| {
            if seen.insert(net.clone()) && !GROUND_NETS.iter().any(|g| net.eq_ignore_ascii_case(g)) {
                sources.push((net.clone(), kv));
            }
        };
        for n in rows.get("supply").into_iter().flatten() {
            add(n, vec![("dc", v.clone())]);
        }
        for n in rows.get("rst").into_iter().flatten() {
            add(n, vec![("dc", v.clone()), ("tran", rst_pulse.clone())]);
        }
        for n in rows.get("tx").into_iter().flatten() {
            if inject || Some(n) == first_tx.as_ref() {
                add(n, vec![("dc", v.clone()), ("tran", tx_pulse.clone())]);
            } else {
                add(n, vec![("dc", "0".into())]);
            }
        }
        for n in rows.get("sel").into_iter().flatten() {
            add(n, vec![("dc", v.clone())]);
        }
        for (i, (net, kv)) in sources.iter().enumerate() {
            let name = format!("V{}", net.to_ascii_uppercase());
            dr.two_pin(DeviceKind::VSource, &name, (-10 * (i as i32 + 1), 6), net.clone(), "0".into(), kv);
        }
        for (i, net) in rows.get("column").into_iter().flatten().enumerate() {
            let name = format!("ICOL_{}", net.to_ascii_uppercase());
            dr.two_pin(DeviceKind::ISource, &name, (-10 * (i as i32 + 1), 18), net.clone(), "0".into(), &[("dc", val(b.icol))]);
        }
        let mut extra = Vec::new();
        let mut nodes = BTreeSet::new();
        for p in &pc.pixels {
            nodes.insert(p.fd);
            nodes.extend(p.photodiodes.iter().map(|q| x.photodiodes[q.0].node()));
        }
        for n in nodes {
            if let Some(c) = wire_cap(x, &b.wire_caps, n) {
                let name = dr.net_name(n);
                extra.push(format!("CW_{} {name} 0 {}", name.to_ascii_uppercase(), val(c)));
            }
        }
        if !extra.is_empty() {
            extra.insert(0, "* wire capacitance from the layout".into());
        }
        if inject && let Some(p) = pc.pixels.first() {
            // Electrons out of FD, then out of the first photodiode: 10 ns current pulses.
            let amps = val(INJECT_ELECTRONS * Q_E / 10e-9);
            extra.push(format!("* charge injection: {INJECT_ELECTRONS} electrons each"));
            let mut targets = vec![("FD", p.fd, "200n")];
            targets.extend(p.photodiodes.first().map(|q| ("PD", x.photodiodes[q.0].node(), "400n")));
            for (tag, n, at) in targets {
                extra.push(format!("IQ_{tag} {} 0 DC 0 PULSE(0 {amps} {at} 0.1n 0.1n 10n 1)", dr.net_name(n)));
            }
            for inst in dr.s.instances.iter_mut().filter(|i| i.name.starts_with("IPH_")) {
                inst.params.insert("dc".into(), "0".into());
            }
        }
        sim.extra = extra.join("\n");
        sim.set_enabled("op", false);
        sim.set_enabled("tran", true);
        if let Some(Analysis::Tran { tstep, tstop, .. }) = sim.get_mut("tran") {
            *tstep = if inject { "0.5n" } else { "1n" }.into();
            *tstop = if inject { "500n" } else { "600n" }.into();
        }
    }
    (dr.s, sim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::CellRef;

    fn aps4t() -> (crate::project::Project, Extraction) {
        let p = crate::examples::demo_project();
        let lay = p.cell(&CellRef::new("work", "aps4t")).unwrap().layout.as_ref().unwrap();
        let x = crate::extract::extract(lay, &p, &p.tech);
        (p, x)
    }

    #[test]
    fn finds_the_4t_pixel() {
        let (_, x) = aps4t();
        assert_eq!(x.photodiodes.len(), 1, "{:?}", x.warnings);
        let d = &x.photodiodes[0];
        let net = |i: usize| x.nets[i].name.as_str();
        assert!(d.n_type);
        assert_eq!((net(d.anode), net(d.cathode)), ("sub", "pd"));
        // The drawn photodiode, 4.75 x 2.7 µm.
        assert_eq!((d.area, d.perimeter), (4750 * 2700, 2 * (4750 + 2700)));
        let pc = classify(&x);
        assert!(pc.warnings.is_empty(), "{:?}", pc.warnings);
        assert_eq!(pc.pixels.len(), 1);
        let p = &pc.pixels[0];
        let dev = |d: Option<usize>| d.map(|d| (x.devices[d].name.as_str(), net(x.devices[d].g)));
        assert_eq!(dev(p.photodiodes[0].1), Some(("M1", "tx")));
        assert_eq!(dev(p.rst), Some(("M4", "rst")));
        assert_eq!(dev(p.sf), Some(("M3", "fd")));
        assert_eq!(dev(p.sel), Some(("M2", "sel")));
        assert_eq!((net(p.fd), p.supply.map(net), p.column.map(net)), ("fd", Some("vdd"), Some("col")));
        assert!(p.other.is_empty());
        let rep = pc.report(&x, &[NetCap { net: "fd".into(), farads: 1.5e-15 }]);
        assert!(rep.contains("wire capacitance of fd: 1.500 fF"), "{rep}");
        assert!(rep.contains("TG    M1") && rep.contains("area=12.8250 um2"), "{rep}");
        assert!(x.spice("t", 1000).contains("* photodiode D1 sub pd area=12.825p pj=14.9u\n"));
    }

    #[test]
    fn schematic_simulates_and_matches_the_layout() {
        let (mut p, x) = aps4t();
        let pc = classify(&x);
        let caps = crate::pex::net_caps(&x, &p.tech);
        let (sch, sim) = schematic(&x, &pc, Some(&Testbench { wire_caps: caps, ..Testbench::default() }));
        assert!(sim.extra.contains("CW_FD fd 0 "), "{}", sim.extra);
        let nl = crate::netlist::netlist(&sch);
        assert!(nl.warnings.is_empty(), "{:?}\n{}", nl.warnings, nl.body);
        for l in ["M1_TG fd tx pd 0 nmos w=0.42u l=0.4u", "M4_RST fd rst vdd 0", "IPH_D1 pd 0 DC 1e-8", "ICOL_COL col 0 DC 2e-6"]
        {
            assert!(nl.body.contains(l), "missing '{l}' in\n{}", nl.body);
        }
        assert!(nl.body.contains("VTX tx 0 DC 1.8 PULSE(1.8 0 50n"), "{}", nl.body);
        assert!(sim.enabled().any(|a| a.kind_name() == "tran"));
        // LVS of the layout against the generated schematic is clean.
        let r = CellRef::new("work", "aps4t");
        p.cell_mut(&r).unwrap().schematic = Some(sch);
        let circuit = p.circuit(&r).unwrap();
        let rep = crate::lvs::compare(&circuit, &x, &crate::lvs::LvsOptions::for_tech(&p.tech));
        assert!(rep.matched, "{:?}", rep.errors);
    }

    #[test]
    fn shared_and_3t_pixels() {
        use crate::extract::{DevKind, Device, Net, Photodiode};
        let net = |n: &str| Net { name: n.into(), labelled: true, shapes: vec![], bbox: None };
        let names = ["pd1", "pd2", "fd", "vdd", "x", "col", "tx1", "tx2", "rst", "sel", "sub"];
        let nets: Vec<Net> = names.iter().map(|n| net(n)).collect();
        let id = |n: &str| names.iter().position(|m| *m == n).unwrap();
        let mos = |name: &str, d: &str, g: &str, s: &str| Device {
            kind: DevKind::Nmos,
            model: "nmos".into(),
            name: name.into(),
            w: 400,
            l: 200,
            d: id(d),
            g: id(g),
            s: id(s),
            b: id("sub"),
            gate: Default::default(),
            jd: Default::default(),
            js: Default::default(),
            od: None,
        };
        let pd = |name: &str, n: &str, y: i64| Photodiode {
            name: name.into(),
            n_type: true,
            anode: id("sub"),
            cathode: id(n),
            area: 1,
            perimeter: 1,
            body: crate::geom::IRect::new(0, y, 1, y + 1),
        };
        // Two photodiodes sharing fd; the follower drain on vdd and the reset drain on vdd.
        let x = Extraction {
            nets,
            devices: vec![
                mos("M1", "fd", "tx1", "pd1"),
                mos("M2", "pd2", "tx2", "fd"),
                mos("M3", "vdd", "rst", "fd"),
                mos("M4", "vdd", "fd", "x"),
                mos("M5", "x", "sel", "col"),
            ],
            photodiodes: vec![pd("D1", "pd1", 0), pd("D2", "pd2", 10)],
            dbu_per_um: 1000,
            ..Default::default()
        };
        let pc = classify(&x);
        assert!(pc.warnings.is_empty(), "{:?}", pc.warnings);
        assert_eq!(pc.pixels.len(), 1);
        let p = &pc.pixels[0];
        assert_eq!(p.photodiodes, vec![(0, Some(0)), (1, Some(1))]);
        assert_eq!((p.rst, p.sf, p.sel, p.supply, p.column), (Some(2), Some(3), Some(4), Some(id("vdd")), Some(id("col"))));
        let (sch, _) = schematic(&x, &pc, Some(&Testbench::default()));
        let nl = crate::netlist::netlist(&sch);
        assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
        assert!(nl.body.contains("VTX2 tx2 0 DC 0"), "{}", nl.body);

        // 3T: the photodiode is the follower's gate and the reset's source.
        let mut x3 = x.clone();
        x3.devices = vec![mos("M1", "vdd", "rst", "pd1"), mos("M2", "vdd", "pd1", "x"), mos("M3", "x", "sel", "col")];
        x3.photodiodes.truncate(1);
        let pc = classify(&x3);
        let p = &pc.pixels[0];
        assert_eq!((p.photodiodes.clone(), p.fd, p.rst, p.sf, p.sel), (vec![(0, None)], id("pd1"), Some(0), Some(1), Some(2)));
    }
}
