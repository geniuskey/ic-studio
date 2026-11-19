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
//! - **DCG** (dual conversion gain): when no device runs from FD to the supply side of SF
//!   directly but one does through another device, that one is RST and the device between it
//!   and FD the DCG switch.
//!
//! [`schematic`] draws the extraction: each pixel in the textbook arrangement (TG, RST, SF and
//! SEL with net labels on their pins, the photodiode as its photocurrent), the other devices in
//! rows below, and optionally a test bench (supply, reset / transfer pulses, row select, column
//! current load) with a transient analysis. The photodiode junction is the TG's source / drain
//! junction (`as` / `ps`), as extracted.

use std::collections::{BTreeMap, BTreeSet};

use crate::extract::{Extraction, Junction, PassiveKind, Photodiode};
use crate::geom::{Orient, Pt};
use crate::pex::NetCap;
use crate::schematic::{DeviceKind, Schematic};
use crate::sim_setup::{Analysis, SimSetup};
use crate::tech::PhotodiodeDef;

/// One pixel's transistors (device indices into [`Extraction::devices`]) and nets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pixel {
    /// Photodiodes (indices into [`Extraction::photodiodes`]) with their transfer gates; `None`
    /// for a 3T pixel.
    pub photodiodes: Vec<(usize, Option<usize>)>,
    /// The floating diffusion (sense node) net.
    pub fd: usize,
    pub rst: Option<usize>,
    /// Dual conversion gain switch between FD and the reset transistor.
    pub dcg: Option<usize>,
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
    /// Every device of the pixel with its role ("TG", "RST", "DCG", "SF", "SEL", "other").
    pub fn roles(&self) -> Vec<(usize, &'static str)> {
        let mut v: Vec<(usize, &'static str)> = self.photodiodes.iter().filter_map(|p| p.1).map(|d| (d, "TG")).collect();
        v.extend(self.rst.map(|d| (d, "RST")));
        v.extend(self.dcg.map(|d| (d, "DCG")));
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
            let nodes = std::iter::once(p.fd).chain(p.photodiodes.iter().map(|q| x.photodiodes[q.0].node())).chain(p.column);
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
        let on_sf = |o: usize| sides.is_some_and(|(a, b)| o == a || o == b);
        let direct = on_fd.iter().find(|(_, o)| on_sf(*o)).copied();
        // Dual conversion gain: FD - DCG - node - RST - supply.
        let chain = || {
            on_fd.iter().find_map(|&(c, node)| {
                (0..x.devices.len())
                    .filter(|e| *e != c && !taken.contains(e) && x.devices[*e].g != node)
                    .find_map(|e| other_side(x, e, node).filter(|o| on_sf(*o)).map(|o| (c, e, o)))
            })
        };
        let (rst, dcg) = match (direct, direct.is_none().then(chain).flatten()) {
            (Some(r), _) => (Some(r), None),
            (None, Some((c, e, o))) => (Some((e, o)), Some(c)),
            (None, None) => {
                let r = on_fd.iter().find(|(_, o)| looks_like_supply(&x.nets[*o].name)).or(on_fd.first()).copied();
                (r, None)
            }
        };
        let supply = match (rst, sides) {
            (Some((_, o)), Some((a, b))) if o == a || o == b => Some(o),
            (_, Some((a, b))) if looks_like_supply(&x.nets[b].name) && !looks_like_supply(&x.nets[a].name) => Some(b),
            (_, Some((a, _))) => Some(a),
            (Some((_, o)), None) => Some(o),
            (None, None) => None,
        };
        let sf_out = sides.map(|(a, b)| if Some(a) == supply { b } else { a });
        p.rst = rst.map(|r| r.0);
        p.dcg = dcg;
        p.sf = sf;
        p.supply = supply;
        p.sf_out = sf_out;
        for &(d, _) in &on_fd {
            if Some(d) != p.rst && Some(d) != p.dcg {
                p.other.push(d);
            }
        }
        taken.extend(p.rst);
        taken.extend(p.dcg);
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
    /// The technology's photodiode: a pinned one ([`PhotodiodeDef::pinning`]) is drawn as its
    /// storage node (see [`schematic`]); with dark current densities each photodiode gets its
    /// dark current at `temp` as a source `IDK_<name>`.
    pub photodiode: Option<PhotodiodeDef>,
    /// Simulation temperature (°C).
    pub temp: f64,
    /// Column line capacitance (F) of the noise bench: it sets the readout bandwidth.
    pub ccol: f64,
    /// Time (s) between the reset and the signal sample of correlated double sampling.
    pub cds_dt: f64,
}

impl Default for Testbench {
    fn default() -> Self {
        Testbench {
            vdd: 1.8,
            iph: 10e-9,
            icol: 2e-6,
            wire_caps: Vec::new(),
            stimulus: Stimulus::ResetTransfer,
            photodiode: None,
            temp: 27.0,
            ccol: 1e-12,
            cds_dt: 2e-6,
        }
    }
}

impl Testbench {
    /// The default bench with the wire capacitance of `x` and the photodiode of `tech`.
    pub fn for_tech(x: &Extraction, tech: &crate::Tech) -> Testbench {
        Testbench {
            wire_caps: crate::pex::net_caps(x, tech),
            photodiode: tech.cross_section.as_ref().and_then(|c| c.photodiode.clone()),
            ..Testbench::default()
        }
    }

    /// Pinning voltage when the photodiode is pinned.
    pub fn pinning(&self) -> Option<f64> {
        self.photodiode.as_ref().and_then(|d| d.pinning)
    }

    /// Dark current (A) of photodiode `d` of `x` at the bench temperature.
    pub fn dark_current(&self, x: &Extraction, d: &Photodiode) -> Option<f64> {
        let u = x.dbu_per_um.max(1) as f64;
        self.photodiode.as_ref()?.dark_current(d.area as f64 / (u * u), d.perimeter as f64 / u, self.temp)
    }
}

/// Conductance (S) of the pinned photodiode's charge path (see [`schematic`]).
const PIN_G: f64 = 1e-3;

/// Net holding the photo charge of photodiode `d` in a drawing with `bench`: the storage node
/// `<node>_q` of a pinned (n-type) photodiode, else the photodiode node.
pub fn charge_net(x: &Extraction, d: &Photodiode, bench: Option<&Testbench>) -> String {
    let node = x.nets.get(d.node()).map_or("?".to_string(), |n| n.name.split_whitespace().collect::<Vec<_>>().join("_"));
    if d.n_type && bench.and_then(Testbench::pinning).is_some() { format!("{node}_q") } else { node }
}

/// What the test bench drives.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Stimulus {
    /// Reset photodiode and FD, integrate the photocurrent, release the reset (reset level on
    /// the column), then transfer (signal level).
    #[default]
    ResetTransfer,
    /// Characterisation in the dark: reset FD and every photodiode, turn RST and TX off, then
    /// take [`INJECT_ELECTRONS`] out of the first pixel's FD (in [`FD_WINDOW`]) and of its
    /// photodiode number `pd` (in [`PD_WINDOW`]). The voltage steps give the node
    /// capacitances, the conversion gain and the follower gain.
    /// Then every TX transfers, on from 450 ns to 550 ns ([`TRANSFER_WINDOW`]); the charge left
    /// in the photodiode (lag) follows against a run without the photodiode injection
    /// (`pd: None`), which has the same TX feedthrough and channel charge.
    ChargeInjection { pd: Option<usize> },
    /// Full well: FD held in reset, the photodiode reset and then filled by the photocurrent
    /// ([`Testbench::iph`]) until it saturates (blooming over TX or into the substrate);
    /// sample at [`FULL_WELL_AT`].
    FullWell,
    /// Readout noise: RST and TX off, SEL on, the first pixel's FD and photodiode charge nodes
    /// (every photodiode at `charge`) held at these voltages through a noiseless 1 nS transconductance (a resistor would add
    /// its own noise), the column line loaded by [`Testbench::ccol`]; a noise analysis of the
    /// column voltage over [`NOISE_BAND`] (input source `VFDB`).
    NoiseBias { fd: f64, charge: f64 },
}

