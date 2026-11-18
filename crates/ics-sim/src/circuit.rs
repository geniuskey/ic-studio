//! SPICE deck parser (subset) producing a flat circuit for the built-in engine.

use std::collections::HashMap;
use std::sync::Arc;

use ics_core::Analysis;
use ics_core::units::parse_spice;

use crate::SimError;
use crate::bsim4;

#[derive(Debug, Clone, PartialEq)]
pub enum Wave {
    Pulse {
        v1: f64,
        v2: f64,
        td: f64,
        tr: f64,
        tf: f64,
        pw: f64,
        per: f64,
    },
    /// `phase` in degrees.
    Sin {
        vo: f64,
        va: f64,
        freq: f64,
        td: f64,
        theta: f64,
        phase: f64,
    },
    Pwl(Vec<(f64, f64)>),
    /// PWL with `r=`: after its last point the waveform repeats from time `r` on.
    PwlRepeat {
        pts: Vec<(f64, f64)>,
        r: f64,
    },
}

impl Wave {
    pub fn value(&self, t: f64) -> f64 {
        match self {
            Wave::Pulse { v1, v2, td, tr, tf, pw, per } => {
                if t < *td {
                    return *v1;
                }
                let mut tt = t - td;
                if *per > 0.0 {
                    tt %= per;
                }
                let tr = tr.max(1e-15);
                let tf = tf.max(1e-15);
                if tt < tr {
                    v1 + (v2 - v1) * tt / tr
                } else if tt < tr + pw {
                    *v2
                } else if tt < tr + pw + tf {
                    v2 + (v1 - v2) * (tt - tr - pw) / tf
                } else {
                    *v1
                }
            }
            Wave::Sin { vo, va, freq, td, theta, phase } => {
                let ph = phase.to_radians();
                if t < *td {
                    vo + va * ph.sin()
                } else {
                    let tt = t - td;
                    vo + va * (-tt * theta).exp() * (2.0 * std::f64::consts::PI * freq * tt + ph).sin()
                }
            }
            Wave::PwlRepeat { pts, r } => Wave::Pwl(pts.clone()).value(Self::repeat_time(pts, *r, t)),
            Wave::Pwl(pts) => {
                if pts.is_empty() {
                    return 0.0;
                }
                if t <= pts[0].0 {
                    return pts[0].1;
                }
                for w in pts.windows(2) {
                    if t <= w[1].0 {
                        let f = if w[1].0 > w[0].0 { (t - w[0].0) / (w[1].0 - w[0].0) } else { 1.0 };
                        return w[0].1 + f * (w[1].1 - w[0].1);
                    }
                }
                pts.last().unwrap().1
            }
        }
    }

    /// Times where the waveform has slope discontinuities (for time-step control).
    pub fn breakpoints(&self, tstop: f64) -> Vec<f64> {
        match self {
            Wave::Pulse { td, tr, tf, pw, per, .. } => {
                let mut v = vec![];
                let mut base = *td;
                loop {
                    for t in [base, base + tr, base + tr + pw, base + tr + pw + tf] {
                        if t <= tstop {
                            v.push(t);
                        }
                    }
                    if *per <= 0.0 || base > tstop || v.len() > 10_000 {
                        break;
                    }
                    base += per;
                }
                v
            }
            Wave::Pwl(pts) => pts.iter().map(|p| p.0).filter(|t| *t <= tstop).collect(),
            Wave::PwlRepeat { pts, r } => {
                let mut v: Vec<f64> = pts.iter().map(|p| p.0).filter(|t| *t <= tstop).collect();
                let (Some(last), true) = (pts.last().map(|p| p.0), pts.len() >= 2) else { return v };
                let period = last - r;
                if period > 0.0 {
                    let inner: Vec<f64> = pts.iter().map(|p| p.0).filter(|t| *t > *r).collect();
                    let mut base = last;
                    while base < tstop && v.len() < 10_000 {
                        v.extend(inner.iter().map(|t| base + t - r).filter(|t| *t <= tstop));
                        base += period;
                    }
                }
                v
            }
            Wave::Sin { .. } => vec![],
        }
    }