/// Frequency band (Hz) of the readout noise analysis. One sample's noise is integrated from
/// [`SAMPLE_NOISE_FROM`]; correlated double sampling filters the whole band itself.
pub const NOISE_BAND: (f64, f64) = (1.0, 1e9);
/// Lower end (Hz) of one sample's noise, the reciprocal of a frame-scale time.
pub const SAMPLE_NOISE_FROM: f64 = 1e3;

/// Electrons taken out of a node by the charge injection stimulus.
pub const INJECT_ELECTRONS: f64 = 500.0;
/// Times (s) before and after the FD injection (10 ns from 200 ns).
pub const FD_WINDOW: (f64, f64) = (190e-9, 290e-9);
/// Times (s) before and after the photodiode injection (10 ns from 400 ns).
pub const PD_WINDOW: (f64, f64) = (390e-9, 440e-9);
/// Times (s) before and after the transfer (TX on 450 ns to 550 ns).
pub const TRANSFER_WINDOW: (f64, f64) = (440e-9, 590e-9);
/// Times (s) at which a full-well run is sampled (the second confirms saturation).
pub const FULL_WELL_AT: (f64, f64) = (1.8e-6, 1.95e-6);
/// Elementary charge (C).
pub const Q_E: f64 = 1.602_176_634e-19;