    /// The time within the PWL points that `t` maps to when it repeats from `r` after the last point.
    fn repeat_time(pts: &[(f64, f64)], r: f64, t: f64) -> f64 {
        let Some(last) = pts.last().map(|p| p.0) else { return t };
        let period = last - r;
        if t <= last || period <= 0.0 { t } else { r + (t - last) % period }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    pub dc: f64,
    pub ac_mag: f64,
    pub ac_phase: f64,
    pub tran: Option<Wave>,
}

impl Source {
    pub fn at(&self, t: Option<f64>) -> f64 {
        match (t, &self.tran) {
            (Some(t), Some(w)) => w.value(t),
            _ => self.dc,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiodeModel {
    pub is: f64,
    pub n: f64,
    /// Zero-bias depletion capacitance, its built-in potential, grading and the forward-bias
    /// linearisation point (fraction of `vj`).
    pub cjo: f64,
    pub vj: f64,
    pub m: f64,
    pub fc: f64,
    /// Transit time: diffusion charge `tt · id`.
    pub tt: f64,
    /// Series resistance (an internal node in the circuit).
    pub rs: f64,
    /// Reverse breakdown voltage (infinite: none) and the current at it.
    pub bv: f64,
    pub ibv: f64,
    /// Flicker noise coefficient and exponent.
    pub kf: f64,
    pub af: f64,
    /// Band gap (eV) and saturation current temperature exponent.
    pub eg: f64,
    pub xti: f64,
    /// Temperature the parameters were measured at (°C).
    pub tnom: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MosModel {
    pub pmos: bool,
    pub vto: f64,
    pub kp: f64,
    pub lambda: f64,
    pub gamma: f64,
    pub phi: f64,
    /// Oxide thickness; 0 when not given (no intrinsic gate capacitance, as in SPICE).
    pub tox: f64,
    /// Lateral diffusion: the effective channel length is `l - 2 ld`.
    pub ld: f64,
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,
    /// Zero-bias junction capacitance per area (F/m²) and per perimeter (F/m), their grading
    /// coefficients, the junction potential and the forward-bias linearisation point.
    pub cj: f64,
    pub cjsw: f64,
    pub mj: f64,
    pub mjsw: f64,
    pub pb: f64,
    pub fc: f64,
    /// Bulk junction saturation current, absolute (A) or per junction area (A/m²) when `js > 0`.
    pub is: f64,
    pub js: f64,
    /// Drain / source series resistance, absolute (Ω) or `rsh` per square times `nrd` / `nrs`.
    pub rd: f64,
    pub rs: f64,
    pub rsh: f64,
    /// Flicker noise coefficient and exponent.
    pub kf: f64,
    pub af: f64,
    /// Temperature the parameters were measured at (°C).
    pub tnom: f64,
    /// BSIM4 card (`level=14` / `54`): the drain current comes from it; the level-1 fields
    /// above only describe junctions, overlap and Meyer capacitances.
    pub bsim4: Option<Bsim4Dev>,
}

/// A BSIM4 device: the model card, the instance's finger count and, once the engine knows the
/// temperature, the size- and temperature-dependent parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Bsim4Dev {
    pub card: Arc<bsim4::Card>,
    pub nf: f64,
    /// Instance threshold shift and mobility multiplier (`delvto`, `mulu0`).
    pub shift: bsim4::Shift,
    pub inst: Option<Arc<bsim4::Inst>>,
}

/// Drain / source junction geometry of a MOS instance (m², m), per device of `m`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Junctions {
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
}

const EPS_OX: f64 = 3.453e-11;

impl MosModel {
    /// Gate oxide capacitance per area (F/m²), 0 without `tox`.
    pub fn cox(&self) -> f64 {
        if self.tox > 0.0 { EPS_OX / self.tox } else { 0.0 }
    }

    pub fn leff(&self, l: f64) -> f64 {
        (l - 2.0 * self.ld).max(1e-9)
    }

    /// The model at `temp` °C (SPICE level 1 temperature scaling): mobility ∝ T^-1.5, surface
    /// potential and threshold from the silicon band gap, junction saturation current, potential
    /// and capacitance.
    pub fn at_temp(&self, temp: f64) -> MosModel {
        let (tnom, t) = (self.tnom + KELVIN, temp + KELVIN);
        // BSIM4 scales with temperature itself (see `bsim4::Inst::new`).
        if (t - tnom).abs() < 1e-9 || self.bsim4.is_some() {
            return self.clone();
        }
        let ty = if self.pmos { -1.0 } else { 1.0 };
        let (g, g1) = (silicon(t), silicon(tnom));
        let ratio = t / tnom;
        let phio = (self.phi - g1.pbfact) / g1.fact;
        let tphi = g.fact * phio + g.pbfact;
        let vbi = self.vto - ty * self.gamma * self.phi.sqrt() + 0.5 * (g1.egfet - g.egfet) + ty * 0.5 * (tphi - self.phi);
        let sat = (-g.egfet / g.vt + g1.egfet / g1.vt).exp();
        let (pb, cap) = junction_temp(self.pb, g, g1);
        MosModel {
            kp: self.kp / (ratio * ratio.sqrt()),
            phi: tphi,
            vto: vbi + ty * self.gamma * tphi.sqrt(),
            is: self.is * sat,
            js: self.js * sat,
            pb,
            cj: self.cj * cap(self.mj),
            cjsw: self.cjsw * cap(self.mjsw),
            tnom: temp,
            ..self.clone()
        }
    }
}

impl DiodeModel {
    /// The model at `temp` °C: saturation current from `eg` / `xti`, junction potential and
    /// capacitance as in SPICE.
    pub fn at_temp(&self, temp: f64) -> DiodeModel {
        let (tnom, t) = (self.tnom + KELVIN, temp + KELVIN);
        if (t - tnom).abs() < 1e-9 {
            return self.clone();
        }
        let (g, g1) = (silicon(t), silicon(tnom));
        let ratio = t / tnom;
        let (vj, cap) = junction_temp(self.vj, g, g1);
        DiodeModel {
            is: self.is * ((ratio - 1.0) * self.eg / (self.n * g.vt) + self.xti / self.n * ratio.ln()).exp(),
            vj,
            cjo: self.cjo * cap(self.m),
            tnom: temp,
            ..self.clone()
        }
    }
}

const KELVIN: f64 = 273.15;
const REFTEMP: f64 = 300.15;

/// Silicon band gap terms at `t` K (SPICE `mos1temp`): thermal voltage, band gap and the
/// built-in potential correction relative to 300.15 K.
#[derive(Clone, Copy)]
struct Silicon {
    vt: f64,
    egfet: f64,
    pbfact: f64,
    fact: f64,
}

fn silicon(t: f64) -> Silicon {
    let vt = 1.380649e-23 * t / 1.602176634e-19;
    let vtref = 1.380649e-23 * REFTEMP / 1.602176634e-19;
    let egfet = 1.16 - 7.02e-4 * t * t / (t + 1108.0);
    let arg = -egfet / (2.0 * vt) + 1.1150877 / (2.0 * vtref);
    let fact = t / REFTEMP;
    Silicon { vt, egfet, pbfact: -2.0 * vt * (1.5 * fact.ln() + arg), fact }
}

/// Junction potential at `g` from `pb` at `g1`, and the capacitance factor for a grading
/// coefficient.
fn junction_temp(pb: f64, g: Silicon, g1: Silicon) -> (f64, impl Fn(f64) -> f64) {
    let pbo = (pb - g1.pbfact) / g1.fact;
    let t_pb = g.fact * pbo + g.pbfact;
    let (gmaold, gmanew) = ((pb - pbo) / pbo, (t_pb - pbo) / pbo);
    let (dt_old, dt_new) = (4e-4 * (g1.fact - 1.0) * REFTEMP, 4e-4 * (g.fact - 1.0) * REFTEMP);
    (t_pb, move |m: f64| (1.0 + m * (dt_new - gmanew)) / (1.0 + m * (dt_old - gmaold)))
}

/// Node index: 0 is ground.
pub type Node = usize;

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    R {
        name: String,
        a: Node,
        b: Node,
        r: f64,
    },
    C {
        name: String,
        a: Node,
        b: Node,
        c: f64,
    },
    L {
        name: String,
        a: Node,
        b: Node,
        l: f64,
    },
    V {
        name: String,
        a: Node,
        b: Node,
        src: Source,
    },
    I {
        name: String,
        a: Node,
        b: Node,
        src: Source,
    },
    D {
        name: String,
        a: Node,
        k: Node,
        model: DiodeModel,
    },
    M {
        name: String,
        d: Node,
        g: Node,
        s: Node,
        b: Node,
        model: MosModel,
        w: f64,
        l: f64,
        m: f64,
        junctions: Junctions,
    },
    /// A linear controlled source between `a` (+) and `b` (−): `E` / `G` controlled by the
    /// voltage `cp` − `cn`, `F` / `H` by the current through voltage source `vctl`.
    Ctrl {
        name: String,
        kind: CtrlKind,
        a: Node,
        b: Node,
        cp: Node,
        cn: Node,
        vctl: String,
        gain: f64,
    },
    /// A bipolar transistor (Gummel-Poon) between the internal collector, base and emitter
    /// nodes (behind `rc`, `rb`, `re`), scaled by `area`.
    Q {
        name: String,
        c: Node,
        b: Node,
        e: Node,
        model: BjtModel,
        area: f64,
    },
}

/// SPICE Gummel-Poon bipolar transistor model (`.model q npn` / `pnp`).
#[derive(Debug, Clone, PartialEq)]
pub struct BjtModel {
    pub pnp: bool,
    pub is: f64,
    pub bf: f64,
    pub br: f64,
    pub nf: f64,
    pub nr: f64,
    /// Forward and reverse Early voltages (infinite: none) and knee currents.
    pub vaf: f64,
    pub var: f64,
    pub ikf: f64,
    pub ikr: f64,
    /// Base-emitter and base-collector leakage (non-ideal base current).
    pub ise: f64,
    pub ne: f64,
    pub isc: f64,
    pub nc: f64,
    pub rb: f64,
    pub re: f64,
    pub rc: f64,
    /// Junction capacitances, potentials and grading; transit times.
    pub cje: f64,
    pub vje: f64,
    pub mje: f64,
    pub cjc: f64,
    pub vjc: f64,
    pub mjc: f64,
    pub fc: f64,
    pub tf: f64,
    pub tr: f64,
    pub kf: f64,
    pub af: f64,
    /// Temperature: band gap (eV), saturation current and beta exponents, and the temperature
    /// (°C) the parameters were measured at.
    pub eg: f64,
    pub xti: f64,
    pub xtb: f64,
    pub tnom: f64,
}

impl BjtModel {
    /// The model at `temp` °C, as SPICE's `bjttemp`: saturation and leakage currents, betas,
    /// junction potentials and capacitances.
    pub fn at_temp(&self, temp: f64) -> BjtModel {
        let (tnom, t) = (self.tnom + KELVIN, temp + KELVIN);
        if (t - tnom).abs() < 1e-9 {
            return self.clone();
        }
        let (g, g1) = (silicon(t), silicon(tnom));
        let ratlog = (t / tnom).ln();
        let factlog = (t / tnom - 1.0) * self.eg / g.vt + self.xti * ratlog;
        let bfactor = (ratlog * self.xtb).exp();
        let (vje, cape) = junction_temp(self.vje, g, g1);
        let (vjc, capc) = junction_temp(self.vjc, g, g1);
        BjtModel {
            is: self.is * factlog.exp(),
            bf: self.bf * bfactor,
            br: self.br * bfactor,
            ise: self.ise * (factlog / self.ne).exp() / bfactor,
            isc: self.isc * (factlog / self.nc).exp() / bfactor,
            vje,
            vjc,
            cje: self.cje * cape(self.mje),
            cjc: self.cjc * capc(self.mjc),
            tnom: temp,
            ..self.clone()
        }
    }
}

/// Bipolar model parameters the built-in engine evaluates.
const BJT_PARAMS: &[&str] = &[
    "is", "bf", "br", "nf", "nr", "vaf", "va", "var", "vb", "ikf", "ik", "ikr", "ise", "ne", "isc", "nc", "rb", "re", "rc",
    "cje", "vje", "pe", "mje", "me", "cjc", "vjc", "pc", "mjc", "mc", "fc", "tf", "tr", "kf", "af", "level", "tnom", "eg", "xti",
    "xtb",
];

/// The four linear controlled sources of SPICE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlKind {
    /// Voltage-controlled voltage source.
    E,
    /// Current-controlled current source.
    F,
    /// Voltage-controlled current source.
    G,
    /// Current-controlled voltage source.
    H,
}

impl Element {
    pub fn name(&self) -> &str {
        match self {
            Element::R { name, .. }
            | Element::C { name, .. }
            | Element::L { name, .. }
            | Element::V { name, .. }
            | Element::I { name, .. }
            | Element::D { name, .. }
            | Element::M { name, .. }
            | Element::Ctrl { name, .. }
            | Element::Q { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Circuit {
    pub title: String,
    /// Node names; index 0 is ground ("0").
    pub nodes: Vec<String>,
    pub elements: Vec<Element>,
    pub analyses: Vec<Analysis>,
    pub temp: f64,
    /// Simulator options (`.option`).
    pub options: Options,
    /// Initial conditions (`.ic`): nodes held at these voltages for the transient's initial
    /// operating point, or the start values with `uic`.
    pub ic: Vec<(Node, f64)>,
    /// Starting guesses (`.nodeset`) for the operating point.
    pub nodeset: Vec<(Node, f64)>,
    /// `.tran ... uic`: start the transient from `.ic` without an operating point.
    pub uic: bool,
    /// Deck content the built-in engine ignores or approximates (unsupported model levels and
    /// parameters, dot-commands); empty when it evaluates the deck as written.
    pub warnings: Vec<String>,
}

/// Simulator options of the built-in engine (`.option name=value`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Newton convergence: relative, absolute current (A) and voltage (V) tolerances.
    pub reltol: f64,
    pub abstol: f64,
    pub vntol: f64,
    /// Relative tolerance of the transient truncation error.
    pub lte_reltol: f64,
    /// Truncation error overestimation factor.
    pub trtol: f64,
    pub gmin: f64,
    /// Newton iteration limits: DC and per transient step.
    pub itl1: usize,
    pub itl4: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { reltol: 1e-4, abstol: 1e-12, vntol: 1e-6, lte_reltol: 1e-3, trtol: 7.0, gmin: 1e-12, itl1: 150, itl4: 60 }
    }
}

impl Options {
    /// Apply `name = value`; false for options the engine does not use.
    fn set(&mut self, name: &str, v: f64) -> bool {
        match name {
            "reltol" => {
                // ngspice uses one reltol; the engine's Newton loop stays at least as tight
                // as its default.
                self.reltol = v.min(1e-4);
                self.lte_reltol = v;
            }
            "abstol" => self.abstol = v,
            "vntol" => self.vntol = v,
            "trtol" => self.trtol = v,
            "gmin" => self.gmin = v,
            "itl1" => self.itl1 = v.max(1.0) as usize,
            "itl4" => self.itl4 = v.max(1.0) as usize,
            _ => return false,
        }
        true
    }
}

impl Circuit {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// MOS model parameters the built-in engine evaluates.
const MOS_PARAMS: &[&str] = &[
    "level", "vto", "kp", "u0", "lambda", "gamma", "phi", "tox", "ld", "cgso", "cgdo", "cgbo", "cj", "cjsw", "mj", "mjsw", "pb",
    "fc", "is", "js", "rd", "rs", "rsh", "kf", "af", "tnom", "lmin", "lmax", "wmin", "wmax",
];
/// Diode model parameters the built-in engine evaluates.
const DIODE_PARAMS: &[&str] =
    &["is", "n", "cjo", "cj0", "vj", "m", "fc", "tt", "rs", "bv", "ibv", "kf", "af", "eg", "xti", "tnom"];
/// MOS instance parameters the built-in engine evaluates.
const MOS_INSTANCE_PARAMS: &[&str] = &["w", "l", "m", "ad", "as", "pd", "ps", "nrd", "nrs", "delvto", "mulu0"];
/// BSIM4 instance parameters the built-in engine evaluates.
const BSIM4_INSTANCE_PARAMS: &[&str] = &["w", "l", "m", "nf", "ad", "as", "pd", "ps", "nrd", "nrs", "delvto", "mulu0"];
/// Dot-commands the built-in engine reads.
const DOT_COMMANDS: &[&str] = &[".model", ".temp", ".end", ".title", ".op", ".dc", ".ac", ".tran", ".noise"];

/// The device-side node of terminal `ext` behind a series resistance `r` (a new internal node
/// `dev#tag` and a resistor), or `ext` itself without one.
fn series(c: &mut Circuit, node: &mut impl FnMut(&str, &mut Circuit) -> Node, dev: &str, tag: &str, ext: Node, r: f64) -> Node {
    if !(r > 0.0 && r.is_finite()) {
        return ext;
    }
    let int = node(&format!("{dev}#{tag}"), c);
    c.elements.push(Element::R { name: format!("{dev}#r{tag}"), a: ext, b: int, r });
    int
}

fn ignored_params(p: &HashMap<String, f64>, known: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = p.keys().filter(|k| !known.contains(&k.as_str())).cloned().collect();
    v.sort();
    v
}

fn tokens(s: &str) -> Vec<String> {
    s.replace(['(', ')', ',', '='], " ").split_whitespace().map(|t| t.to_string()).collect()
}

fn kv_params(toks: &[String]) -> HashMap<String, f64> {
    // tokens are already split on '=' so pairs alternate key, value
    let mut m = HashMap::new();
    let mut i = 0;
    while i + 1 < toks.len() {
        if let Some(v) = parse_spice(&toks[i + 1]) {
            m.insert(toks[i].to_ascii_lowercase(), v);
            i += 2;
        } else {
            i += 1;
        }
    }
    m
}

fn parse_source(rest: &str) -> Result<Source, String> {
    let toks = tokens(rest);
    let mut src = Source::default();
    let mut i = 0;
    let is_kw = |t: &str| matches!(t.to_ascii_lowercase().as_str(), "dc" | "ac" | "pulse" | "sin" | "pwl" | "exp");
    let nums_from = |i: &mut usize| -> Vec<f64> {
        let mut v = vec![];
        while *i < toks.len() && !is_kw(&toks[*i]) {
            if let Some(x) = parse_spice(&toks[*i]) {
                v.push(x);
            }
            *i += 1;
        }
        v
    };
    while i < toks.len() {
        let t = toks[i].to_ascii_lowercase();
        i += 1;
        match t.as_str() {
            "dc" => {
                let v = nums_from(&mut i);
                src.dc = *v.first().ok_or("DC without value")?;
            }
            "ac" => {
                let v = nums_from(&mut i);
                src.ac_mag = v.first().copied().unwrap_or(1.0);
                src.ac_phase = v.get(1).copied().unwrap_or(0.0);
            }
            "pulse" => {
                let v = nums_from(&mut i);
                let g = |k: usize, d: f64| v.get(k).copied().unwrap_or(d);
                src.tran = Some(Wave::Pulse {
                    v1: g(0, 0.0),
                    v2: g(1, 0.0),
                    td: g(2, 0.0),
                    tr: g(3, 0.0),
                    tf: g(4, 0.0),
                    pw: g(5, f64::INFINITY),
                    per: g(6, 0.0),
                });
            }
            "sin" => {
                let v = nums_from(&mut i);
                let g = |k: usize, d: f64| v.get(k).copied().unwrap_or(d);
                src.tran = Some(Wave::Sin {
                    vo: g(0, 0.0),
                    va: g(1, 0.0),
                    freq: g(2, 0.0),
                    td: g(3, 0.0),
                    theta: g(4, 0.0),
                    phase: g(5, 0.0),
                });
            }
            "pwl" => {
                // Time / value pairs, then the optional `r=` (repeat from) and `td=` (delay).
                let (mut v, mut r, mut td) = (vec![], None, 0.0);
                while i < toks.len() && !is_kw(&toks[i]) {
                    let k = toks[i].to_ascii_lowercase();
                    if k == "r" || k == "td" {
                        let x =
                            toks.get(i + 1).and_then(|t| parse_spice(t)).ok_or_else(|| format!("PWL: '{k}=' needs a value"))?;
                        if k == "r" {
                            r = Some(x)
                        } else {
                            td = x
                        }
                        i += 2;
                        continue;
                    }
                    v.push(parse_spice(&toks[i]).ok_or_else(|| format!("PWL: '{}' is not a number", toks[i]))?);
                    i += 1;
                }
                if v.len() % 2 != 0 {
                    return Err("PWL needs time / value pairs".into());
                }
                let pts: Vec<(f64, f64)> = v.as_chunks::<2>().0.iter().map(|c| (c[0] + td, c[1])).collect();
                src.tran = Some(match r {
                    Some(r) => {
                        if !pts.iter().any(|p| (p.0 - td - r).abs() <= 1e-15 * r.abs().max(1e-12)) && r != 0.0 {
                            return Err(format!("PWL: r={r} is not one of its time points"));
                        }
                        Wave::PwlRepeat { pts, r: r + td }
                    }
                    None => Wave::Pwl(pts),
                });
            }
            other => {
                if let Some(x) = parse_spice(other) {
                    src.dc = x;
                } else {
                    return Err(format!("unsupported source keyword '{other}'"));
                }
            }
        }
    }
    Ok(src)
}

/// Join continuation lines and drop comments. Returns (line number, text).
fn logical_lines(deck: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = vec![];
    for (n, raw) in deck.lines().enumerate() {
        let line = raw.split(';').next().unwrap_or("").trim_end();
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('*') {
            continue;
        }
        if let Some(cont) = t.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(cont);
            }
            continue;
        }
        out.push((n + 1, t.to_string()));
    }
    out
}

/// The analysis of a `.op` / `.dc` / `.ac` / `.tran` line.
fn dot_analysis(l: &str) -> Option<Analysis> {
    let t: Vec<&str> = l.split_whitespace().collect();
    let num = |k: usize| t.get(k).map(|s| s.to_string()).unwrap_or_default();
    Some(match t.first()?.to_ascii_lowercase().as_str() {
        ".op" => Analysis::Op,
        ".dc" => Analysis::Dc { source: num(1), start: num(2), stop: num(3), step: num(4) },
        ".ac" => Analysis::Ac {
            points_per_decade: t.get(2).and_then(|s| s.parse().ok()).unwrap_or(10),
            fstart: num(3),
            fstop: num(4),
            sweep: t.get(1).and_then(|s| ics_core::sim_setup::AcSweep::parse(s)).unwrap_or_default(),
        },
        ".tran" => {
            let uic = t.iter().any(|s| s.eq_ignore_ascii_case("uic"));
            let nums: Vec<String> = t.iter().skip(3).filter(|s| !s.eq_ignore_ascii_case("uic")).map(|s| s.to_string()).collect();
            Analysis::Tran {
                tstep: num(1),
                tstop: num(2),
                tstart: nums.first().cloned().unwrap_or_default(),
                tmax: nums.get(1).cloned().unwrap_or_default(),
                uic,
            }
        }
        ".noise" => Analysis::Noise {
            output: num(1).trim_start_matches(['v', 'V']).trim_matches(['(', ')']).to_string(),
            source: num(2),
            points_per_decade: t.get(4).and_then(|s| s.parse().ok()).unwrap_or(10),
            fstart: num(5),
            fstop: num(6),
        },
        _ => return None,
    })
}

/// The deck's own analyses, without parsing its elements (for backends that read the deck
/// themselves, such as ngspice with PDK subcircuits and model libraries).
pub fn deck_analyses(deck: &str) -> Vec<Analysis> {
    let mut out = vec![];
    let mut in_control = false;
    for (_, l) in logical_lines(deck) {
        let lower = l.to_ascii_lowercase();
        if lower.starts_with(".control") {
            in_control = true;
        } else if lower.starts_with(".endc") {
            in_control = false;
        } else if lower.split_whitespace().next() == Some(".end") {
            break;
        } else if !in_control && let Some(a) = dot_analysis(&l) {
            out.push(a);
        }
    }
    out
}

pub fn parse_deck(deck: &str) -> Result<Circuit, SimError> {
    let mut c = Circuit { nodes: vec!["0".into()], temp: 27.0, ..Default::default() };
    c.title = deck.lines().next().unwrap_or("").trim_start_matches('*').trim().to_string();
    let mut node_map: HashMap<String, usize> = HashMap::new();
    node_map.insert("0".into(), 0);
    node_map.insert("gnd".into(), 0);
    let mut node = |name: &str, c: &mut Circuit| -> usize {
        let key = name.to_ascii_lowercase();
        if let Some(&i) = node_map.get(&key) {
            return i;
        }
        c.nodes.push(key.clone());
        node_map.insert(key, c.nodes.len() - 1);
        c.nodes.len() - 1
    };

    let lines = logical_lines(deck);
    // (.ic rather than .nodeset, node, value, line), resolved once all nodes are known.
    let mut pending_ic: Vec<(bool, String, f64, usize)> = vec![];
    // Resistor temperature coefficients, applied once `.temp` is known: (element, (tc1, tc2)).
    let mut pending_tc: Vec<(usize, (f64, f64))> = vec![];
    let mut scale = 1.0;
    // First pass: models.
    let mut diode_models: HashMap<String, DiodeModel> = HashMap::new();
    let mut bjt_models: HashMap<String, BjtModel> = HashMap::new();
    let mut mos_models: HashMap<String, MosModel> = HashMap::new();
    let mut in_control = false;
    for (model_line, l) in &lines {
        let lower = l.to_ascii_lowercase();
        if lower.starts_with(".control") {
            in_control = true;
        }
        if lower.starts_with(".endc") {
            in_control = false;
            continue;
        }
        if in_control || !lower.starts_with(".model") {
            continue;
        }
        let toks = tokens(l);
        if toks.len() < 3 {
            continue;
        }
        let name = toks[1].to_ascii_lowercase();
        let ty = toks[2].to_ascii_lowercase();
        let p = kv_params(&toks[3..]);
        let g = |k: &str, d: f64| p.get(k).copied().unwrap_or(d);
        let known = match ty.as_str() {
            "d" => Some(DIODE_PARAMS),
            "nmos" | "pmos" => Some(MOS_PARAMS),
            "npn" | "pnp" => Some(BJT_PARAMS),
            _ => None,
        };
        let bsim4_card = known == Some(MOS_PARAMS) && matches!(g("level", 1.0) as i64, 14 | 54);
        if let Some(known) = known.filter(|_| !bsim4_card) {
            let ignored = ignored_params(&p, known);
            if !ignored.is_empty() {
                c.warnings.push(format!("model {name}: parameters not evaluated by the built-in engine: {}", ignored.join(" ")));
            }
        }
        let is_bsim4 = known == Some(MOS_PARAMS) && matches!(g("level", 1.0) as i64, 14 | 54);
        if is_bsim4 {
            let card = bsim4::Card::new(ty == "pmos", p.clone());
            for w in card.warnings() {
                c.warnings.push(format!("model {name}: BSIM4 {w} is not evaluated by the built-in engine"));
            }
            let pmos = ty == "pmos";
            let m = MosModel {
                pmos,
                vto: g("vth0", g("vtho", if pmos { -0.7 } else { 0.7 })),
                kp: 0.0,
                lambda: 0.0,
                gamma: 0.0,
                phi: 0.8,
                tox: g("toxe", 3.0e-9),
                ld: 0.0,
                // Overlap capacitances are set per instance by the engine.
                cgso: 0.0,
                cgdo: 0.0,
                cgbo: 0.0,
                cj: g("cjs", g("cj", 5.0e-4)),
                cjsw: g("cjsws", g("cjsw", 5.0e-10)),
                mj: g("mjs", g("mj", 0.5)),
                mjsw: g("mjsws", g("mjsw", 0.33)),
                pb: g("pbs", g("pb", 1.0)),
                fc: 0.5,
                is: 0.0,
                js: g("jss", g("js", 1.0e-4)),
                rd: 0.0,
                rs: 0.0,
                rsh: 0.0,
                kf: g("kf", 0.0),
                af: g("af", 1.0),
                tnom: g("tnom", 27.0),
                bsim4: Some(Bsim4Dev { card: Arc::new(card), nf: 1.0, shift: bsim4::Shift::default(), inst: None }),
            };
            mos_models.insert(name, m);
            continue;
        }
        if known == Some(MOS_PARAMS) && g("level", 1.0) != 1.0 {
            // Running another model as level 1 gives plausible but wrong currents: refuse, so
            // that Auto picks ngspice and the built-in engine says why.
            return Err(SimError::Parse {
                line: *model_line,
                msg: format!(
                    "model {name}: MOS level={} is not supported by the built-in engine (it evaluates levels 1, 14 and 54); use ngspice",
                    g("level", 1.0)
                ),
            });
        }
        match ty.as_str() {
            "npn" | "pnp" => {
                let inf = |k: &str, alt: &str| {
                    let v = g(k, g(alt, 0.0));
                    if v > 0.0 { v } else { f64::INFINITY }
                };
                let q = BjtModel {
                    pnp: ty == "pnp",
                    is: g("is", 1e-16),
                    bf: g("bf", 100.0),
                    br: g("br", 1.0),
                    nf: g("nf", 1.0),
                    nr: g("nr", 1.0),
                    vaf: inf("vaf", "va"),
                    var: inf("var", "vb"),
                    ikf: inf("ikf", "ik"),
                    ikr: inf("ikr", "ikr"),
                    ise: g("ise", 0.0),
                    ne: g("ne", 1.5),
                    isc: g("isc", 0.0),
                    nc: g("nc", 2.0),
                    rb: g("rb", 0.0),
                    re: g("re", 0.0),
                    rc: g("rc", 0.0),
                    cje: g("cje", 0.0),
                    vje: g("vje", g("pe", 0.75)),
                    mje: g("mje", g("me", 0.33)),
                    cjc: g("cjc", 0.0),
                    vjc: g("vjc", g("pc", 0.75)),
                    mjc: g("mjc", g("mc", 0.33)),
                    fc: g("fc", 0.5),
                    tf: g("tf", 0.0),
                    tr: g("tr", 0.0),
                    kf: g("kf", 0.0),
                    af: g("af", 1.0),
                    eg: g("eg", 1.11),
                    xti: g("xti", 3.0),
                    xtb: g("xtb", 0.0),
                    tnom: g("tnom", 27.0),
                };
                bjt_models.insert(name, q);
            }
            "d" => {
                let d = DiodeModel {
                    is: g("is", 1e-14),
                    n: g("n", 1.0),
                    cjo: g("cjo", g("cj0", 0.0)),
                    vj: g("vj", 1.0),
                    m: g("m", 0.5),
                    fc: g("fc", 0.5),
                    tt: g("tt", 0.0),
                    rs: g("rs", 0.0),
                    bv: g("bv", f64::INFINITY),
                    ibv: g("ibv", 1e-3),
                    kf: g("kf", 0.0),
                    af: g("af", 1.0),
                    eg: g("eg", 1.11),
                    xti: g("xti", 3.0),
                    tnom: g("tnom", 27.0),
                };
                diode_models.insert(name, d);
            }
            "nmos" | "pmos" => {
                // SPICE level 1 defaults: without `tox` there is no intrinsic gate capacitance
                // and `kp` defaults to 2e-5; with it, `kp = u0 · cox`.
                let tox = g("tox", 0.0);
                let kp = match p.get("kp") {
                    Some(kp) => *kp,
                    None if tox > 0.0 => g("u0", 600.0) * 1e-4 * EPS_OX / tox,
                    None => 2e-5,
                };
                let m = MosModel {
                    pmos: ty == "pmos",
                    vto: g("vto", 0.0),
                    kp,
                    lambda: g("lambda", 0.0),
                    gamma: g("gamma", 0.0),
                    phi: g("phi", 0.6),
                    tox,
                    ld: g("ld", 0.0),
                    cgso: g("cgso", 0.0),
                    cgdo: g("cgdo", 0.0),
                    cgbo: g("cgbo", 0.0),
                    cj: g("cj", 0.0),
                    cjsw: g("cjsw", 0.0),
                    mj: g("mj", 0.5),
                    mjsw: g("mjsw", 0.5),
                    pb: g("pb", 0.8),
                    fc: g("fc", 0.5),
                    is: g("is", 1e-14),
                    js: g("js", 0.0),
                    rd: g("rd", 0.0),
                    rs: g("rs", 0.0),
                    rsh: g("rsh", 0.0),
                    kf: g("kf", 0.0),
                    af: g("af", 1.0),
                    tnom: g("tnom", 27.0),
                    bsim4: None,
                };
                mos_models.insert(name, m);
            }
            _ => {}
        }
    }

    let err = |line: usize, msg: String| SimError::Parse { line, msg };
    in_control = false;
    for (ln, l) in &lines {
        let ln = *ln;
        let lower = l.to_ascii_lowercase();
        if lower.starts_with(".control") {
            in_control = true;
            continue;
        }
        if lower.starts_with(".endc") {
            in_control = false;
            continue;
        }
        if in_control {
            continue;
        }
        if lower.starts_with('.') {
            if lower.split_whitespace().next() == Some(".end") {
                break;
            }
            let cmd = lower.split_whitespace().next().unwrap_or("");
            if let Some(a) = dot_analysis(l) {
                if cmd == ".tran" && lower.split_whitespace().any(|w| w == "uic") {
                    c.uic = true;
                }
                if cmd == ".dc" && lower.split_whitespace().count() > 5 {
                    c.warnings.push(format!("line {ln}: the second '.dc' sweep source is ignored by the built-in engine"));
                }
                if cmd == ".ac" && !lower.split_whitespace().nth(1).is_some_and(|k| matches!(k, "dec" | "oct" | "lin")) {
                    c.warnings.push(format!("line {ln}: '.ac' needs dec, oct or lin; read as dec"));
                }
                c.analyses.push(a);
            } else if cmd == ".temp" {
                c.temp = l.split_whitespace().nth(1).and_then(parse_spice).unwrap_or(27.0);
            } else if matches!(cmd, ".option" | ".options" | ".opt") {
                let spaced = l.replace('=', " = ");
                let t: Vec<&str> = spaced.split_whitespace().skip(1).collect();
                let mut i = 0;
                while i < t.len() {
                    let name = t[i].to_ascii_lowercase();
                    let value = (t.get(i + 1) == Some(&"=")).then(|| t.get(i + 2).and_then(|v| parse_spice(v))).flatten();
                    i += if t.get(i + 1) == Some(&"=") { 3 } else { 1 };
                    let used = match (name.as_str(), value) {
                        ("temp", Some(v)) => {
                            c.temp = v;
                            true
                        }
                        ("scale", Some(v)) if v > 0.0 => {
                            scale = v;
                            true
                        }
                        (_, Some(v)) => c.options.set(&name, v),
                        _ => false,
                    };
                    if !used {
                        c.warnings.push(format!("line {ln}: option '{name}' is ignored by the built-in engine"));
                    }
                }
            } else if matches!(cmd, ".ic" | ".nodeset") {
                let t = tokens(&l.to_ascii_lowercase());
                let mut i = 1;
                while i < t.len() {
                    if t[i] == "v" && i + 2 < t.len() {
                        match t.get(i + 2).and_then(|v| parse_spice(v)) {
                            Some(v) => pending_ic.push((cmd == ".ic", t[i + 1].clone(), v, ln)),
                            None => c.warnings.push(format!("line {ln}: bad value in '{l}'")),
                        }
                        i += 3;
                    } else {
                        c.warnings.push(format!("line {ln}: '{cmd}' takes v(node)=value, '{}' is ignored", t[i]));
                        i += 1;
                    }
                }
            } else if !DOT_COMMANDS.contains(&cmd) {
                c.warnings.push(format!("line {ln}: '{cmd}' is ignored by the built-in engine"));
            }
            continue;
        }
        let t: Vec<&str> = l.split_whitespace().collect();
        let name = t[0].to_ascii_lowercase();
        let need = |n: usize| -> Result<(), SimError> {
            if t.len() < n { Err(err(ln, format!("'{}' needs at least {} fields", t[0], n))) } else { Ok(()) }
        };
        let value = |k: usize| -> Result<f64, SimError> {
            t.get(k).and_then(|s| parse_spice(s)).ok_or_else(|| err(ln, format!("bad value in '{l}'")))
        };
        let first = name.chars().next().unwrap_or(' ');
        let el = match first {
            'r' | 'c' | 'l' => {
                need(4)?;
                let (a, b) = (node(t[1], &mut c), node(t[2], &mut c));
                let v = value(3)?;
                let p = kv_params(&tokens(&t[4..].join(" ")));
                let m = p.get("m").copied().unwrap_or(1.0);
                if m <= 0.0 {
                    return Err(err(ln, format!("{name}: m must be positive")));
                }
                let known: &[&str] = if first == 'r' { &["m", "tc1", "tc2"] } else { &["m"] };
                let ignored = ignored_params(&p, known);
                if !ignored.is_empty() {
                    c.warnings.push(format!("{name}: parameters not evaluated: {}", ignored.join(" ")));
                }
                match first {
                    'r' => {
                        if v == 0.0 {
                            return Err(err(ln, format!("{name}: zero resistance")));
                        }
                        let tc = (p.get("tc1").copied().unwrap_or(0.0), p.get("tc2").copied().unwrap_or(0.0));
                        if tc != (0.0, 0.0) {
                            pending_tc.push((c.elements.len(), tc));
                        }
                        Element::R { name, a, b, r: v / m }
                    }
                    'c' => Element::C { name, a, b, c: v * m },
                    _ => Element::L { name, a, b, l: v / m },
                }
            }
            'v' | 'i' => {
                need(3)?;
                let (a, b) = (node(t[1], &mut c), node(t[2], &mut c));
                let rest = t[3..].join(" ");
                let src = parse_source(&rest).map_err(|m| err(ln, m))?;
                if first == 'v' { Element::V { name, a, b, src } } else { Element::I { name, a, b, src } }
            }
            'd' => {
                need(4)?;
                let (a, k) = (node(t[1], &mut c), node(t[2], &mut c));
                let model = diode_models
                    .get(&t[3].to_ascii_lowercase())
                    .cloned()
                    .ok_or_else(|| err(ln, format!("unknown diode model '{}'", t[3])))?;
                // Optional area (bare number or area=) and multiplier m scale the junction.
                let rest = tokens(&t[4..].join(" "));
                let mut area = 1.0;
                let mut j = 0;
                if let Some(x) = rest.first().and_then(|s| parse_spice(s)) {
                    area = x;
                    j = 1;
                }
                let p = kv_params(&rest[j..]);
                let off = rest[j..].iter().any(|s| s.eq_ignore_ascii_case("off"));
                let ignored = ignored_params(&p, &["area", "m"]);
                if !ignored.is_empty() || off {
                    let mut what = ignored;
                    if off {
                        what.push("off".into());
                    }
                    c.warnings.push(format!("{name}: parameters not evaluated: {}", what.join(" ")));
                }
                let scale = p.get("area").copied().unwrap_or(area) * p.get("m").copied().unwrap_or(1.0);
                if scale <= 0.0 {
                    return Err(err(ln, format!("{name}: area and m must be positive")));
                }
                let mut model = model;
                model.is *= scale;
                model.cjo *= scale;
                model.ibv *= scale;
                model.rs /= scale;
                let a = series(&mut c, &mut node, &name, "a", a, model.rs);
                Element::D { name, a, k, model }
            }
            'm' => {
                need(6)?;
                let d = node(t[1], &mut c);
                let g = node(t[2], &mut c);
                let s = node(t[3], &mut c);
                let b = node(t[4], &mut c);
                let mut model = mos_models
                    .get(&t[5].to_ascii_lowercase())
                    .cloned()
                    .ok_or_else(|| err(ln, format!("unknown MOS model '{}'", t[5])))?;
                let p = kv_params(&tokens(&t[6..].join(" ")));
                let delvto = p.get("delvto").copied().unwrap_or(0.0);
                let mulu0 = p.get("mulu0").copied().unwrap_or(1.0);
                let known: &[&str] = match &mut model.bsim4 {
                    Some(b) => {
                        b.nf = p.get("nf").copied().unwrap_or(1.0).max(1.0);
                        b.shift = bsim4::Shift { delvto, mulu0 };
                        BSIM4_INSTANCE_PARAMS
                    }
                    None => {
                        model.vto += delvto;
                        model.kp *= mulu0;
                        MOS_INSTANCE_PARAMS
                    }
                };
                let ignored = ignored_params(&p, known);
                if !ignored.is_empty() {
                    c.warnings.push(format!("{name}: instance parameters not evaluated: {}", ignored.join(" ")));
                }
                let mult = p.get("m").copied().unwrap_or(1.0).max(1e-12);
                let r = |abs: f64, squares: &str| {
                    if abs > 0.0 { abs } else { model.rsh * p.get(squares).copied().unwrap_or(0.0) }
                };
                let d = series(&mut c, &mut node, &name, "d", d, r(model.rd, "nrd") / mult);
                let s = series(&mut c, &mut node, &name, "s", s, r(model.rs, "nrs") / mult);
                Element::M {
                    name,
                    d,
                    g,
                    s,
                    b,
                    model,
                    w: p.get("w").copied().unwrap_or(1e-6),
                    l: p.get("l").copied().unwrap_or(1e-6),
                    m: p.get("m").copied().unwrap_or(1.0),
                    junctions: Junctions {
                        ad: p.get("ad").copied().unwrap_or(0.0),
                        as_: p.get("as").copied().unwrap_or(0.0),
                        pd: p.get("pd").copied().unwrap_or(0.0),
                        ps: p.get("ps").copied().unwrap_or(0.0),
                    },
                }
            }
            'q' => {
                // Q c b e [substrate] model [area] [area=..] [m=..]
                need(5)?;
                let is_model = |k: usize| t.get(k).is_some_and(|m| bjt_models.contains_key(&m.to_ascii_lowercase()));
                let mi = if is_model(4) {
                    4
                } else if is_model(5) {
                    5
                } else {
                    return Err(err(ln, format!("{name}: unknown bipolar model '{}'", t[4])));
                };
                if mi == 5 {
                    c.warnings.push(format!("{name}: the substrate node is not evaluated"));
                }
                let model = bjt_models[&t[mi].to_ascii_lowercase()].clone();
                let rest = tokens(&t[mi + 1..].join(" "));
                let mut area = 1.0;
                let mut j = 0;
                if let Some(x) = rest.first().and_then(|s| parse_spice(s)) {
                    area = x;
                    j = 1;
                }
                let p = kv_params(&rest[j..]);
                let ignored = ignored_params(&p, &["area", "m"]);
                if !ignored.is_empty() {
                    c.warnings.push(format!("{name}: parameters not evaluated: {}", ignored.join(" ")));
                }
                let area = p.get("area").copied().unwrap_or(area) * p.get("m").copied().unwrap_or(1.0);
                if area <= 0.0 {
                    return Err(err(ln, format!("{name}: area must be positive")));
                }
                let (cn, bn, en) = (node(t[1], &mut c), node(t[2], &mut c), node(t[3], &mut c));
                let cn = series(&mut c, &mut node, &name, "c", cn, model.rc / area);
                let bn = series(&mut c, &mut node, &name, "b", bn, model.rb / area);
                let en = series(&mut c, &mut node, &name, "e", en, model.re / area);
                Element::Q { name, c: cn, b: bn, e: en, model, area }
            }
            'e' | 'f' | 'g' | 'h' => {
                // Linear forms only: E/G n+ n- nc+ nc- gain, F/H n+ n- vname gain.
                if t.iter().skip(3).any(|w| {
                    let w = w.to_ascii_lowercase();
                    w.starts_with("poly") || w.starts_with("value") || w.starts_with("table") || w.starts_with("laplace")
                }) {
                    return Err(err(
                        ln,
                        format!("{name}: POLY / VALUE / TABLE sources are not supported by the built-in engine"),
                    ));
                }
                let (a, b) = (node(t[1], &mut c), node(t[2], &mut c));
                let kind = match first {
                    'e' => CtrlKind::E,
                    'f' => CtrlKind::F,
                    'g' => CtrlKind::G,
                    _ => CtrlKind::H,
                };
                if matches!(kind, CtrlKind::E | CtrlKind::G) {
                    need(6)?;
                    let (cp, cn) = (node(t[3], &mut c), node(t[4], &mut c));
                    Element::Ctrl { name, kind, a, b, cp, cn, vctl: String::new(), gain: value(5)? }
                } else {
                    need(5)?;
                    Element::Ctrl { name, kind, a, b, cp: 0, cn: 0, vctl: t[3].to_ascii_lowercase(), gain: value(4)? }
                }
            }
            _ => {
                return Err(err(
                    ln,
                    format!("unsupported element '{}' (built-in engine supports R C L V I D M Q E F G H)", t[0]),
                ));
            }
        };
        c.elements.push(el);
    }
    // `.option scale`: MOS geometry is written in units of `scale` metres.
    if scale != 1.0 {
        for e in &mut c.elements {
            if let Element::M { w, l, junctions: j, .. } = e {
                *w *= scale;
                *l *= scale;
                j.ad *= scale * scale;
                j.as_ *= scale * scale;
                j.pd *= scale;
                j.ps *= scale;
            }
        }
    }
    for (k, (tc1, tc2)) in pending_tc {
        if let Some(Element::R { r, .. }) = c.elements.get_mut(k) {
            let dt = c.temp - 27.0;
            *r *= 1.0 + tc1 * dt + tc2 * dt * dt;
        }
    }
    for (is_ic, name, v, ln) in pending_ic {
        match node_map.get(&name) {
            Some(&n) if n > 0 => {
                if is_ic {
                    c.ic.push((n, v))
                } else {
                    c.nodeset.push((n, v))
                }
            }
            _ => c.warnings.push(format!("line {ln}: initial condition for unknown node '{name}' is ignored")),
        }
    }
    let mut seen = std::collections::HashSet::new();
    c.warnings.retain(|w| seen.insert(w.clone()));
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sources() {
        let s = parse_source("DC 0 AC 1 PULSE(0 1.8 1n 100p 100p 4n 10n)").unwrap();
        assert_eq!(s.dc, 0.0);
        assert_eq!(s.ac_mag, 1.0);
        let w = s.tran.unwrap();
        assert!((w.value(3e-9) - 1.8).abs() < 1e-12);
        assert_eq!(w.value(0.0), 0.0);
        let s = parse_source("5").unwrap();
        assert_eq!(s.dc, 5.0);
    }

    #[test]
    fn sources_with_delay_repeat_and_phase() {
        // PWL with td= delays it and r= repeats it from that time point after the last one.
        let s = parse_source("PWL(0 0 1n 1 2n 0) r=0 td=1n").unwrap();
        let w = s.tran.unwrap();
        assert_eq!(w.value(0.5e-9), 0.0);
        assert!((w.value(2e-9) - 1.0).abs() < 1e-12, "delayed peak");
        assert!((w.value(4e-9) - 1.0).abs() < 1e-12, "repeats with period 2n");
        assert!(w.breakpoints(6e-9).iter().any(|t| (t - 5e-9).abs() < 1e-18));
        assert!(parse_source("PWL(0 0 1n 1) r=0.5n").is_err(), "r= must be a time point");
        assert!(parse_source("PWL(0 0 1n)").is_err());
        // SIN phase in degrees.
        let s = parse_source("SIN(0 1 1meg 0 0 90)").unwrap();
        assert!((s.tran.unwrap().value(0.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn device_multipliers_and_scale() {
        let c = parse_deck(
            "* t\nR1 a 0 1k m=2 tc1=1e-3\nC1 a 0 1p m=3\nD1 a 0 dm 4 m=2\n.model dm d (is=1e-14 rs=8)\n\
            .temp 127\n.op\n.end\n",
        )
        .unwrap();
        let find = |n: &str| c.elements.iter().find(|e| e.name() == n).unwrap().clone();
        let Element::R { r, .. } = find("r1") else { panic!() };
        assert!((r - 500.0 * 1.1).abs() < 1e-9, "{r}");
        let Element::C { c: cap, .. } = find("c1") else { panic!() };
        assert!((cap - 3e-12).abs() < 1e-24);
        let Element::D { model, .. } = find("d1") else { panic!() };
        assert!((model.is - 8e-14).abs() < 1e-26 && (model.rs - 1.0).abs() < 1e-12, "{model:?}");
        let c = parse_deck("* t\n.option scale=1e-6\nM1 d g 0 0 nm w=2 l=0.5 ad=1\n.model nm nmos (vto=0.4 kp=100u)\nV1 d 0 1\nV2 g 0 1\n.op\n.end\n").unwrap();
        let Element::M { w, l, junctions, .. } = &c.elements.iter().find(|e| e.name() == "m1").unwrap() else { panic!() };
        assert!((w - 2e-6).abs() < 1e-18 && (l - 0.5e-6).abs() < 1e-18 && (junctions.ad - 1e-12).abs() < 1e-24);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn ac_sweeps_and_tran_window() {
        use ics_core::sim_setup::AcSweep;
        let deck = "* t\nV1 in 0 dc 0 ac 1\nR1 in out 1k\nC1 out 0 1n\n.ac lin 11 0 1meg\n.ac oct 2 1k 4k\n\
            .tran 1n 100n 50n 0.5n\n.end\n";
        let c = parse_deck(deck).unwrap();
        assert!(matches!(c.analyses[0], Analysis::Ac { sweep: AcSweep::Lin, .. }));
        let mut e = crate::engine::Engine::new(c.clone()).unwrap();
        let lin = e.run(&c.analyses[0]).unwrap();
        assert_eq!(lin.vectors[0].re, (0..11).map(|k| k as f64 * 1e5).collect::<Vec<_>>());
        let oct = e.run(&c.analyses[1]).unwrap();
        assert_eq!(oct.vectors[0].re.len(), 5, "{:?}", oct.vectors[0].re);
        let tran = e.run(&c.analyses[2]).unwrap();
        let t = &tran.vectors[0].re;
        assert!(
            t[0] >= 50e-9 && t.windows(2).all(|w| w[1] - w[0] <= 0.5e-9 * (1.0 + 1e-9)),
            "starts at tstart, steps under tmax"
        );
        assert_eq!(c.analyses[2].spice_command(), "tran 1n 100n 50n 0.5n");
    }

    #[test]
    fn warns_about_what_it_does_not_evaluate() {
        let deck = "* t\n.param wn=1u\n.option reltol=1e-4 post\n.model nm nmos (vto=0.4 u0=0.03)\n\
                    .model dm d (is=1e-14 rs=10 trs=1e-3)\nM1 d g 0 0 nm w=1u l=1u nf=2 sa=0.2u\nV1 d 0 1\nV2 g 0 1 \n\
                    R1 d 0 1k ic=1\n.dc V1 0 1 0.1 V2 0 1 0.5\n.op\n.end\n";
        let c = parse_deck(deck).unwrap();
        let w = c.warnings.join("\n");
        for s in [
            "'.param'",
            "option 'post'",
            "parameters not evaluated by the built-in engine: trs",
            "nf sa",
            "r1: parameters not evaluated: ic",
            "second '.dc'",
        ] {
            assert!(w.contains(s), "{s} missing from\n{w}");
        }
        // Another MOS level is refused rather than evaluated as level 1.
        let e = parse_deck("* t\n.model nm nmos (level=8 vto=0.4)\nM1 d g 0 0 nm\nV1 d 0 1\nV2 g 0 1\n.op\n.end\n").unwrap_err();
        assert!(e.to_string().contains("level=8 is not supported"), "{e}");
        let clean = format!(
            "* t\nV1 a 0 1\nR1 a 0 1k\nM1 a a 0 0 nmos w=1u l=1u ad=1p\n{}\n.op\n.end\n",
            ics_core::netlist::DEFAULT_MODELS
        );
        assert!(parse_deck(&clean).unwrap().warnings.is_empty());
    }

    #[test]
    fn series_resistances_get_internal_nodes() {
        let deck = "* t\n.model nm nmos (rsh=10)\n.model dm d (rs=5)\nM1 d g 0 0 nm w=1u l=1u nrd=2 m=2\nD1 a 0 dm\n.end\n";
        let c = parse_deck(deck).unwrap();
        let rs: Vec<(&str, f64)> = c
            .elements
            .iter()
            .filter_map(|e| if let Element::R { name, r, .. } = e { Some((name.as_str(), *r)) } else { None })
            .collect();
        assert_eq!(rs, vec![("m1#rd", 10.0), ("d1#ra", 5.0)]);
        assert!(c.nodes.iter().any(|n| n == "m1#d") && c.nodes.iter().any(|n| n == "d1#a"));
    }

    #[test]
    fn parse_basic_deck() {
        let deck = "* t\nV1 in 0 DC 1\nR1 in out 1k\nC1 out 0 1n\n+ \n.model nmos nmos (vto=0.5 kp=100u)\nM1 out in 0 0 nmos w=1u l=1u\n.tran 1n 10n\n.end\n";
        let c = parse_deck(deck).unwrap();
        assert_eq!(c.elements.len(), 4);
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.analyses.len(), 1);
    }
}