/// The extraction cut down to pixel `k` of `pc`: its devices and photodiodes, and the other
/// devices on its column line (the select transistors of the other pixels of an array, their
/// gates left off), with nets unchanged and the pixel circuit renumbered to match. For
/// simulating one pixel of an array with its column load.
pub fn pixel_only(x: &Extraction, pc: &PixelCircuit, k: usize) -> Option<(Extraction, PixelCircuit)> {
    let p = pc.pixels.get(k)?;
    let mut devs: Vec<usize> = p.roles().into_iter().map(|r| r.0).collect();
    if let Some(col) = p.column.filter(|c| Some(*c) != p.sf_out) {
        let on_col = (0..x.devices.len()).filter(|d| !devs.contains(d) && other_side(x, *d, col).is_some());
        devs.extend(on_col.collect::<Vec<_>>());
    }
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
        dcg: p.dcg.and_then(dev_ix),
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

/// The grid points of the axis-parallel segment from `a` to `b`.
fn raster(a: Pt, b: Pt) -> impl Iterator<Item = (i32, i32)> {
    let n = (b.x - a.x).abs().max((b.y - a.y).abs());
    let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
    (0..=n).map(move |k| (a.x + k * dx, a.y + k * dy))
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
    /// Labelled pins per net, for wiring nets inside a pixel.
    pins: BTreeMap<String, Vec<Pt>>,
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
                self.pins.entry(net.clone()).or_default().push(p);
            }
        }
    }

    /// Wire the pins of `net` where a route is clear: each pin (left to right) joins the
    /// nearest pin already wired by an L of two segments that touches no other pin, label or
    /// wire. Wired pins keep one label naming the net; a pin no route reaches keeps its label.
    fn wire(&mut self, net: &str) {
        let Some(mut pts) = self.pins.get(net).cloned() else { return };
        if pts.len() < 2 {
            return;
        }
        pts.sort_by_key(|p| (p.x, p.y));
        let own: BTreeSet<(i32, i32)> = pts.iter().map(|p| (p.x, p.y)).collect();
        let mut busy: BTreeSet<(i32, i32)> = BTreeSet::new();
        for i in &self.s.instances {
            busy.extend(i.pin_positions().into_iter().map(|(_, p)| (p.x, p.y)));
            busy.insert((i.pos.x, i.pos.y));
        }
        busy.extend(self.s.labels.iter().map(|l| (l.pos.x, l.pos.y)));
        for w in &self.s.wires {
            busy.extend(raster(w.a, w.b));
        }
        busy.retain(|p| !own.contains(p));
        let mut wired = vec![pts[0]];
        let mut stranded = Vec::new();
        for &b in &pts[1..] {
            let mut from: Vec<Pt> = wired.clone();
            from.sort_by_key(|a| (a.x - b.x).abs() + (a.y - b.y).abs());
            let route = from.iter().find_map(|&a| {
                [Pt::new(b.x, a.y), Pt::new(a.x, b.y)]
                    .into_iter()
                    .find(|&c| raster(a, c).chain(raster(c, b)).all(|q| !busy.contains(&q)))
                    .map(|c| (a, c))
            });
            match route {
                Some((a, c)) => {
                    for (p, q) in [(a, c), (c, b)] {
                        if p != q {
                            self.s.add_wire(p, q);
                        }
                    }
                    wired.push(b);
                }
                None => stranded.push(b),
            }
        }
        let first = pts[0];
        self.s.labels.retain(|l| l.name != net || l.pos == first || stranded.contains(&l.pos));
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
    let mut dr = Drawer { s: Schematic::default(), x, pins: BTreeMap::new() };
    let mut placed: BTreeSet<usize> = BTreeSet::new();
    let mut rows: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    let mut max_y = 28;
    for (k, p) in pc.pixels.iter().enumerate() {
        let ox = k as i32 * PIXEL_PITCH;
        for (j, &(pd, tg)) in p.photodiodes.iter().enumerate() {
            let oy = j as i32 * SHARED_PITCH;
            let d = &x.photodiodes[pd];
            let pinned = d.n_type && bench.and_then(Testbench::pinning).is_some();
            if let Some(t) = tg {
                let id = dr.mos(t, (ox + 8, 12 + oy), R90, Some("TG"));
                placed.insert(t);
                rows.entry("tx").or_default().insert(dr.net_name(x.devices[t].g));
                if pinned {
                    // The storage capacitance is the photodiode's own: the TG junction keeps
                    // only the diffusion outside the photodiode region.
                    let m = &x.devices[t];
                    let u = x.dbu_per_um.max(1) as f64;
                    let (ka, kp, j) = if m.s == d.node() { ("as", "ps", m.js) } else { ("ad", "pd", m.jd) };
                    let area = (j.area - d.area as f64).max(0.0) / (u * u);
                    let perim = (j.perimeter - d.perimeter as f64).max(0.0) / u;
                    let inst = dr.s.instance_mut(id).expect("TG");
                    inst.params.insert(ka.into(), spice_num(area, "p"));
                    inst.params.insert(kp.into(), spice_num(perim, "u"));
                }
            }
            // The photodiode as its photocurrent from the charge node into its well.
            let (node, well) = (charge_net(x, d, bench), dr.net_name(d.well()));
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
        put(p.dcg, (34, 4), "DCG", Some("dcg"));
        put(p.sf, (24, 12), "SF", None);
        put(p.sel, (24, 20), "SEL", Some("sel"));
        let first_other = if p.dcg.is_some() { 1 } else { 0 };
        for (i, &d) in p.other.iter().enumerate() {
            let row = (first_other + i) as i32;
            put(Some(d), (34, 4 + 8 * row), "other", None);
            max_y = max_y.max(12 + 8 * row);
        }
        if let Some(s) = p.supply {
            rows.entry("supply").or_default().insert(dr.net_name(s));
        }
        if let Some(c) = p.column {
            rows.entry("column").or_default().insert(dr.net_name(c));
        }
    }
    // Wires for the nets inside each pixel: FD, the follower output, the photodiode nodes.
    for p in &pc.pixels {
        let mut nets = vec![dr.net_name(p.fd)];
        nets.extend(p.sf_out.filter(|o| Some(*o) != p.column).map(|o| dr.net_name(o)));
        nets.extend(p.photodiodes.iter().map(|q| charge_net(x, &x.photodiodes[q.0], bench)));
        for n in nets {
            dr.wire(&n);
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
        let st = b.stimulus;
        let (rst_pulse, tx_pulse) = match st {
            Stimulus::ResetTransfer => {
                (format!("PULSE({v} 0 300n 1n 1n 400n 800n)"), format!("PULSE({v} 0 50n 1n 1n 350n 800n)"))
            }
            // Reset off for good at 100 ns; TX off at 50 ns and on again 450 to 550 ns.
            Stimulus::ChargeInjection { .. } => {
                (format!("PULSE({v} 0 100n 1n 1n 1 2)"), format!("PULSE({v} 0 50n 1n 1n 399n 500n)"))
            }
            Stimulus::FullWell => (String::new(), format!("PULSE({v} 0 50n 1n 1n 1 2)")),
            Stimulus::NoiseBias { .. } => ("off".to_string(), "off".to_string()),
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
        let pulsed = |p: &String| -> Vec<(&'static str, String)> {
            match p.as_str() {
                "" => vec![("dc", v.clone())],
                "off" => vec![("dc", "0".into())],
                _ => vec![("dc", v.clone()), ("tran", p.clone())],
            }
        };
        for n in rows.get("supply").into_iter().flatten() {
            add(n, vec![("dc", v.clone())]);
        }
        for n in rows.get("rst").into_iter().flatten() {
            add(n, pulsed(&rst_pulse));
        }
        for n in rows.get("tx").into_iter().flatten() {
            if st != Stimulus::ResetTransfer || Some(n) == first_tx.as_ref() {
                add(n, pulsed(&tx_pulse));
            } else {
                add(n, vec![("dc", "0".into())]);
            }
        }
        for n in rows.get("sel").into_iter().flatten() {
            add(n, vec![("dc", v.clone())]);
        }
        // Dual conversion gain switch off: the high gain.
        for n in rows.get("dcg").into_iter().flatten() {
            add(n, vec![("dc", "0".into())]);
        }
        for (i, (net, kv)) in sources.iter().enumerate() {
            let name = format!("V{}", net.to_ascii_uppercase());
            dr.two_pin(DeviceKind::VSource, &name, (-10 * (i as i32 + 1), 6), net.clone(), "0".into(), kv);
        }
        for (i, net) in rows.get("column").into_iter().flatten().enumerate() {
            let name = format!("ICOL_{}", net.to_ascii_uppercase());
            dr.two_pin(DeviceKind::ISource, &name, (-10 * (i as i32 + 1), 18), net.clone(), "0".into(), &[("dc", val(b.icol))]);
        }
        let spice = |n: String| if GROUND_NETS.iter().any(|g| n.eq_ignore_ascii_case(g)) { "0".to_string() } else { n };
        let mut extra = Vec::new();
        let mut nodes = BTreeSet::new();
        for p in &pc.pixels {
            nodes.insert(p.fd);
            nodes.extend(p.column);
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
        if let (Some(def), Some(vpin)) = (&b.photodiode, b.pinning()) {
            // A pinned photodiode: its charge sits on the storage node <pd>_q (the storage
            // capacitance), which follows the photodiode node through a conductance but never
            // rises above the pinning voltage: emptied, it holds no charge whatever TX and FD
            // do. Electrons left on the photodiode side when TX turns off flow back into it.
            let u = x.dbu_per_um.max(1) as f64;
            let mut done = BTreeSet::new();
            for p in &pc.pixels {
                for &(k, _) in &p.photodiodes {
                    let d = &x.photodiodes[k];
                    if !d.n_type || !done.insert(k) {
                        continue;
                    }
                    let (node, q) = (dr.net_name(d.node()), charge_net(x, d, Some(b)));
                    let c = def.storage_cap(d.area as f64 / (u * u), d.perimeter as f64 / u);
                    extra.push(format!("* pinned photodiode {}: pinning {vpin} V, storage node {q}", d.name));
                    extra.push(format!("CPD_{} {q} {} {}", d.name, spice(dr.net_name(d.well())), val(c)));
                    extra.push(format!("BPIN_{} {node} {q} I={}*(min(V({node}),{vpin})-V({q}))", d.name, val(PIN_G)));
                }
            }
        }
        // Dark current of every photodiode, from its charge node like the photocurrent.
        let mut dark = BTreeSet::new();
        for p in &pc.pixels {
            for &(k, _) in &p.photodiodes {
                let d = &x.photodiodes[k];
                if let Some(i) = b.dark_current(x, d).filter(|_| dark.insert(k)) {
                    let (q, well) = (charge_net(x, d, Some(b)), spice(dr.net_name(d.well())));
                    let (pn, nn) = if d.n_type { (q, well) } else { (well, q) };
                    extra.push(format!("* dark current of {} at {} C", d.name, b.temp));
                    extra.push(format!("IDK_{} {pn} {nn} DC {}", d.name, val(i)));
                }
            }
        }
        if let Stimulus::ChargeInjection { pd } = st
            && let Some(p) = pc.pixels.first()
        {
            // Electrons out of FD, then out of photodiode `pd`: 10 ns current pulses.
            let amps = val(INJECT_ELECTRONS * Q_E / 10e-9);
            extra.push(format!("* charge injection: {INJECT_ELECTRONS} electrons each"));
            let mut targets = vec![("FD", dr.net_name(p.fd), "200n")];
            targets.extend(
                pd.and_then(|j| p.photodiodes.get(j)).map(|q| ("PD", charge_net(x, &x.photodiodes[q.0], Some(b)), "400n")),
            );
            for (tag, n, at) in targets {
                extra.push(format!("IQ_{tag} {n} 0 DC 0 PULSE(0 {amps} {at} 0.1n 0.1n 10n 1)"));
            }
        }
        if st != Stimulus::ResetTransfer {
            // Dark, except the full-well run's photocurrent.
            let dc = if st == Stimulus::FullWell { val(b.iph) } else { "0".into() };
            for inst in dr.s.instances.iter_mut().filter(|i| i.name.starts_with("IPH_")) {
                inst.params.insert("dc".into(), dc.clone());
            }
        }
        let noise_out = match st {
            Stimulus::NoiseBias { fd, charge } => pc.pixels.first().and_then(|p| {
                extra.push("* readout noise bias: noiseless 1 nS holds FD and the photodiodes".into());
                let mut held = vec![("FD".to_string(), dr.net_name(p.fd), fd)];
                for (j, q) in p.photodiodes.iter().enumerate() {
                    held.push((format!("PD{}", j + 1), charge_net(x, &x.photodiodes[q.0], Some(b)), charge));
                }
                for (tag, n, volts) in held {
                    // An AC value: ngspice refuses a noise input source without one.
                    let ac = if tag == "FD" { " AC 1" } else { "" };
                    extra.push(format!("V{tag}B {n}_b 0 DC {}{ac}", val(volts)));
                    extra.push(format!("G{tag}B {n} 0 {n} {n}_b 1e-9"));
                }
                let col = dr.net_name(p.column.or(p.sf_out)?);
                extra.push(format!("CCOL {col} 0 {}", val(b.ccol)));
                Some(col)
            }),
            _ => None,
        };
        sim.extra = extra.join("\n");
        sim.temp = b.temp;
        sim.set_enabled("op", false);
        if let Some(col) = noise_out {
            sim.set_enabled("tran", false);
            sim.set_enabled("noise", true);
            if let Some(Analysis::Noise { output, source, points_per_decade, fstart, fstop }) = sim.get_mut("noise") {
                *output = col;
                *source = "VFDB".into();
                *points_per_decade = 20;
                *fstart = val(NOISE_BAND.0);
                *fstop = val(NOISE_BAND.1);
            }
        } else {
            sim.set_enabled("tran", true);
            let (step, stop) = match st {
                Stimulus::ResetTransfer | Stimulus::NoiseBias { .. } => ("1n", "600n"),
                Stimulus::ChargeInjection { .. } => ("0.5n", "600n"),
                Stimulus::FullWell => ("2n", "2u"),
            };
            if let Some(Analysis::Tran { tstep, tstop, .. }) = sim.get_mut("tran") {
                *tstep = step.into();
                *tstop = stop.into();
            }
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
    fn finds_the_shared_demo_pixel() {
        let mut p = crate::examples::demo_project();
        let r = CellRef::new("work", "aps2s");
        let x = crate::extract::extract(p.cell(&r).unwrap().layout.as_ref().unwrap(), &p, &p.tech);
        assert_eq!(x.photodiodes.len(), 2, "{:?}", x.warnings);
        let pc = classify(&x);
        assert!(pc.warnings.is_empty(), "{:?}", pc.warnings);
        assert_eq!(pc.pixels.len(), 1);
        let px = &pc.pixels[0];
        let net = |i: usize| x.nets[i].name.as_str();
        let tgs: Vec<(&str, &str)> =
            px.photodiodes.iter().map(|&(d, t)| (net(x.photodiodes[d].node()), net(x.devices[t.unwrap()].g))).collect();
        assert_eq!(tgs, [("pd1", "tx1"), ("pd2", "tx2")]);
        assert_eq!((net(px.fd), px.supply.map(net), px.column.map(net)), ("fd", Some("vdd"), Some("col")));
        assert!(px.rst.is_some() && px.sf.is_some() && px.sel.is_some() && px.other.is_empty());
        // The drawn schematic of the shared pixel matches the layout too.
        let (sch, _) = schematic(&x, &pc, Some(&Testbench::for_tech(&x, &p.tech)));
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

    #[test]
    fn dual_conversion_gain_pixel() {
        use crate::extract::{DevKind, Device, Net, Photodiode};
        let names = ["pd", "fd", "fdx", "vdd", "x", "col", "tx", "dcg", "rst", "sel", "sub"];
        let nets: Vec<Net> =
            names.iter().map(|n| Net { name: (*n).into(), labelled: true, shapes: vec![], bbox: None }).collect();
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
        // FD - DCG - fdx - RST - vdd: the device on FD is the switch, not the reset.
        let x = Extraction {
            nets,
            devices: vec![
                mos("M1", "fd", "tx", "pd"),
                mos("M2", "fdx", "dcg", "fd"),
                mos("M3", "vdd", "rst", "fdx"),
                mos("M4", "vdd", "fd", "x"),
                mos("M5", "x", "sel", "col"),
            ],
            photodiodes: vec![Photodiode {
                name: "D1".into(),
                n_type: true,
                anode: id("sub"),
                cathode: id("pd"),
                area: 1,
                perimeter: 1,
                body: Default::default(),
            }],
            dbu_per_um: 1000,
            ..Default::default()
        };
        let pc = classify(&x);
        assert!(pc.warnings.is_empty(), "{:?}", pc.warnings);
        let p = &pc.pixels[0];
        assert_eq!((p.rst, p.dcg, p.sf, p.sel, p.supply), (Some(2), Some(1), Some(3), Some(4), Some(id("vdd"))));
        assert!(p.other.is_empty());
        assert_eq!(pc.role(1), Some("DCG"));
        let (sch, _) = schematic(&x, &pc, Some(&Testbench::default()));
        let nl = crate::netlist::netlist(&sch);
        assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
        assert!(nl.body.contains("M2_DCG fdx dcg fd 0") && nl.body.contains("VDCG dcg 0 DC 0"), "{}", nl.body);
    }

    #[test]
    fn one_pixel_of_an_array_keeps_its_column_load() {
        let p = crate::examples::demo_project();
        let lay = p.cell(&CellRef::new("work", "pixel_array")).unwrap().layout.as_ref().unwrap();
        let x = crate::extract::extract(lay, &p, &p.tech);
        let pc = classify(&x);
        assert_eq!(pc.pixels.len(), 64);
        let (sub, one) = pixel_only(&x, &pc, 0).unwrap();
        // The pixel's four transistors and the select transistors of the column's other 7 rows.
        assert_eq!(sub.devices.len(), 4 + 7);
        let col = one.pixels[0].column.unwrap();
        assert_eq!(sub.devices.iter().filter(|m| m.s == col || m.d == col).count(), 8);
        let tb = Testbench::for_tech(&sub, &p.tech);
        let (_, sim) = schematic(&sub, &one, Some(&tb));
        let name = &x.nets[col].name;
        assert!(sim.extra.contains(&format!("CW_{} {name} 0 ", name.to_ascii_uppercase())), "{}", sim.extra);
    }
}
