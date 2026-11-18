//! BSIM4 MOSFET model (`level=14` / `level=54`): drain current.
//!
//! The equations follow BSIM4 4.8 as implemented in ngspice (`b4temp.c`, `b4ld.c`),
//! Copyright (c) University of California, BSIM Research Group (BSIM4 is licensed under the
//! Educational Community License 2.0). Derivatives come from forward-mode automatic
//! differentiation ([`D`]) instead of the hand-written chains of the reference code.
//!
//! Covered: threshold voltage with short-channel, DIBL, narrow-width, body, pocket-implant and
//! DITS terms; poly depletion; the unified subthreshold-to-strong-inversion `Vgsteff`; bulk
//! charge; mobility models 0, 1 and 2; velocity saturation and overshoot; source-end velocity
//! limit; the internal source/drain resistance (`rdsmod=0`); CLM, DIBL, DITS and SCBE output
//! conductance; temperature (`tempmod` 0 to 3); size dependence (`l*`, `w*`, `p*` binning);
//! multiple fingers. Not covered (reported as warnings): gate tunnelling, GIDL / GISL,
//! substrate current, external / bias-dependent `rdsmod=1` resistances, gate and body resistance
//! networks, layout-dependent stress and well-proximity effects, high-k `mtrlmod`, and the BSIM4
//! charge model (gate capacitances use the engine's Meyer model).

use std::collections::HashMap;
use std::ops::{Add, Div, Mul, Neg, Sub};

const EPS0: f64 = 8.85418e-12;
const EPSSI: f64 = 1.03594e-10;
const Q: f64 = 1.60219e-19;
const CHARGE: f64 = 1.6021766208e-19;
const KBOQ: f64 = 8.617087e-5;
/// Boltzmann constant as in ngspice's noise code.
const BOLTZ: f64 = 1.38064852e-23;
const N_MINLOG: f64 = 1.0e-38;
const MAX_EXP: f64 = 5.834617425e14;
const MIN_EXP: f64 = 1.713908431e-15;
const EXP_THRESHOLD: f64 = 34.0;
/// The reference code's value of pi, kept so results match it to the last digit.
#[allow(clippy::approx_constant)]
const PI: f64 = 3.141592654;

/// A value with its derivatives with respect to (Vds, Vgs, Vbs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct D {
    pub v: f64,
    pub d: [f64; 3],
}

impl D {
    pub const fn c(v: f64) -> D {
        D { v, d: [0.0; 3] }
    }

    fn var(v: f64, k: usize) -> D {
        let mut d = [0.0; 3];
        d[k] = 1.0;
        D { v, d }
    }

    /// `f(self)` given `f(v)` and `f'(v)`.
    fn chain(self, f: f64, df: f64) -> D {
        D { v: f, d: self.d.map(|x| x * df) }
    }

    pub fn sqrt(self) -> D {
        let s = self.v.sqrt();
        self.chain(s, if s > 0.0 { 0.5 / s } else { 0.0 })
    }

    pub fn exp(self) -> D {
        let e = self.v.exp();
        self.chain(e, e)
    }

    pub fn ln(self) -> D {
        self.chain(self.v.ln(), 1.0 / self.v)
    }

    pub fn powf(self, p: f64) -> D {
        self.chain(self.v.powf(p), p * self.v.powf(p - 1.0))
    }
}

macro_rules! ops {
    ($tr:ident, $f:ident, $op:tt, |$a:ident, $b:ident| $d:expr) => {
        #[allow(clippy::suspicious_arithmetic_impl)]
        impl $tr for D {
            type Output = D;
            fn $f(self, $b: D) -> D {
                let $a = self;
                D { v: $a.v $op $b.v, d: std::array::from_fn(|k| $d(k)) }
            }
        }
        impl $tr<f64> for D {
            type Output = D;
            fn $f(self, b: f64) -> D {
                self $op D::c(b)
            }
        }
        impl $tr<D> for f64 {
            type Output = D;
            fn $f(self, b: D) -> D {
                D::c(self) $op b
            }
        }
    };
}

ops!(Add, add, +, |a, b| |k: usize| a.d[k] + b.d[k]);
ops!(Sub, sub, -, |a, b| |k: usize| a.d[k] - b.d[k]);
ops!(Mul, mul, *, |a, b| |k: usize| a.d[k] * b.v + a.v * b.d[k]);
ops!(Div, div, /, |a, b| |k: usize| (a.d[k] * b.v - a.v * b.d[k]) / (b.v * b.v));

impl Neg for D {
    type Output = D;
    fn neg(self) -> D {
        D { v: -self.v, d: self.d.map(|x| -x) }
    }
}

/// `e^x / ((e^x - 1)² + 2 e^x MIN_EXP)`, the short-channel roll-off factor of BSIM4.
fn theta(x: f64) -> f64 {
    if x < EXP_THRESHOLD {
        let e = x.exp();
        e / ((e - 1.0).powi(2) + 2.0 * e * MIN_EXP)
    } else {
        1.0 / (MAX_EXP - 2.0)
    }
}

/// Same as [`theta`] for a bias-dependent argument.
fn theta_d(x: D) -> D {
    if x.v < EXP_THRESHOLD {
        let e = x.exp();
        let t = e - 1.0;
        e / (t * t + 2.0 * MIN_EXP * e)
    } else {
        D::c(1.0 / (MAX_EXP - 2.0))
    }
}

/// A BSIM4 model card: its parameters by lower-case name.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub pmos: bool,
    pub p: HashMap<String, f64>,
}

/// Parameters whose values change behaviour the port does not model, with the value that
/// keeps the model inside what it evaluates.
const UNMODELLED: &[(&str, f64, &str)] = &[
    ("rdsmod", 0.0, "external source / drain resistances"),
    ("rbodymod", 0.0, "body resistance network"),
    ("rgatemod", 0.0, "gate resistance"),
    ("trnqsmod", 0.0, "transient non-quasi-static model"),
    ("acnqsmod", 0.0, "AC non-quasi-static model"),
    ("mtrlmod", 0.0, "material (high-k) model"),
    ("capmod", 2.0, "charge model other than capmod=2"),
];

impl Card {
    pub fn new(pmos: bool, mut p: HashMap<String, f64>) -> Card {
        if let Some(v) = p.remove("vtho") {
            p.entry("vth0".into()).or_insert(v);
        }
        Card { pmos, p }
    }

    fn given(&self, k: &str) -> bool {
        self.p.contains_key(k)
    }

    fn g(&self, k: &str, d: f64) -> f64 {
        self.p.get(k).copied().unwrap_or(d)
    }

    /// What the built-in engine does not evaluate in this card.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = vec![];
        for (k, ok, what) in UNMODELLED {
            if let Some(v) = self.p.get(*k)
                && *v != *ok
            {
                w.push(format!("{what} ({k}={v})"));
            }
        }
        for (k, ok, what) in [("tnoimod", 0.0, "thermal noise model"), ("diomod", 1.0, "junction diode model")] {
            if let Some(v) = self.p.get(k)
                && *v != ok
            {
                w.push(format!("{what} {k}={v} (evaluated as {k}={ok})"));
            }
        }
        for k in ["jtss", "jtsd", "jtssws", "jtsswd", "jtsswgs", "jtsswgd"] {
            if self.p.get(k).is_some_and(|v| *v != 0.0) {
                w.push(format!("trap-assisted junction tunnelling ({k})"));
            }
        }
        let mobmod = self.g("mobmod", 0.0);
        if mobmod > 2.0 {
            w.push(format!("mobility model mobmod={mobmod} (evaluated as mobmod=0)"));
        }
        if let Some(v) = self.p.get("version")
            && !(4.8..4.9).contains(v)
        {
            w.push(format!("version={v} (evaluated with BSIM4.8 equations)"));
        }
        w
    }
}

/// Instance settings of a BSIM4 device that the model reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Drawn width (all fingers) and length, m.
    pub w: f64,
    pub l: f64,
    /// Number of fingers.
    pub nf: f64,
    /// Drain / source junction area (m²) and perimeter (m); 0 when not given (BSIM4 then
    /// derives them from the finger layout).
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
}

/// Per-instance shifts of a BSIM4 device (ngspice's `delvto` and `mulu0` instance
/// parameters): Monte Carlo mismatch and process draws go through them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shift {
    /// Added to vth0 (V, in the card's sign convention: negative values raise a PMOS |Vth|).
    pub delvto: f64,
    /// Multiplies the low-field mobility u0.
    pub mulu0: f64,
}

impl Default for Shift {
    fn default() -> Self {
        Self { delvto: 0.0, mulu0: 1.0 }
    }
}

impl Geometry {
    pub fn new(w: f64, l: f64, nf: f64) -> Geometry {
        Geometry { w, l, nf, ad: 0.0, as_: 0.0, pd: 0.0, ps: 0.0 }
    }
}

/// Zero-bias capacitances (F) and grading of one source / drain junction: bottom, sidewall
/// and gate-edge sidewall.
#[derive(Debug, Clone, PartialEq, Default)]
struct Junction {
    cz: [f64; 3],
    mj: [f64; 3],
    pb: [f64; 3],
}

impl Junction {
    /// Depletion charge at junction voltage `v` (bulk minus source / drain).
    fn charge(&self, v: D) -> D {
        if v.v < 0.0 {
            let mut q = D::c(0.0);
            for k in 0..3 {
                if self.cz[k] > 0.0 {
                    let arg = 1.0 - v / self.pb[k];
                    q = q + self.pb[k] * self.cz[k] * (1.0 - arg.powf(1.0 - self.mj[k])) / (1.0 - self.mj[k]);
                }
            }
            q
        } else {
            let t0: f64 = self.cz.iter().sum();
            let t1 = v * (0..3).map(|k| self.cz[k] * self.mj[k] / self.pb[k]).sum::<f64>();
            v * (t0 + 0.5 * t1)
        }
    }
}

/// A BSIM4 device at one size and temperature: the size- and temperature-dependent parameters,
/// ready for bias evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Inst {
    ty: f64,
    mobmod: i32,
    tempmod: i32,
    nf: f64,
    vtm: f64,
    vtm0: f64,
    toxe: f64,
    toxp: f64,
    coxe: f64,
    coxp: f64,
    factor1: f64,
    temp_ratio: f64,
    pub leff: f64,
    pub weff: f64,
    pub leff_cv: f64,
    pub weff_cv: f64,
    xj: f64,
    phi: f64,
    sqrt_phi: f64,
    xdep0: f64,
    vbi: f64,
    litl: f64,
    cdep0: f64,
    mstar: f64,
    voffcbn: f64,
    k1: f64,
    k1ox: f64,
    k2ox: f64,
    k3: f64,
    k3b: f64,
    w0: f64,
    lpe0: f64,
    lpeb: f64,
    kt1: f64,
    kt1l: f64,
    kt2: f64,
    dvt0: f64,
    dvt1: f64,
    dvt2: f64,
    dvt0w: f64,
    dvt1w: f64,
    dvt2w: f64,
    eta0: f64,
    etab: f64,
    theta0vb0: f64,
    nfactor: f64,
    cdsc: f64,
    cdscb: f64,
    cdscd: f64,
    cit: f64,
    dvtp0: f64,
    dvtp1: f64,
    dvtp4: f64,
    dvtp2factor: f64,
    vfb: f64,
    ngate: f64,
    pub vth0: f64,
    dwg: f64,
    dwb: f64,
    rds0: f64,
    rdswmin: f64,
    prwg: f64,
    prwb: f64,
    a0: f64,
    ags: f64,
    b0: f64,
    b1: f64,
    keta: f64,
    ua: f64,
    ub: f64,
    uc: f64,
    ud: f64,
    eu: f64,
    u0temp: f64,
    vsattemp: f64,
    vtfbphi1: f64,
    vtfbphi2: f64,
    a1: f64,
    a2: f64,
    delta: f64,
    lambda: Option<f64>,
    pvag: f64,
    pclm: f64,
    fprout: f64,
    theta_rout: f64,
    pdiblb: f64,
    pdits: f64,
    pditsd: f64,
    pditsl: f64,
    pscbe1: f64,
    pscbe2: f64,
    ados: f64,
    bdos: f64,
    vtl: Option<f64>,
    tfactor: f64,
    vbsc: f64,
    /// Overlap capacitances (F): gate-source and gate-drain per finger including the fringe,
    /// gate-bulk for all fingers; with the bias-dependent overlap parameters.
    cgso: f64,
    cgdo: f64,
    cgbo: f64,
    cgsl: f64,
    cgdl: f64,
    ckappas: f64,
    ckappad: f64,
    // C-V model.
    xpart: f64,
    cvchargemod: i32,
    noff: f64,
    voffcv: f64,
    voffcbncv: f64,
    mstarcv: f64,
    acde: f64,
    moin: f64,
    ldeb: f64,
    abulk_cv_factor: f64,
    ketac: f64,
    vfbzb: f64,
    jct_s: Junction,
    jct_d: Junction,
    dio_s: JunctionDc,
    dio_d: JunctionDc,
    leak: Option<Leakage>,
    // Noise.
    temp_k: f64,
    tnoimod: i32,
    fnoimod: i32,
    ntnoi: f64,
    noia: f64,
    noib: f64,
    noic: f64,
    em: f64,
    ef: f64,
    lintnoi: f64,
    kf: f64,
    af: f64,
}

/// Leakage parameters (size dependent), present when any leakage current is enabled.
#[derive(Debug, Clone, PartialEq)]
struct Leakage {
    alpha0: f64,
    alpha1: f64,
    beta0: f64,
    gidlmod: i32,
    /// a, b, c, e, r, k, f of GIDL and GISL.
    gidl: [f64; 7],
    gisl: [f64; 7],
    gidlclamp: f64,
    weff_cj: f64,
    igcmod: i32,
    igbmod: i32,
    nigc: f64,
    /// (a, b, c) of the gate-to-channel, gate-to-source and gate-to-drain tunnelling.
    igc: [f64; 3],
    igs: [f64; 3],
    igd: [f64; 3],
    pigcd: Option<f64>,
    aechvb: f64,
    bechvb: f64,
    aechvb_edge_s: f64,
    aechvb_edge_d: f64,
    bechvb_edge: f64,
    vfbsd: f64,
    igbacc: [f64; 3],
    igbinv: [f64; 3],
    nigbacc: f64,
    nigbinv: f64,
    eigbinv: f64,
    igb_area: f64,
}

/// Leakage currents of a BSIM4 device.
pub struct Leak {
    /// Currents into the drain, gate, source and bulk terminals (device polarity), with their
    /// derivatives with respect to (Vds, Vgs, Vbs); they sum to zero.
    pub terminal: [D; 4],
    /// Gate currents to the source, drain and bulk (for shot noise).
    pub gate_s: f64,
    pub gate_d: f64,
    pub gate_b: f64,
}

/// `exp(x)` clamped to [MIN_EXP, MAX_EXP] at |x| > 34, as the tunnelling expressions do.
fn exp_clamped(x: D) -> D {
    if x.v > EXP_THRESHOLD {
        D::c(MAX_EXP)
    } else if x.v < -EXP_THRESHOLD {
        D::c(MIN_EXP)
    } else {
        x.exp()
    }
}

/// `t0 ln(1 + e^(x/t0))`, the smooth positive part used by the tunnelling currents.
fn soft_plus(x: D, t0: f64) -> D {
    let r = x / t0;
    if r.v > EXP_THRESHOLD {
        x
    } else if r.v < -EXP_THRESHOLD {
        D::c(t0 * (1.0 + MIN_EXP).ln())
    } else {
        t0 * (1.0 + r.exp()).ln()
    }
}

/// Tunnelling current density factor `exp(b (a + (ac - b) v - bc v²))` scaled.
fn tunnel(coef: [f64; 3], bexp: f64, v: D) -> D {
    let [a, b, c] = coef;
    exp_clamped(bexp * (a + (a * c - b) * v - b * c * v * v))
}

/// GIDL (or GISL) current: `p` = (a, b, c, e, r, k, f), `vdx` the drain (source) voltage over
/// the gate, `vgx_eff` the gate drive, `vbx` the bulk-junction voltage.
fn gidl_current(p: &[f64; 7], gidlmod: i32, clamp: f64, weff_cj: f64, tox3: f64, vdx: D, vgx_eff: D, vbx: D) -> D {
    const EXPL_THRESHOLD: f64 = 100.0;
    const MIN_EXPL: f64 = 3.720075976e-44;
    const MAX_EXPL: f64 = 2.688117142e43;
    let [a, b, c, e, r, k, f] = *p;
    if gidlmod == 0 {
        let t1 = (vdx - vgx_eff - e) / tox3;
        if a <= 0.0 || b <= 0.0 || t1.v <= 0.0 || c <= 0.0 || vbx.v > 0.0 {
            return D::c(0.0);
        }
        let t2 = b / t1;
        let i = if t2.v < 100.0 { a * weff_cj * t1 * (-t2).exp() } else { a * weff_cj * MIN_EXPL * t1 };
        let t5 = -(vbx * vbx * vbx);
        i * (t5 / (c + t5))
    } else {
        let t1 = (vdx - r * vgx_eff - e) / tox3;
        if a <= 0.0 || b <= 0.0 || t1.v <= 0.0 || c < 0.0 {
            return D::c(0.0);
        }
        let t2 = b / t1;
        let i = if t2.v < EXPL_THRESHOLD { weff_cj * a * t1 * (-t2).exp() } else { weff_cj * a * MIN_EXPL * t1 };
        let mut t4 = vbx - f;
        if t4.v > clamp {
            t4 = D::c(clamp);
        }
        let t6 = if t4.v == 0.0 {
            D::c(MAX_EXPL)
        } else {
            let t5 = k / t4;
            if t5.v < EXPL_THRESHOLD { t5.exp() } else { D::c(MAX_EXPL) }
        };
        i * t6
    }
}

/// Values of the current model the noise model reads.
struct Internals {
    vth: f64,
    vdsat: f64,
    vgsteff: f64,
    vds: f64,
    vdseff: f64,
    ueff: f64,
    coxeff: f64,
    /// Bulk charge factor before the `keta` body-bias scaling.
    abulk: f64,
    abov_vgst2vtm: f64,
    abulk0_q: f64,
    rds: f64,
    xdep: f64,
}

/// DC model of one source / drain junction diode (`diomod=1`).
#[derive(Debug, Clone, PartialEq, Default)]
struct JunctionDc {
    /// Saturation current (A) and emission voltage n·Vt.
    isat: f64,
    nvtm: f64,
    /// Above this forward voltage the current continues linearly (`ijth`).
    vjm: f64,
    ivjm: f64,
}

impl JunctionDc {
    fn new(isat: f64, nvtm: f64, ijth: f64) -> JunctionDc {
        if isat <= 0.0 {
            return JunctionDc::default();
        }
        let vjm = nvtm * (1.0 + ijth / isat).ln();
        JunctionDc { isat, nvtm, vjm, ivjm: isat * (vjm / nvtm).exp() }
    }

    /// Current from bulk into the source / drain at junction voltage `v`, and its conductance.
    fn current(&self, v: f64) -> (f64, f64) {
        if self.isat <= 0.0 {
            return (0.0, 0.0);
        }
        let t2 = v / self.nvtm;
        if t2 < -EXP_THRESHOLD {
            (self.isat * (MIN_EXP - 1.0), 0.0)
        } else if v <= self.vjm {
            let e = t2.exp();
            (self.isat * (e - 1.0), self.isat * e / self.nvtm)
        } else {
            let g = self.ivjm / self.nvtm;
            (self.ivjm - self.isat + g * (v - self.vjm), g)
        }
    }
}

/// Bias-dependent quantities shared by the current and charge models.
struct Front {
    vbseff: D,
    sqrt_phis: D,
    xdep: D,
    lpe_vb: f64,
    vth_narrow_w: f64,
    n: D,
    vth: D,
    vgs_eff: D,
    vgst: D,
    vgsteff: D,
}

/// Drain current of a BSIM4 device at one bias, in the device's own frame (NMOS polarity,
/// drain the higher terminal).
#[derive(Debug, Clone, Copy)]
pub struct Eval {
    /// Drain current and its derivatives with respect to (Vds, Vgs, Vbs).
    pub ids: D,
    pub vth: f64,
    pub vdsat: f64,
    pub vgsteff: f64,
}

impl Inst {
    /// The device of `card` at size `geo` and temperature `temp` °C.
    pub fn new(card: &Card, geo: Geometry, temp: f64) -> Result<Inst, String> {
        Self::with_shift(card, geo, temp, Shift::default())
    }

    /// [`Inst::new`] with the instance's threshold shift and mobility multiplier (`delvto`,
    /// `mulu0`), applied where ngspice's BSIM4 applies them.
    pub fn with_shift(card: &Card, geo: Geometry, temp: f64, shift: Shift) -> Result<Inst, String> {
        let c = card;
        let ty = if c.pmos { -1.0 } else { 1.0 };
        let mobmod = c.g("mobmod", 0.0) as i32;
        let mobmod = if (0..=2).contains(&mobmod) { mobmod } else { 0 };
        let tempmod = c.g("tempmod", 0.0) as i32;
        let binunit = c.g("binunit", 1.0);

        // Oxide.
        let dtox = c.g("dtox", 0.0);
        let (toxe, toxp) = match (c.p.get("toxe"), c.p.get("toxp")) {
            (Some(&e), Some(&p)) => (e, p),
            (Some(&e), None) => (e, e - dtox),
            (None, Some(&p)) => (p + dtox, p),
            (None, None) => (30.0e-10, 30.0e-10 - dtox),
        };
        let toxm = c.g("toxm", toxe);
        let epsrox = c.g("epsrox", 3.9);
        let epssub = EPSSI;
        let coxe = epsrox * EPS0 / toxe;
        let coxp = epsrox * EPS0 / toxp;
        let cf = c.g("cf", 2.0 * epsrox * EPS0 / PI * (1.0 + 0.4e-6 / toxe).ln());

        // Temperatures (K).
        let tnom = c.g("tnom", 27.0) + 273.15;
        let t = temp + 273.15;
        let tratio = t / tnom;
        let del_temp = t - tnom;
        let factor1 = (epssub / (epsrox * EPS0) * toxe).sqrt();
        let vtm0 = KBOQ * tnom;
        let vtm = KBOQ * t;
        let eg0 = 1.16 - 7.02e-4 * tnom * tnom / (tnom + 1108.0);
        let ni = 1.45e10 * (tnom / 300.15) * (tnom / 300.15).sqrt() * (21.5565981 - eg0 / (2.0 * vtm0)).exp();

        // Effective size.
        let nf = geo.nf.max(1.0);
        let lnew = geo.l + c.g("xl", 0.0);
        let wnew = geo.w / nf + c.g("xw", 0.0);
        let (lint, wint) = (c.g("lint", 0.0), c.g("wint", 0.0));
        let (ll, lw, lwl) = (c.g("ll", 0.0), c.g("lw", 0.0), c.g("lwl", 0.0));
        let (wl, ww, wwl) = (c.g("wl", 0.0), c.g("ww", 0.0), c.g("wwl", 0.0));
        let t0 = lnew.powf(c.g("lln", 1.0));
        let t1 = wnew.powf(c.g("lwn", 1.0));
        let dl = lint + ll / t0 + lw / t1 + lwl / (t0 * t1);
        let dlc = c.g("dlc", lint) + c.g("llc", ll) / t0 + c.g("lwc", lw) / t1 + c.g("lwlc", lwl) / (t0 * t1);
        let t2 = lnew.powf(c.g("wln", 1.0));
        let t3 = wnew.powf(c.g("wwn", 1.0));
        let dw = wint + wl / t2 + ww / t3 + wwl / (t2 * t3);
        let tmp2 = c.g("wlc", wl) / t2 + c.g("wwc", ww) / t3 + c.g("wwlc", wwl) / (t2 * t3);
        let dwc_model = c.g("dwc", wint);
        let dwc = dwc_model + tmp2;
        let dwj = c.g("dwj", dwc_model) + tmp2;
        let leff = lnew - 2.0 * dl;
        let weff = wnew - 2.0 * dw;
        let leff_cv = lnew - 2.0 * dlc;
        let weff_cv = wnew - 2.0 * dwc;
        let weff_cj = wnew - 2.0 * dwj;
        for (what, v) in [("length", leff), ("width", weff), ("length for C-V", leff_cv), ("width for C-V", weff_cv)] {
            if v <= 0.0 {
                return Err(format!("BSIM4: effective channel {what} <= 0 (w={} l={})", geo.w, geo.l));
            }
        }
        let k = if binunit == 1.0 { 1e-6 } else { 1.0 };
        let (inv_l, inv_w, inv_lw) = (k / leff, k / weff, k * k / (leff * weff));
        // A size-dependent parameter: p + lp/L + wp/W + pp/(LW).
        let b = |name: &str, d: f64| -> f64 {
            c.g(name, d)
                + c.g(&format!("l{name}"), 0.0) * inv_l
                + c.g(&format!("w{name}"), 0.0) * inv_w
                + c.g(&format!("p{name}"), 0.0) * inv_lw
        };

        let xj = b("xj", 0.15e-6);
        let vsat = b("vsat", 8.0e4);
        let at = b("at", 3.3e4);
        let keta = b("keta", -0.047);
        let mut ndep = b("ndep", 1.7e17);
        let nsub = b("nsub", 6.0e16);
        let nsd = b("nsd", 1.0e20);
        let phin = b("phin", 0.0);
        let ngate = b("ngate", 0.0);
        let gamma1_given = c.given("gamma1");
        let mut vbm = b("vbm", -3.0);
        let mut ua = b("ua", if mobmod == 2 { 1.0e-15 } else { 1.0e-9 });
        let mut ub = b("ub", 1.0e-19);
        let mut uc = b("uc", if mobmod == 1 { -0.0465 } else { -0.0465e-9 });
        let mut ud = b("ud", 0.0);
        let (ua1, ub1) = (b("ua1", 1.0e-9), b("ub1", -1.0e-18));
        let uc1 = b("uc1", if mobmod == 1 { -0.056 } else { -0.056e-9 });
        let ud1 = b("ud1", 0.0);
        let mut u0 = b("u0", if c.pmos { 0.025 } else { 0.067 });
        let ute = b("ute", -1.5);
        let mut eu = b("eu", if c.pmos { 1.0 } else { 1.67 });
        let mut voff = b("voff", -0.08);
        let mut nfactor = b("nfactor", 1.0);
        let mut eta0 = b("eta0", 0.08);
        let (rdsw, prt, wr) = (b("rdsw", 200.0), b("prt", 0.0), b("wr", 1.0));
        let rdswmin_model = c.g("rdswmin", 0.0);

        // Temperature.
        let pow_weff_wr = (weff_cj * 1.0e6).powf(wr) * nf;
        let tr1 = tratio - 1.0;
        let (vsattemp, rds0, rdswmin);
        if tempmod == 0 {
            ua += ua1 * tr1;
            ub += ub1 * tr1;
            uc += uc1 * tr1;
            ud += ud1 * tr1;
            vsattemp = vsat - at * tr1;
            let t10 = prt * tr1;
            rds0 = (rdsw + t10) * nf / pow_weff_wr;
            rdswmin = (rdswmin_model + t10) * nf / pow_weff_wr;
        } else {
            if tempmod == 3 {
                ua *= tratio.powf(ua1);
                ub *= tratio.powf(ub1);
                uc *= tratio.powf(uc1);
                ud *= tratio.powf(ud1);
            } else {
                ua *= 1.0 + ua1 * del_temp;
                ub *= 1.0 + ub1 * del_temp;
                uc *= 1.0 + uc1 * del_temp;
                ud *= 1.0 + ud1 * del_temp;
            }
            vsattemp = vsat * (1.0 - at * del_temp);
            let t10 = 1.0 + prt * del_temp;
            rds0 = rdsw * t10 * nf / pow_weff_wr;
            rdswmin = rdswmin_model * t10 * nf / pow_weff_wr;
        }
        if u0 > 1.0 {
            u0 /= 1.0e4;
        }
        let t5 = 1.0 - b("up", 0.0) * (-leff / b("lp", 1.0e-8)).exp();
        let u0temp = u0 * t5 * tratio.powf(ute) * shift.mulu0;
        eu = eu.max(0.0);
        voff *= 1.0 + b("tvoff", 0.0) * del_temp;
        nfactor += b("tnfactor", 0.0) * del_temp / tnom;
        eta0 += b("teta0", 0.0) * del_temp / tnom;
        let vtl = c.p.get("vtl").copied().filter(|v| *v > 0.0).map(|_| b("vtl", 2.0e5));
        let tfactor = if vtl.is_some() {
            let lc = c.g("lc", 5.0e-9).max(0.0);
            let t0 = leff / (b("xn", 3.0) * leff + lc);
            (1.0 - t0) / (1.0 + t0)
        } else {
            1.0
        };

        // Doping, potentials.
        if !c.given("ndep") && gamma1_given {
            let t0 = b("gamma1", 0.0) * coxe;
            ndep = 3.01248e22 * t0 * t0;
        }
        let phi = vtm0 * (ndep / ni).ln() + phin + 0.4;
        if phi <= 0.0 {
            return Err(format!("BSIM4: phi = {phi} is not positive (phin={phin}, ndep={ndep})"));
        }
        let sqrt_phi = phi.sqrt();
        let xdep0 = (2.0 * epssub / (Q * ndep * 1.0e6)).sqrt() * sqrt_phi;
        let litl = (3.0 * 3.9 / epsrox * xj * toxe).sqrt();
        let vbi = vtm0 * (nsd * ndep / (ni * ni)).ln();
        let cdep0 = (Q * epssub * ndep * 1.0e6 / 2.0 / phi).sqrt();
        let mstar = 0.5 + b("minv", 0.0).atan() / PI;
        let voffcbn = voff + c.g("voffl", 0.0) / leff;

        // Body effect coefficients.
        let (k1, k2);
        if c.given("k1") || c.given("k2") {
            k1 = b("k1", 0.53);
            k2 = b("k2", -0.0186);
        } else {
            let mut vbx = if c.given("vbx") { b("vbx", 0.0) } else { phi - 7.7348e-4 * ndep * b("xt", 1.55e-7).powi(2) };
            if vbx > 0.0 {
                vbx = -vbx;
            }
            if vbm > 0.0 {
                vbm = -vbm;
            }
            let gamma1 = if gamma1_given { b("gamma1", 0.0) } else { 5.753e-12 * ndep.sqrt() / coxe };
            let gamma2 = if c.given("gamma2") { b("gamma2", 0.0) } else { 5.753e-12 * nsub.sqrt() / coxe };
            let t0 = gamma1 - gamma2;
            let t1 = (phi - vbx).sqrt() - sqrt_phi;
            let t2 = (phi * (phi - vbm)).sqrt() - phi;
            k2 = t0 * t1 / (2.0 * t2 + vbm);
            k1 = gamma2 - 2.0 * k2 * (phi - vbm).sqrt();
        }
        let vth0_given = c.given("vth0");
        let vfb = if c.given("vfb") {
            b("vfb", -1.0)
        } else if vth0_given {
            ty * b("vth0", 0.0) - phi - k1 * sqrt_phi
        } else {
            -1.0
        };
        let vth0 = if vth0_given { b("vth0", 0.0) } else { ty * (vfb + phi + k1 * sqrt_phi) };
        // `delvto` moves vth0 and vfb together, as ngspice's b4temp.c does.
        let (vth0, vfb) = (vth0 + shift.delvto, vfb + ty * shift.delvto);
        let k1ox = k1 * toxe / toxm;
        let k2ox = k2 * toxe / toxm;

        let tmp = (epssub / (epsrox * EPS0) * toxe * xdep0).sqrt();
        let drout = b("drout", 0.56);
        let theta0vb0 = theta(b("dsub", drout) * leff / tmp);
        let theta_rout = b("pdiblc1", 0.39) * theta(drout * leff / tmp) + b("pdiblc2", 0.0086);
        let dvtp2factor = b("dvtp5", 0.0) + b("dvtp2", 0.0) * (-b("dvtp3", 0.0) * leff.ln()).exp();

        let t3 = ty * vth0 - vfb - phi;
        let vtfbphi1 = (if c.pmos { 2.5 * t3 } else { 2.0 * t3 }).max(0.0);
        let vtfbphi2 = (4.0 * t3).max(0.0);
        let mut vbsc = if k2 < 0.0 {
            let t0 = 0.5 * k1 / k2;
            (0.9 * (phi - t0 * t0)).clamp(-30.0, -3.0)
        } else {
            -30.0
        };
        if vbsc > vbm {
            vbsc = vbm;
        }

        let dlc_model = c.g("dlc", lint);
        let ov = |given: &str, cgxl: f64| match c.p.get(given) {
            Some(v) => *v,
            None if c.given("dlc") && dlc_model > 0.0 => dlc_model * coxe - cgxl,
            None => 0.6 * c.g("xj", 0.15e-6) * coxe,
        };
        let cgso = (ov("cgso", c.g("cgsl", 0.0)) + cf) * weff_cv;
        let cgdo = (ov("cgdo", c.g("cgdl", 0.0)) + cf) * weff_cv;
        let cgbo = c.g("cgbo", 2.0 * dwc_model * coxe) * leff_cv * nf;

        // C-V: flat-band voltage at zero bias, charge centroid, Vdsat for C-V.
        let tmp2 = factor1 * xdep0.sqrt();
        let t8 = b("dvt0w", 0.0) * theta(b("dvt1w", 5.3e6) * weff * leff / tmp2) * (vbi - phi);
        let t9 = b("dvt0", 2.2) * theta(b("dvt1", 0.53) * leff / tmp2) * (vbi - phi);
        let t4 = toxe * phi / (weff + b("w0", 2.5e-6));
        let t3 = if tempmod < 2 {
            (b("kt1", -0.11) + b("kt1l", 0.0) / leff) * (tratio - 1.0)
        } else {
            -b("kt1", -0.11) * (tratio - 1.0)
        };
        let t5 = k1ox * ((1.0 + b("lpe0", 1.74e-7) / leff).sqrt() - 1.0) * sqrt_phi + t3;
        let vfbzb = -t8 - t9 + b("k3", 80.0) * t4 + t5 - phi - k1 * sqrt_phi + ty * vth0;
        let voffcv = b("voffcv", 0.0) * (1.0 + b("tvoffcv", 0.0) * del_temp);

        // Source / drain junctions: geometry from the instance or the finger layout.
        let weff_cj_nf = weff_cj * nf;
        let (dmcg, dmci, dmdg) = {
            let dmcg = c.g("dmcg", 0.0);
            let dmcgt = c.g("dmcgt", 0.0);
            (dmcg - dmcgt, c.g("dmci", dmcg), c.g("dmdg", 0.0) - dmcgt)
        };
        let _ = dmdg;
        let (end_d, int_d, end_s, int_s) = {
            let nfi = nf as i64;
            if nfi % 2 != 0 {
                let i = 2.0 * ((nf - 1.0) / 2.0).max(0.0);
                (1.0, i, 1.0, i)
            } else if c.g("min", 0.0) == 1.0 {
                (2.0, 2.0 * (nf / 2.0 - 1.0).max(0.0), 0.0, nf)
            } else {
                (0.0, nf, 2.0, 2.0 * (nf / 2.0 - 1.0).max(0.0))
            }
        };
        let p_iso = 2.0 * (dmcg + dmci) + weff_cj;
        let p_sha = 2.0 * dmcg;
        let a_iso = (dmcg + dmci) * weff_cj;
        let a_sha = dmcg * weff_cj;
        let permod = c.g("permod", 1.0);
        let perim = |given: f64, end: f64, int: f64| {
            if given > 0.0 {
                if permod == 0.0 { given } else { (given - weff_cj_nf).max(0.0) }
            } else {
                end * p_iso + int * p_sha
            }
        };
        let area = |given: f64, end: f64, int: f64| if given > 0.0 { given } else { end * a_iso + int * a_sha };
        let (as_eff, ps_eff) = (area(geo.as_, end_s, int_s), perim(geo.ps, end_s, int_s));
        let (ad_eff, pd_eff) = (area(geo.ad, end_d, int_d), perim(geo.pd, end_d, int_d));
        let cap_t = |cj: f64, tc: &str| cj * (1.0 + c.g(tc, 0.0) * del_temp).max(0.0);
        let pb_t = |pb: f64, tp: &str| (pb - c.g(tp, 0.0) * del_temp).max(0.01);
        let cjs = c.g("cjs", 5.0e-4);
        let cjsws = c.g("cjsws", 5.0e-10);
        let cjswgs = c.g("cjswgs", cjsws);
        let (mjs, mjsws) = (c.g("mjs", 0.5), c.g("mjsws", 0.33));
        let mjswgs = c.g("mjswgs", mjsws);
        let (pbs, pbsws) = (c.g("pbs", 1.0), c.g("pbsws", 1.0));
        let pbswgs = c.g("pbswgs", pbsws);
        let jct_s = Junction {
            cz: [cap_t(cjs, "tcj") * as_eff, cap_t(cjsws, "tcjsw") * ps_eff, cap_t(cjswgs, "tcjswg") * weff_cj_nf],
            mj: [mjs, mjsws, mjswgs],
            pb: [pb_t(pbs, "tpb"), pb_t(pbsws, "tpbsw"), pb_t(pbswgs, "tpbswg")],
        };
        let jct_d = Junction {
            cz: [
                cap_t(c.g("cjd", cjs), "tcj") * ad_eff,
                cap_t(c.g("cjswd", cjsws), "tcjsw") * pd_eff,
                cap_t(c.g("cjswgd", cjswgs), "tcjswg") * weff_cj_nf,
            ],
            mj: [c.g("mjd", mjs), c.g("mjswd", mjsws), c.g("mjswgd", mjswgs)],
            pb: [pb_t(c.g("pbd", pbs), "tpb"), pb_t(c.g("pbswd", pbsws), "tpbsw"), pb_t(c.g("pbswgd", pbswgs), "tpbswg")],
        };

        // Junction saturation currents with their temperature dependence.
        let eg = 1.16 - 7.02e-4 * t * t / (t + 1108.0);
        let sat_t =
            |xti: f64, nj: f64| if t != tnom { ((eg0 / vtm0 - eg / vtm + xti * (t / tnom).ln()) / nj).exp() } else { 1.0 };
        let (jss, jsws, jswgs) = (c.g("jss", 1.0e-4), c.g("jsws", 0.0), c.g("jswgs", 0.0));
        let (njs, xtis) = (c.g("njs", 1.0), c.g("xtis", 3.0));
        let (njd, xtid) = (c.g("njd", njs), c.g("xtid", xtis));
        let ks = sat_t(xtis, njs);
        let kd = sat_t(xtid, njd);
        let isat_s = (jss * as_eff + jsws * ps_eff + jswgs * weff_cj_nf) * ks;
        let isat_d = (c.g("jsd", jss) * ad_eff + c.g("jswd", jsws) * pd_eff + c.g("jswgd", jswgs) * weff_cj_nf) * kd;
        let ijths = c.g("ijthsfwd", 0.1);
        let dio_s = JunctionDc::new(isat_s.max(0.0), vtm * njs, ijths);
        let dio_d = JunctionDc::new(isat_d.max(0.0), vtm * njd, c.g("ijthdfwd", ijths));

        // Leakage: substrate current, GIDL / GISL and gate tunnelling.
        let leak = {
            // A GISL parameter defaults to its GIDL counterpart, binning terms included.
            let b2 = |name: &str, alt: &str, d: f64| -> f64 {
                let g = |pre: &str, d: f64| c.g(&format!("{pre}{name}"), c.g(&format!("{pre}{alt}"), d));
                g("", d) + g("l", 0.0) * inv_l + g("w", 0.0) * inv_w + g("p", 0.0) * inv_lw
            };
            let nm = !c.pmos;
            let igcmod = c.g("igcmod", 0.0) as i32;
            let igbmod = c.g("igbmod", 0.0) as i32;
            let alpha0 = b("alpha0", 0.0);
            let alpha1 = b("alpha1", 0.0);
            let beta0 = b("beta0", 0.0);
            let agidl = b("agidl", 0.0);
            let agisl = b2("agisl", "agidl", 0.0);
            let isub_on = alpha0 + alpha1 * leff > 0.0 && beta0 > 0.0;
            if !(isub_on || agidl > 0.0 || agisl > 0.0 || igcmod != 0 || igbmod != 0) {
                None
            } else {
                let ntox = b("ntox", 1.0);
                let poxedge = b("poxedge", 1.0);
                let toxref = c.g("toxref", 30.0e-10);
                let tox_ratio = (ntox * (toxref / toxe).ln()).exp() / toxe / toxe;
                let tox_ratio_edge = (ntox * (toxref / (toxe * poxedge)).ln()).exp() / toxe / toxe / poxedge / poxedge;
                let aechvb0 = if nm { 4.97232e-7 } else { 3.42537e-7 };
                let bechvb0 = if nm { 7.45669e11 } else { 1.16645e12 };
                let dlcig = c.g("dlcig", lint).max(0.0);
                let dlcigd = c.g("dlcigd", dlcig).max(0.0);
                let ig = |n: &str, d_n: f64, d_p: f64, alt: &str| b2(n, alt, if nm { d_n } else { d_p });
                Some(Leakage {
                    alpha0,
                    alpha1,
                    beta0,
                    gidlmod: c.g("gidlmod", 0.0) as i32,
                    gidl: [
                        agidl,
                        b("bgidl", 2.3e9),
                        b("cgidl", 0.5),
                        b("egidl", 0.8),
                        b("rgidl", 1.0),
                        b("kgidl", 0.0),
                        b("fgidl", 1.0),
                    ],
                    gisl: [
                        agisl,
                        b2("bgisl", "bgidl", 2.3e9),
                        b2("cgisl", "cgidl", 0.5),
                        b2("egisl", "egidl", 0.8),
                        b2("rgisl", "rgidl", 1.0),
                        b2("kgisl", "kgidl", 0.0),
                        b2("fgisl", "fgidl", 1.0),
                    ],
                    gidlclamp: c.g("gidlclamp", -1e-5),
                    weff_cj,
                    igcmod,
                    igbmod,
                    nigc: b("nigc", 1.0),
                    igc: [
                        b("aigc", if nm { 1.36e-2 } else { 9.80e-3 }),
                        b("bigc", if nm { 1.71e-3 } else { 7.59e-4 }),
                        b("cigc", if nm { 0.075 } else { 0.03 }),
                    ],
                    igs: [
                        ig("aigs", 1.36e-2, 9.80e-3, "aigsd"),
                        ig("bigs", 1.71e-3, 7.59e-4, "bigsd"),
                        ig("cigs", 0.075, 0.03, "cigsd"),
                    ],
                    igd: [
                        ig("aigd", 1.36e-2, 9.80e-3, "aigsd"),
                        ig("bigd", 1.71e-3, 7.59e-4, "bigsd"),
                        ig("cigd", 0.075, 0.03, "cigsd"),
                    ],
                    pigcd: c.given("pigcd").then(|| b("pigcd", 1.0)),
                    aechvb: aechvb0 * weff * leff * tox_ratio,
                    bechvb: -bechvb0 * toxe,
                    aechvb_edge_s: aechvb0 * weff * dlcig * tox_ratio_edge,
                    aechvb_edge_d: aechvb0 * weff * dlcigd * tox_ratio_edge,
                    bechvb_edge: -bechvb0 * toxe * poxedge,
                    vfbsd: if ngate > 0.0 { vtm0 * (ngate / nsd).ln() } else { 0.0 }
                        + b("vfbsdoff", 0.0) * (1.0 + b("tvfbsdoff", 0.0) * del_temp),
                    igbacc: [b("aigbacc", 1.36e-2), b("bigbacc", 1.71e-3), b("cigbacc", 0.075)],
                    igbinv: [b("aigbinv", 1.11e-2), b("bigbinv", 9.49e-4), b("cigbinv", 0.006)],
                    nigbacc: b("nigbacc", 1.0),
                    nigbinv: b("nigbinv", 3.0),
                    eigbinv: b("eigbinv", 1.1),
                    igb_area: 4.97232e-7 * weff * leff * tox_ratio,
                })
            }
        };

        Ok(Inst {
            ty,
            mobmod,
            tempmod,
            nf,
            vtm,
            vtm0,
            toxe,
            toxp,
            coxe,
            coxp,
            factor1,
            temp_ratio: tratio - 1.0,
            leff,
            weff,
            leff_cv,
            weff_cv,
            xj,
            phi,
            sqrt_phi,
            xdep0,
            vbi,
            litl,
            cdep0,
            mstar,
            voffcbn,
            k1,
            k1ox,
            k2ox,
            k3: b("k3", 80.0),
            k3b: b("k3b", 0.0),
            w0: b("w0", 2.5e-6),
            lpe0: b("lpe0", 1.74e-7),
            lpeb: b("lpeb", 0.0),
            kt1: b("kt1", -0.11),
            kt1l: b("kt1l", 0.0),
            kt2: b("kt2", 0.022),
            dvt0: b("dvt0", 2.2),
            dvt1: b("dvt1", 0.53),
            dvt2: b("dvt2", -0.032),
            dvt0w: b("dvt0w", 0.0),
            dvt1w: b("dvt1w", 5.3e6),
            dvt2w: b("dvt2w", -0.032),
            eta0,
            etab: b("etab", -0.07),
            theta0vb0,
            nfactor,
            cdsc: b("cdsc", 2.4e-4),
            cdscb: b("cdscb", 0.0),
            cdscd: b("cdscd", 0.0),
            cit: b("cit", 0.0),
            dvtp0: b("dvtp0", 0.0),
            dvtp1: b("dvtp1", 0.0),
            dvtp4: b("dvtp4", 0.0),
            dvtp2factor,
            vfb,
            ngate,
            vth0,
            dwg: b("dwg", 0.0),
            dwb: b("dwb", 0.0),
            rds0,
            rdswmin,
            prwg: b("prwg", 1.0),
            prwb: b("prwb", 0.0),
            a0: b("a0", 1.0),
            ags: b("ags", 0.0),
            b0: b("b0", 0.0),
            b1: b("b1", 0.0),
            keta,
            ua,
            ub,
            uc,
            ud,
            eu,
            u0temp,
            vsattemp,
            vtfbphi1,
            vtfbphi2,
            a1: b("a1", 0.0),
            a2: b("a2", 1.0),
            delta: b("delta", 0.01),
            lambda: c.p.get("lambda").copied().filter(|v| *v > 0.0).map(|_| b("lambda", 0.0)),
            pvag: b("pvag", 0.0),
            pclm: b("pclm", 1.3),
            fprout: b("fprout", 0.0),
            theta_rout,
            pdiblb: b("pdiblcb", 0.0),
            pdits: b("pdits", 0.0),
            pditsd: b("pditsd", 0.0),
            pditsl: c.g("pditsl", 0.0),
            pscbe1: b("pscbe1", 4.24e8),
            pscbe2: b("pscbe2", 1.0e-5),
            ados: c.g("ados", 1.0),
            bdos: c.g("bdos", 1.0),
            vtl,
            tfactor,
            vbsc,
            cgso,
            cgdo,
            cgbo,
            cgsl: b("cgsl", 0.0),
            cgdl: b("cgdl", 0.0),
            ckappas: b("ckappas", 0.6),
            ckappad: b("ckappad", c.g("ckappas", 0.6)),
            xpart: c.g("xpart", 0.0),
            cvchargemod: c.g("cvchargemod", 0.0) as i32,
            noff: b("noff", 1.0),
            voffcv,
            voffcbncv: voffcv + c.g("voffcvl", 0.0) / leff,
            mstarcv: 0.5 + b("minvcv", 0.0).atan() / PI,
            acde: b("acde", 1.0) * (ndep / 2.0e16).powf(-0.25),
            moin: b("moin", 15.0),
            ldeb: (epssub * vtm0 / (Q * ndep * 1.0e6)).sqrt() / 3.0,
            abulk_cv_factor: 1.0 + (b("clc", 0.1e-6) / leff_cv).powf(b("cle", 0.6)),
            ketac: b("ketac", c.g("keta", -0.047)),
            vfbzb,
            jct_s,
            jct_d,
            dio_s,
            dio_d,
            leak,
            temp_k: t,
            tnoimod: c.g("tnoimod", 0.0) as i32,
            fnoimod: c.g("fnoimod", 1.0) as i32,
            ntnoi: c.g("ntnoi", 1.0),
            noia: c.g("noia", if c.pmos { 6.188e40 } else { 6.25e41 }),
            noib: c.g("noib", if c.pmos { 1.5e25 } else { 3.125e26 }),
            noic: c.g("noic", 8.75e9),
            em: c.g("em", 4.1e7),
            ef: c.g("ef", 1.0),
            lintnoi: c.g("lintnoi", 0.0),
            kf: c.g("kf", 0.0),
            af: c.g("af", 1.0),
        })
    }

    /// Gate oxide capacitance per area (F/m²).
    pub fn coxe(&self) -> f64 {
        self.coxe
    }

    /// Drain current at `vds >= 0`, `vgs`, `vbs` (NMOS polarity: voltages already multiplied by
    /// the device type), for all fingers.
    pub fn eval(&self, vds: f64, vgs: f64, vbs: f64) -> Eval {
        let vds_d = D::var(vds, 0);
        let vgs_d = D::var(vgs, 1);
        let vbs_d = D::var(vbs, 2);
        let (ids, i, _, _) = self.ids(vds_d, vgs_d, vbs_d);
        Eval { ids: ids * self.nf, vth: i.vth, vdsat: i.vdsat, vgsteff: i.vgsteff }
    }

    /// Body bias, threshold voltage and effective gate drive, shared by current and charge.
    fn front(&self, vds: D, vgs: D, vbs: D) -> Front {
        let p = self;
        let epssub = EPSSI;
        let toxe = p.toxe;
        let vtm = p.vtm;
        let leff = p.leff;

        // Effective body bias.
        let t0 = vbs - p.vbsc - 0.001;
        let t1 = (t0 * t0 - 0.004 * p.vbsc).sqrt();
        let mut vbseff = if t0.v >= 0.0 { p.vbsc + 0.5 * (t0 + t1) } else { p.vbsc * (1.0 + (-0.002) / (t1 - t0)) };
        let t9 = 0.95 * p.phi;
        let t0 = t9 - vbseff - 0.001;
        let t1 = (t0 * t0 + 0.004 * t9).sqrt();
        vbseff = t9 - 0.5 * (t0 + t1);
        let phis = p.phi - vbseff;
        let sqrt_phis = phis.sqrt();
        let xdep = p.xdep0 * sqrt_phis / p.sqrt_phi;

        // Threshold voltage.
        let t3 = xdep.sqrt();
        let v0 = p.vbi - p.phi;
        let smooth = |t0: D| if t0.v >= -0.5 { 1.0 + t0 } else { (1.0 + 3.0 * t0) / (3.0 + 8.0 * t0) };
        let lt1 = p.factor1 * t3 * smooth(p.dvt2 * vbseff);
        let ltw = p.factor1 * t3 * smooth(p.dvt2w * vbseff);
        let theta0 = theta_d(p.dvt1 * leff / lt1);
        let delt_vth = p.dvt0 * theta0 * v0;
        let t2 = p.dvt0w * theta_d(p.dvt1w * p.weff * leff / ltw) * v0;
        let t0 = (1.0 + p.lpe0 / leff).sqrt();
        let t1 = p.k1ox * (t0 - 1.0) * p.sqrt_phi + (p.kt1 + p.kt1l / leff + p.kt2 * vbseff) * p.temp_ratio;
        let vth_narrow_w = toxe * p.phi / (p.weff + p.w0);
        let mut t3 = p.eta0 + p.etab * vbseff;
        if t3.v < 1.0e-4 {
            let t9 = 1.0 / (3.0 - 2.0e4 * t3);
            t3 = (2.0e-4 - t3) * t9;
        }
        let dibl_sft = t3 * p.theta0vb0 * vds;
        let lpe_vb = (1.0 + p.lpeb / leff).sqrt();
        let mut vth = p.ty * p.vth0 + (p.k1ox * sqrt_phis - p.k1 * p.sqrt_phi) * lpe_vb - p.k2ox * vbseff - delt_vth - t2
            + (p.k3 + p.k3b * vbseff) * vth_narrow_w
            + t1
            - dibl_sft;

        // Subthreshold swing factor.
        let tmp1 = epssub / xdep;
        let tmp2 = p.nfactor * tmp1;
        let tmp3 = p.cdsc + p.cdscb * vbseff + p.cdscd * vds;
        let tmp4 = (tmp2 + tmp3 * theta0 + p.cit) / p.coxe;
        let n = smooth(tmp4);

        // Pocket implant.
        if p.dvtp0 > 0.0 {
            let t0 = -p.dvtp1 * vds;
            let t2 = if t0.v < -EXP_THRESHOLD { D::c(MIN_EXP) } else { t0.exp() };
            let t3 = leff + p.dvtp0 * (1.0 + t2);
            let vt = if p.tempmod < 2 { vtm } else { p.vtm0 };
            vth = vth - n * (vt * (leff / t3).ln());
        }
        // Drain-induced threshold shift (DITS).
        if p.dvtp4 != 0.0 && p.dvtp2factor != 0.0 {
            let t0 = dexp(2.0 * p.dvtp4 * vds);
            vth = vth - p.dvtp2factor * (t0 - 1.0) / (t0 + 1.0);
        }

        // Poly depletion.
        let vgs_eff = poly_depletion(p.vfb + p.phi, p.ngate, EPSSI, p.coxe, vgs);
        let vgst = vgs_eff - vth;

        let vgsteff = p.vgsteff(vgst, n, p.mstar, p.voffcbn);
        Front { vbseff, sqrt_phis, xdep, lpe_vb, vth_narrow_w, n, vth, vgs_eff, vgst, vgsteff }
    }

    /// The smooth effective gate drive from `vgst` (`mstar` / `voffcbn`: the DC or C-V set).
    fn vgsteff(&self, vgst: D, n: D, mstar: f64, voffcbn: f64) -> D {
        let p = self;
        let vtm = p.vtm;
        let t0 = n * vtm;
        let t1 = mstar * vgst;
        let t2 = t1 / t0;
        let t10 = if t2.v > EXP_THRESHOLD {
            t1
        } else if t2.v < -EXP_THRESHOLD {
            vtm * (1.0 + MIN_EXP).ln() * n
        } else {
            n * (vtm * (1.0 + t2.exp()).ln())
        };
        let t1 = voffcbn - (1.0 - mstar) * vgst;
        let t2 = t1 / t0;
        let t9 = if t2.v < -EXP_THRESHOLD {
            mstar + n * (p.coxe * MIN_EXP / p.cdep0)
        } else if t2.v > EXP_THRESHOLD {
            mstar + n * (p.coxe * MAX_EXP / p.cdep0)
        } else {
            mstar + n * (p.coxe / p.cdep0 * t2.exp())
        };
        t10 / t9
    }

    /// Bulk charge factor at zero gate drive (before its lower clamp), with the terms the
    /// gate-drive correction reuses: (Abulk0, T1, T5).
    fn abulk0(&self, sqrt_phis: D, xdep: D, lpe_vb: f64, vth_narrow_w: f64) -> (D, D, D) {
        let p = self;
        let t9 = 0.5 * p.k1ox * lpe_vb / sqrt_phis;
        let t1 = t9 + p.k2ox - p.k3b * vth_narrow_w;
        let t9 = (p.xj * xdep).sqrt();
        let t5 = p.leff / (p.leff + 2.0 * t9);
        let t2 = p.a0 * t5 + p.b0 / (p.weff + p.b1);
        (1.0 + t1 * t2, t1, t5)
    }

    /// Whether the device has leakage currents (substrate, GIDL / GISL, gate tunnelling).
    pub fn has_leakage(&self) -> bool {
        self.leak.is_some()
    }

    /// Leakage currents at `vds`, `vgs`, `vbs` (device polarity, `vds` of either sign).
    pub fn leakage(&self, vds: f64, vgs: f64, vbs: f64) -> Option<Leak> {
        const DELTA_3: f64 = 0.02;
        let lk = self.leak.as_ref()?;
        let p = self;
        let (vds, vgs, vbs) = (D::var(vds, 0), D::var(vgs, 1), D::var(vbs, 2));
        let reverse = vds.v < 0.0;
        let (mvds, mvgs, mvbs) = if reverse { (-vds, vgs - vds, vbs - vds) } else { (vds, vgs, vbs) };
        let (ed, es) = if reverse { (2, 0) } else { (0, 2) };
        let f = p.front(mvds, mvgs, mvbs);
        let (_, _, isub, vdseff) = p.ids(mvds, mvgs, mvbs);
        let zero = D::c(0.0);
        let mut i = [zero; 4];
        let nf = p.nf;

        // Substrate current: effective drain to bulk.
        i[ed] = i[ed] + isub * nf;
        i[3] = i[3] - isub * nf;

        // GIDL (drain to bulk) and GISL (source to bulk) in the terminal frame.
        let tox3 = 3.0 * p.toxe;
        let vgs_eff = poly_depletion(p.vfb + p.phi, p.ngate, EPSSI, p.coxe, vgs);
        let vgd_eff = poly_depletion(p.vfb + p.phi, p.ngate, EPSSI, p.coxe, vgs - vds);
        let igidl = gidl_current(&lk.gidl, lk.gidlmod, lk.gidlclamp, lk.weff_cj, tox3, vds, vgs_eff, vbs - vds) * nf;
        let igisl = gidl_current(&lk.gisl, lk.gidlmod, lk.gidlclamp, lk.weff_cj, tox3, -vds, vgd_eff, vbs) * nf;
        i[0] = i[0] + igidl;
        i[2] = i[2] + igisl;
        i[3] = i[3] - igidl - igisl;

        let (mut gate_s, mut gate_d, mut gate_b) = (zero, zero, zero);
        if lk.igcmod != 0 || lk.igbmod != 0 {
            let vt = if p.tempmod < 2 { p.vtm } else { p.vtm0 };
            let vfb = p.vfbzb;
            let v3 = vfb - f.vgs_eff + f.vbseff - DELTA_3;
            let t0 = if vfb <= 0.0 { (v3 * v3 - 4.0 * DELTA_3 * vfb).sqrt() } else { (v3 * v3 + 4.0 * DELTA_3 * vfb).sqrt() };
            let vfbeff = vfb - 0.5 * (v3 + t0);
            let voxacc = if (vfb - vfbeff).v < 0.0 { zero } else { vfb - vfbeff };
            let t0 = 0.5 * p.k1ox;
            let t3 = f.vgs_eff - vfbeff - f.vbseff - f.vgsteff;
            let voxdepinv = if p.k1ox == 0.0 {
                zero
            } else if t3.v < 0.0 {
                -t3
            } else {
                p.k1ox * ((t0 * t0 + t3).sqrt() - t0)
            } + f.vgsteff;

            if lk.igcmod != 0 {
                // Gate to channel, split between source and drain.
                let von = if lk.igcmod == 1 { D::c(p.ty * p.vth0) } else { f.vth };
                let vaux = soft_plus(f.vgs_eff - von, vt * lk.nigc);
                let igc = lk.aechvb * (mvgs * vaux) * tunnel(lk.igc, lk.bechvb, voxdepinv);
                let pigcd = match lk.pigcd {
                    Some(v) => D::c(v),
                    None => {
                        let t12 = f.vgsteff + 1.0e-20;
                        let t13 = (-lk.bechvb) / t12 / t12;
                        t13 * (1.0 - 0.5 * vdseff / t12)
                    }
                };
                let t7 = -pigcd * vdseff;
                let t8 = t7 * t7 + 2.0e-4;
                let t9 = exp_clamped(t7);
                let igcs = igc * ((t9 - 1.0 + 1.0e-4 - t7) / t8);
                let igcd = igc * ((t7 * t9 - (t9 - 1.0 - 1.0e-4)) / t8);
                // Gate to source / drain overlap (edge) tunnelling, terminal frame.
                let edge = |vgx: D, coef: [f64; 3], a: f64| {
                    let t0 = vgx - lk.vfbsd;
                    let vgx_eff = (t0 * t0 + 1.0e-4).sqrt();
                    a * (vgx * vgx_eff) * tunnel(coef, lk.bechvb_edge, vgx_eff)
                };
                let igs = edge(vgs, lk.igs, lk.aechvb_edge_s);
                let igd = edge(vgs - vds, lk.igd, lk.aechvb_edge_d);
                let (to_s, to_d) = if reverse { (igcd, igcs) } else { (igcs, igcd) };
                gate_s = (igs + to_s) * nf;
                gate_d = (igd + to_d) * nf;
            }
            if lk.igbmod != 0 {
                let vaux = soft_plus(-f.vgs_eff + f.vbseff + vfb, vt * lk.nigbacc);
                let t12 = -7.45669e11 * p.toxe;
                let igbacc = lk.igb_area * ((mvgs - mvbs) * vaux) * tunnel(lk.igbacc, t12, voxacc);
                let vaux = soft_plus(voxdepinv - lk.eigbinv, vt * lk.nigbinv);
                let igbinv = lk.igb_area * 0.75610 * ((mvgs - mvbs) * vaux) * tunnel(lk.igbinv, t12 * 1.31724, voxdepinv);
                gate_b = (igbacc + igbinv) * nf;
            }
        }
        i[1] = i[1] + gate_s + gate_d + gate_b;
        i[2] = i[2] - gate_s;
        i[0] = i[0] - gate_d;
        i[3] = i[3] - gate_b;
        let _ = es;
        Some(Leak { terminal: i, gate_s: gate_s.v, gate_d: gate_d.v, gate_b: gate_b.v })
    }

    /// Bulk-source and bulk-drain junction currents at `vbs`, `vbd` (device polarity), with
    /// their conductances: ((ibs, gbs), (ibd, gbd)).
    pub fn junction_currents(&self, vbs: f64, vbd: f64) -> ((f64, f64), (f64, f64)) {
        (self.dio_s.current(vbs), self.dio_d.current(vbd))
    }

    /// Drain-source noise current spectral densities (A²/Hz) at `vds >= 0`, `vgs`, `vbs` (device
    /// polarity, forward frame) and frequency `freq`: (thermal, flicker), for all fingers.
    /// Thermal noise follows `tnoimod=0` (charge based, with the internal Rds), flicker noise
    /// `fnoimod` 1 (unified oxide-trap model) or 0 (`kf`).
    pub fn noise(&self, vds: f64, vgs: f64, vbs: f64, freq: f64) -> (f64, f64) {
        const DELTA_4: f64 = 0.02;
        let p = self;
        let (id, i, _, _) = p.ids(D::c(vds), D::c(vgs), D::c(vbs));
        let cd = (id.v * p.nf).abs();
        let temp = p.temp_k;

        // Thermal: 4kT ueff |Qinv| / (Leff² + ueff |Qinv| Rds).
        let abulk = i.abulk0_q * p.abulk_cv_factor;
        let vdsat = i.vgsteff / abulk;
        let t0 = vdsat - vds - DELTA_4;
        let t1 = (t0 * t0 + 4.0 * DELTA_4 * vdsat).sqrt();
        let mut vdseff = if t0 >= 0.0 { vdsat - 0.5 * (t0 + t1) } else { vdsat * (1.0 - 2.0 * DELTA_4 / (t1 - t0)) };
        if vds == 0.0 {
            vdseff = 0.0;
        }
        let t0 = abulk * vdseff;
        let t1 = 12.0 * (i.vgsteff - 0.5 * t0 + 1.0e-20);
        let t3 = t0 * vdseff / t1;
        let qinv = i.coxeff * p.weff_cv * p.nf * p.leff_cv * (i.vgsteff - 0.5 * t0 + abulk * t3);
        let rds_per = if i.rds > 0.0 { i.rds / p.nf } else { 0.0 };
        let uq = i.ueff * qinv.abs();
        let thermal = 4.0 * BOLTZ * temp * uq / (uq * rds_per + p.leff * p.leff) * p.ntnoi;

        // Flicker.
        let ef = freq.powf(p.ef);
        let flicker = if p.fnoimod == 0 {
            p.kf * cd.max(N_MINLOG).powf(p.af) / (ef * p.leff * p.leff * p.coxe)
        } else {
            let nstar = p.vtm / Q * (p.coxe + EPSSI / i.xdep + p.cit);
            let leff = p.leff - 2.0 * p.lintnoi;
            let leffsq = leff * leff;
            let esat = 2.0 * p.vsattemp / i.ueff;
            let del_clm = if p.em <= 0.0 {
                0.0
            } else {
                let t0 = ((i.vds - i.vdseff) / p.litl + p.em) / esat;
                (p.litl * t0.max(N_MINLOG).ln()).max(0.0)
            };
            let t1 = CHARGE * CHARGE * BOLTZ * cd * temp * i.ueff;
            let t2 = 1.0e10 * ef * i.abulk * p.coxe * leffsq;
            let n0 = p.coxe * i.vgsteff / CHARGE;
            let nl = p.coxe * i.vgsteff * (1.0 - i.abov_vgst2vtm * i.vdseff) / CHARGE;
            let t3 = p.noia * ((n0 + nstar) / (nl + nstar)).max(N_MINLOG).ln();
            let t4 = p.noib * (n0 - nl);
            let t5 = p.noic * 0.5 * (n0 * n0 - nl * nl);
            let t6 = BOLTZ * temp * cd * cd;
            let t7 = 1.0e10 * ef * leffsq * p.weff * p.nf;
            let t8 = p.noia + p.noib * nl + p.noic * nl * nl;
            let t9 = (nl + nstar) * (nl + nstar);
            let ssi = t1 / t2 * (t3 + t4 + t5) + t6 / t7 * del_clm * t8 / t9;
            let swi = p.noia * BOLTZ * temp / (p.weff * p.nf * p.leff * ef * 1.0e10 * nstar * nstar) * cd * cd;
            if ssi + swi > 0.0 { ssi * swi / (ssi + swi) } else { 0.0 }
        };
        (thermal, flicker)
    }

    /// Terminal charges (C) at `vds`, `vgs`, `vbs` in the device's polarity (voltages already
    /// multiplied by the type, `vds` of either sign): drain, gate, source, bulk, with their
    /// derivatives with respect to (Vds, Vgs, Vbs). Intrinsic charge from the charge-thickness
    /// model (`capmod=2`), bias-dependent overlap charges and the junction depletion charges.
    pub fn charges(&self, vds: f64, vgs: f64, vbs: f64) -> [D; 4] {
        let (vds, vgs, vbs) = (D::var(vds, 0), D::var(vgs, 1), D::var(vbs, 2));
        let reverse = vds.v < 0.0;
        let (qg_i, qb_i, qd_i) = if self.xpart < 0.0 {
            (D::c(0.0), D::c(0.0), D::c(0.0))
        } else if reverse {
            self.intrinsic(-vds, vgs - vds, vbs - vds)
        } else {
            self.intrinsic(vds, vgs, vbs)
        };
        let qgdo = overlap(vgs - vds, self.cgdo, self.weff_cv * self.cgdl, self.ckappad) * self.nf;
        let qgso = overlap(vgs, self.cgso, self.weff_cv * self.cgsl, self.ckappas) * self.nf;
        let qgb = self.cgbo * (vgs - vbs);
        let qgate = qg_i + qgdo + qgso + qgb;
        let qbulk = qb_i - qgb;
        let (qdrn, qsrc) = if reverse {
            let qs = qd_i - qgso;
            (-(qgate + qbulk + qs), qs)
        } else {
            let qd = qd_i - qgdo;
            (qd, -(qgate + qbulk + qd))
        };
        let qbs = self.jct_s.charge(vbs);
        let qbd = self.jct_d.charge(vbs - vds);
        [qdrn - qbd, qgate, qsrc - qbs, qbulk + qbd + qbs]
    }

    /// Intrinsic (gate, bulk, drain) charges of `capmod=2` in the forward frame (`vds >= 0`).
    fn intrinsic(&self, vds: D, vgs: D, vbs: D) -> (D, D, D) {
        const DELTA_3: f64 = 0.02;
        const DELTA_4: f64 = 0.02;
        let p = self;
        let epssub = EPSSI;
        let vtm = p.vtm;
        let f = p.front(vds, vgs, vbs);
        let vbseff_cv = f.vbseff;
        let vgs_eff = f.vgs_eff;
        let cox_wl = p.coxe * p.weff_cv * p.leff_cv * p.nf;
        let vgsteff = if p.cvchargemod == 0 {
            let t0 = vtm * (f.n * p.noff);
            let x = (f.vgst - p.voffcv) / t0;
            if x.v > EXP_THRESHOLD {
                f.vgst - p.voffcv
            } else if x.v < -EXP_THRESHOLD {
                t0 * (1.0 + MIN_EXP).ln()
            } else {
                t0 * (1.0 + x.exp()).ln()
            }
        } else {
            p.vgsteff(f.vgst, f.n, p.mstarcv, p.voffcbncv)
        };

        // Accumulation and depletion charges.
        let v3 = p.vfbzb - vgs_eff + vbseff_cv - DELTA_3;
        let t0 =
            if p.vfbzb <= 0.0 { (v3 * v3 - 4.0 * DELTA_3 * p.vfbzb).sqrt() } else { (v3 * v3 + 4.0 * DELTA_3 * p.vfbzb).sqrt() };
        let vfbeff = p.vfbzb - 0.5 * (v3 + t0);
        let cox = p.coxp;
        let tox = 1.0e8 * p.toxp;
        let tmp = (vgs_eff - vbseff_cv - p.vfbzb) / tox * p.acde;
        let tcen = if tmp.v > -EXP_THRESHOLD && tmp.v < EXP_THRESHOLD {
            p.ldeb * tmp.exp()
        } else if tmp.v <= -EXP_THRESHOLD {
            D::c(p.ldeb * MIN_EXP)
        } else {
            D::c(p.ldeb * MAX_EXP)
        };
        let link = 1.0e-3 * p.toxp;
        let v3 = p.ldeb - tcen - link;
        let v4 = (v3 * v3 + 4.0 * link * p.ldeb).sqrt();
        let tcen = p.ldeb - 0.5 * (v3 + v4);
        let ccen = epssub / tcen;
        let coxeff = cox * ccen / (cox + ccen);
        let cox_wl_cen = cox_wl * coxeff / p.coxe;
        let qac0 = cox_wl_cen * (vfbeff - p.vfbzb);
        let t0 = 0.5 * p.k1ox;
        let t3 = vgs_eff - vfbeff - vbseff_cv - vgsteff;
        let t1 = if p.k1ox == 0.0 {
            D::c(0.0)
        } else if t3.v < 0.0 {
            t0 + t3 / p.k1ox
        } else {
            (t0 * t0 + t3).sqrt()
        };
        let qsub0 = cox_wl_cen * p.k1ox * (t1 - t0);

        // Gate-bias dependent surface potential.
        let (denomi, t0) = if p.k1ox <= 0.0 {
            (0.25 * p.moin * vtm, 0.5 * p.sqrt_phi)
        } else {
            (p.moin * vtm * p.k1ox * p.k1ox, p.k1ox * p.sqrt_phi)
        };
        let t1 = 2.0 * t0 + vgsteff;
        let delta_phi = vtm * (1.0 + t1 * vgsteff / denomi).ln();
        let t0 = vgsteff - delta_phi - 0.001;
        let t1 = (t0 * t0 + vgsteff * 0.004).sqrt();
        let vgdp = 0.5 * (t0 + t1);

        // Inversion charge with the charge centroid at this gate drive.
        let t0 = (vgsteff + p.vtfbphi2) / (2.0 * tox);
        let tmp = (p.bdos * 0.7 * t0.ln()).exp();
        let tcen = p.ados * 1.9e-9 / (1.0 + tmp);
        let ccen = epssub / tcen;
        let coxeff = cox * ccen / (cox + ccen);
        let cox_wl_cen = cox_wl * coxeff / p.coxe;
        let (abulk0, _, _) = p.abulk0(f.sqrt_phis, f.xdep, f.lpe_vb, f.vth_narrow_w);
        let abulk0 = if abulk0.v < 0.1 { (0.2 - abulk0) / (3.0 - 20.0 * abulk0) } else { abulk0 };
        let t2 = p.ketac * f.vbseff;
        let kf = if t2.v >= -0.9 { 1.0 / (1.0 + t2) } else { (17.0 + 20.0 * t2) / (0.8 + t2) };
        let abulk_cv = abulk0 * kf * p.abulk_cv_factor;
        let vdsat_cv = vgdp / abulk_cv;
        let t0 = vdsat_cv - vds - DELTA_4;
        let t1 = (t0 * t0 + 4.0 * DELTA_4 * vdsat_cv).sqrt();
        let mut vdseff_cv = if t0.v >= 0.0 { vdsat_cv - 0.5 * (t0 + t1) } else { vdsat_cv * (1.0 - 2.0 * DELTA_4 / (t1 - t0)) };
        if vds.v == 0.0 {
            vdseff_cv = D { v: 0.0, d: [vdseff_cv.d[0], 0.0, 0.0] };
        }
        let t0 = abulk_cv * vdseff_cv;
        let t1 = vgdp;
        let t2 = 12.0 * (t1 - 0.5 * t0 + 1.0e-20);
        let t3 = t0 / t2;
        let mut qgate = cox_wl_cen * (t1 - t0 * (0.5 - t3));
        let mut qbulk = cox_wl_cen * (1.0 - abulk_cv) * (0.5 * vdseff_cv - t0 * vdseff_cv / t2);
        let qsrc = if p.xpart > 0.5 {
            -cox_wl_cen * (t1 / 2.0 + t0 / 4.0 - 0.5 * t0 * t0 / t2)
        } else if p.xpart < 0.5 {
            let t2 = t2 / 12.0;
            let t3 = 0.5 * cox_wl_cen / (t2 * t2);
            let t4 = t1 * (2.0 * t0 * t0 / 3.0 + t1 * (t1 - 4.0 * t0 / 3.0)) - 2.0 * t0 * t0 * t0 / 15.0;
            -(t3 * t4)
        } else {
            -0.5 * qgate
        };
        qgate = qgate + qac0 + qsub0 - qbulk;
        qbulk = qbulk - (qac0 + qsub0);
        let qdrn = -(qgate + qbulk + qsrc);
        (qgate, qbulk, qdrn)
    }

    fn ids(&self, vds: D, vgs: D, vbs: D) -> (D, Internals, D, D) {
        let p = self;
        let epssub = EPSSI;
        let toxe = p.toxe;
        let vtm = p.vtm;
        let leff = p.leff;

        let Front { vbseff, sqrt_phis, xdep, lpe_vb, vth_narrow_w, n: _, vth, vgs_eff: _, vgst: _, vgsteff } =
            self.front(vds, vgs, vbs);

        // Effective width and source/drain resistance.
        let t9 = sqrt_phis - p.sqrt_phi;
        let mut weff = p.weff - 2.0 * (p.dwg * vgsteff + p.dwb * t9);
        if weff.v < 2.0e-8 {
            weff = 2.0e-8 * (4.0e-8 - weff) / (6.0e-8 - 2.0 * weff);
        }
        let rds = {
            let t0 = 1.0 + p.prwg * vgsteff;
            let t1 = p.prwb * t9;
            let t2 = 1.0 / t0 + t1;
            let t3 = t2 + (t2 * t2 + 0.01).sqrt();
            p.rdswmin + t3 * (p.rds0 * 0.5)
        };

        // Bulk charge factor.
        let (abulk0, t1, t5) = self.abulk0(sqrt_phis, xdep, lpe_vb, vth_narrow_w);
        let t7 = t5 * t5 * t5;
        let dabulk_dvg = -t1 * (p.ags * p.a0 * t7);
        let mut abulk = abulk0 + dabulk_dvg * vgsteff;
        if abulk.v < 0.1 {
            abulk = (0.2 - abulk) / (3.0 - 20.0 * abulk);
        }
        let abulk_stored = abulk.v;
        let t2 = p.keta * vbseff;
        let t0 = if t2.v >= -0.9 { 1.0 / (1.0 + t2) } else { (17.0 + 20.0 * t2) / (0.8 + t2) };
        abulk = abulk * t0;
        let abulk0_q = {
            let a = if abulk0.v < 0.1 { (0.2 - abulk0.v) / (3.0 - 20.0 * abulk0.v) } else { abulk0.v };
            let t2 = p.ketac * vbseff.v;
            a * if t2 >= -0.9 { 1.0 / (1.0 + t2) } else { (17.0 + 20.0 * t2) / (0.8 + t2) }
        };

        // Mobility.
        let t12 = (vth * vth + 0.0001).sqrt();
        let t10 = toxe / (vgsteff + 2.0 * t12);
        let t6 = p.ud * t10 * t10 * vth * vth;
        let t5 = match p.mobmod {
            1 => {
                let t3 = (vgsteff + vth + vth) / toxe;
                t3 * (p.ua + p.ub * t3) * (1.0 + p.uc * vbseff) + t6
            }
            2 => {
                let t0 = (vgsteff + p.vtfbphi1) / toxe;
                (p.eu * t0.ln()).exp() * (p.ua + p.uc * vbseff) + t6
            }
            _ => {
                let t3 = (vgsteff + vth + vth) / toxe;
                t3 * (p.ua + p.uc * vbseff + p.ub * t3) + t6
            }
        };
        let denomi = if t5.v >= -0.8 { 1.0 + t5 } else { (0.6 + t5) / (7.0 + 10.0 * t5) };
        let ueff = p.u0temp / denomi;

        // Saturation voltage.
        let wvcox = weff * (p.vsattemp * p.coxe);
        let wvcox_rds = wvcox * rds;
        let mut esat = 2.0 * p.vsattemp / ueff;
        let mut esat_l = esat * leff;
        let lambda = if p.a1 == 0.0 {
            D::c(p.a2)
        } else if p.a1 > 0.0 {
            let t0 = 1.0 - p.a2;
            let t1 = t0 - p.a1 * vgsteff - 0.0001;
            let t2 = (t1 * t1 + 0.0004 * t0).sqrt();
            p.a2 + t0 - 0.5 * (t1 + t2)
        } else {
            let t1 = p.a2 + p.a1 * vgsteff - 0.0001;
            let t2 = (t1 * t1 + 0.0004 * p.a2).sqrt();
            0.5 * (t1 + t2)
        };
        let vgst2vtm = vgsteff + 2.0 * vtm;
        let vdsat = if rds.v == 0.0 && lambda.v == 1.0 {
            esat_l * vgst2vtm / (abulk * esat_l + vgst2vtm)
        } else {
            let t9 = abulk * wvcox_rds;
            let t7 = vgst2vtm * t9;
            let t6 = vgst2vtm * wvcox_rds;
            let t0 = 2.0 * abulk * (t9 - 1.0 + 1.0 / lambda);
            let t1 = vgst2vtm * (2.0 / lambda - 1.0) + abulk * esat_l + 3.0 * t7;
            let t2 = vgst2vtm * (esat_l + 2.0 * t6);
            let t3 = (t1 * t1 - 2.0 * t0 * t2).sqrt();
            (t1 - t3) / t0
        };

        // Effective drain voltage.
        let t1 = vdsat - vds - p.delta;
        let t2 = (t1 * t1 + 4.0 * p.delta * vdsat).sqrt();
        let mut vdseff = if t1.v >= 0.0 {
            vdsat - 0.5 * (t1 + t2)
        } else {
            let t4 = 2.0 * p.delta / (t2 - t1);
            vdsat * (1.0 - t4)
        };
        if vds.v == 0.0 {
            // As in the reference: the value is 0, only its drain derivative remains.
            vdseff = D { v: 0.0, d: [vdseff.d[0], 0.0, 0.0] };
        }
        if vdseff.v > vds.v {
            vdseff = vds;
        }
        let diff_vds = vds - vdseff;

        // Velocity overshoot.
        if let Some(lam) = p.lambda {
            let t2 = lam / (leff * ueff);
            let t5 = 1.0 / (esat * p.litl);
            let t6 = 1.0 + diff_vds * t5;
            let t8 = 1.0 - 2.0 / (t6 * t6 + 1.0);
            esat_l = esat_l * (1.0 + t2 * t8);
            esat = esat_l / leff;
        }

        // Vasat.
        let tmp4 = 1.0 - 0.5 * abulk * vdsat / vgst2vtm;
        let t9 = wvcox_rds * vgsteff;
        let t0 = esat_l + vdsat + 2.0 * t9 * tmp4;
        let t1 = 2.0 / lambda - 1.0 + wvcox_rds * abulk;
        let vasat = t0 / t1;

        // Linear-region current with the charge centroid correction of Cox.
        let t0 = (vgsteff + p.vtfbphi2) / (2.0e8 * p.toxp);
        let tmp3 = (p.bdos * 0.7 * t0.ln()).exp();
        let tcen = p.ados * 1.9e-9 / (1.0 + tmp3);
        let coxeff = epssub * p.coxp / (epssub + p.coxp * tcen);
        let coxeff_wov_l = coxeff * weff / leff;
        let beta = ueff * coxeff_wov_l;
        let fgche1 = vgsteff * (1.0 - 0.5 * vdseff * abulk / vgst2vtm);
        let fgche2 = 1.0 + vdseff / esat_l;
        let gche = beta * fgche1 / fgche2;
        let idl = gche / (1.0 + gche * rds);

        // Output resistance.
        let fp = if p.fprout <= 0.0 { D::c(1.0) } else { 1.0 / (1.0 + p.fprout * leff.sqrt() / vgst2vtm) };
        let t9 = p.pvag / esat_l * vgsteff;
        let pvag_term = if t9.v > -0.9 { 1.0 + t9 } else { (0.8 + t9) / (17.0 + 20.0 * t9) };
        let (vaclm, cclm) = if p.pclm > MIN_EXP && diff_vds.v > 1.0e-10 {
            let t0 = 1.0 + rds * idl;
            let t1 = leff + vdsat / esat;
            let cclm = fp * pvag_term * t0 * t1 / (p.pclm * p.litl);
            (cclm * diff_vds, cclm)
        } else {
            (D::c(MAX_EXP), D::c(MAX_EXP))
        };
        let vadibl = if p.theta_rout > MIN_EXP {
            let t8 = abulk * vdsat;
            let t0 = vgst2vtm * t8;
            let t1 = vgst2vtm + t8;
            let v = (vgst2vtm - t0 / t1) / p.theta_rout;
            let t7 = p.pdiblb * vbseff;
            let v = if t7.v >= -0.9 { v / (1.0 + t7) } else { v * (17.0 + 20.0 * t7) / (0.8 + t7) };
            v * pvag_term
        } else {
            D::c(MAX_EXP)
        };
        let va = vasat + vaclm;
        let vadits = if p.pdits > MIN_EXP {
            let t0 = p.pditsd * vds;
            let t1 = if t0.v > EXP_THRESHOLD { D::c(MAX_EXP) } else { t0.exp() };
            let t2 = 1.0 + p.pditsl * leff;
            (1.0 + t2 * t1) / p.pdits * fp
        } else {
            D::c(MAX_EXP)
        };
        let vascbe = if p.pscbe2 > 0.0 && p.pscbe1 >= 0.0 {
            if diff_vds.v > p.pscbe1 * p.litl / EXP_THRESHOLD {
                leff * (p.pscbe1 * p.litl / diff_vds).exp() / p.pscbe2
            } else {
                D::c(MAX_EXP * leff / p.pscbe2)
            }
        } else {
            D::c(MAX_EXP)
        };
        let mut idsa = idl * (1.0 + diff_vds / vadibl);
        idsa = idsa * (1.0 + diff_vds / vadits);
        idsa = idsa * (1.0 + (va / vasat).ln() / cclm);
        // Substrate (impact ionisation) current.
        let isub = {
            let (alpha0, alpha1, beta0) = p.leak.as_ref().map_or((0.0, 0.0, 0.0), |l| (l.alpha0, l.alpha1, l.beta0));
            let tmp = alpha0 + alpha1 * leff;
            if tmp <= 0.0 || beta0 <= 0.0 {
                D::c(0.0)
            } else {
                let t2 = tmp / leff;
                let t1 = if diff_vds.v > beta0 / EXP_THRESHOLD {
                    t2 * diff_vds * (-beta0 / diff_vds).exp()
                } else {
                    t2 * MIN_EXP * diff_vds
                };
                t1 * idsa * vdseff
            }
        };
        let ids = idsa * (1.0 + diff_vds / vascbe);
        let mut cdrain = ids * vdseff;

        // Source-end velocity limit.
        if let Some(vtl) = p.vtl {
            let t11 = 1.0 / leff / coxeff_wov_l / vgsteff;
            let vs = cdrain * t11;
            let t0 = 6.0;
            let t1 = vs / (vtl * p.tfactor);
            if t1.v > 0.0 {
                let t2 = 1.0 + (t0 * t1.ln()).exp();
                let fsevl = 1.0 / (t2.ln() / t0).exp();
                cdrain = cdrain * fsevl;
            }
        }
        let internals = Internals {
            vth: vth.v,
            vdsat: vdsat.v,
            vgsteff: vgsteff.v,
            vds: vds.v,
            vdseff: vdseff.v,
            ueff: ueff.v,
            coxeff: coxeff.v,
            abulk: abulk_stored,
            abov_vgst2vtm: (abulk / vgst2vtm).v,
            abulk0_q,
            rds: rds.v,
            xdep: xdep.v,
        };
        (cdrain, internals, isub, vdseff)
    }
}

/// Bias-dependent overlap charge at gate voltage `v` over the source / drain (`capmod` 1, 2):
/// `cgo` the constant part, `cgxl` the bias-dependent part (both F, per finger).
fn overlap(v: D, cgo: f64, cgxl: f64, ckappa: f64) -> D {
    const DELTA_1: f64 = 0.02;
    let t0 = v + DELTA_1;
    let t1 = (t0 * t0 + 4.0 * DELTA_1).sqrt();
    let t2 = 0.5 * (t0 - t1);
    let t4 = (1.0 - 4.0 * t2 / ckappa).sqrt();
    (cgo + cgxl) * v - cgxl * (t2 + 0.5 * ckappa * (t4 - 1.0))
}

/// `exp` limited as by the BSIM4 `DEXP` macro.
fn dexp(x: D) -> D {
    if x.v > EXP_THRESHOLD {
        D { v: MAX_EXP * (1.0 + x.v - EXP_THRESHOLD), d: x.d.map(|d| d * MAX_EXP) }
    } else if x.v < -EXP_THRESHOLD {
        D::c(MIN_EXP)
    } else {
        x.exp()
    }
}

fn poly_depletion(phi: f64, ngate: f64, epsgate: f64, coxe: f64, vgs: D) -> D {
    if ngate > 1.0e18 && ngate < 1.0e25 && vgs.v > phi && epsgate != 0.0 {
        let t1 = 1.0e6 * CHARGE * epsgate * ngate / (coxe * coxe);
        let t8 = vgs - phi;
        let t4 = (1.0 + 2.0 * t8 / t1).sqrt();
        let t2 = 2.0 * t8 / (t4 + 1.0);
        let t3 = 0.5 * t2 * t2 / t1;
        let t7 = 1.12 - t3 - 0.05;
        let t6 = (t7 * t7 + 0.224).sqrt();
        let t5 = 1.12 - 0.5 * (t7 + t6);
        vgs - t5
    } else {
        vgs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(pmos: bool, p: &[(&str, f64)]) -> Card {
        Card::new(pmos, p.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    }

    fn nmos() -> Card {
        card(false, &[("toxe", 4e-9), ("vth0", 0.4), ("u0", 0.04), ("vsat", 1e5), ("ndep", 2e17)])
    }

    #[test]
    fn dual_numbers_differentiate() {
        let x = D::var(2.0, 0);
        let y = (x * x + 3.0 * x).sqrt().ln() / x;
        let f = |x: f64| (x * x + 3.0 * x).sqrt().ln() / x;
        let h = 1e-6;
        assert!((y.d[0] - (f(2.0 + h) - f(2.0 - h)) / (2.0 * h)).abs() < 1e-8);
        assert!((x.powf(2.5).d[0] - 2.5 * 2.0f64.powf(1.5)).abs() < 1e-12);
    }

    #[test]
    fn derivatives_match_finite_differences() {
        let inst = Inst::new(&nmos(), Geometry::new(1e-6, 0.1e-6, 1.0), 27.0).unwrap();
        let h = 1e-6;
        for (vds, vgs, vbs) in [(0.05, 0.8, 0.0), (1.0, 1.0, -0.3), (0.6, 0.2, 0.0), (1.2, 0.6, -0.5), (0.0, 1.0, 0.0)] {
            let e = inst.eval(vds, vgs, vbs);
            let f = |a: f64, b: f64, c: f64| inst.eval(a, b, c).ids.v;
            let num = [
                (f(vds + h, vgs, vbs) - f((vds - h).max(0.0), vgs, vbs)) / (vds + h - (vds - h).max(0.0)),
                (f(vds, vgs + h, vbs) - f(vds, vgs - h, vbs)) / (2.0 * h),
                (f(vds, vgs, vbs + h) - f(vds, vgs, vbs - h)) / (2.0 * h),
            ];
            for k in 0..3 {
                let tol = 1e-4 * num[k].abs().max(1e-9);
                assert!((e.ids.d[k] - num[k]).abs() < tol, "bias {vds} {vgs} {vbs} k={k}: {} vs {}", e.ids.d[k], num[k]);
            }
        }
    }

    #[test]
    fn current_behaves_like_a_transistor() {
        let inst = Inst::new(&nmos(), Geometry::new(1e-6, 0.1e-6, 1.0), 27.0).unwrap();
        let id = |vds, vgs| inst.eval(vds, vgs, 0.0).ids.v;
        // Off, subthreshold slope, on; saturation flattens; body bias raises Vth.
        assert!(id(1.0, 0.0) < 1e-8 && id(1.0, 1.0) > 1e-4, "{} {}", id(1.0, 0.0), id(1.0, 1.0));
        let swing = 0.1 / (id(1.0, 0.25) / id(1.0, 0.15)).log10();
        assert!((0.06..0.12).contains(&swing), "subthreshold swing {swing} V/dec");
        let gds = |vds| inst.eval(vds, 1.0, 0.0).ids.d[0];
        assert!(gds(1.0) < gds(0.05) / 3.0, "saturation: gds {} vs linear {}", gds(1.0), gds(0.05));
        assert!(inst.eval(1.0, 1.0, -0.5).vth > inst.eval(1.0, 1.0, 0.0).vth);
        // Two fingers carry twice the current of one finger of half the width.
        let one = Inst::new(&nmos(), Geometry::new(0.5e-6, 0.1e-6, 1.0), 27.0).unwrap();
        let two = Inst::new(&nmos(), Geometry::new(1e-6, 0.1e-6, 2.0), 27.0).unwrap();
        assert!((two.eval(1.0, 1.0, 0.0).ids.v / one.eval(1.0, 1.0, 0.0).ids.v - 2.0).abs() < 1e-12);
        // Hotter: lower mobility, less on-current.
        let hot = Inst::new(&nmos(), Geometry::new(1e-6, 0.1e-6, 1.0), 125.0).unwrap();
        assert!(hot.eval(1.0, 1.0, 0.0).ids.v < id(1.0, 1.0));
    }

    #[test]
    fn delvto_and_mulu0_shift_the_instance() {
        // delvto moves vth0 and vfb together: the same device as a card with vth0 + delvto.
        let shifted =
            Inst::with_shift(&nmos(), Geometry::new(1e-6, 0.1e-6, 1.0), 27.0, Shift { delvto: 0.03, mulu0: 1.0 }).unwrap();
        let mut moved = nmos();
        moved.p.insert("vth0".into(), 0.43);
        let card = Inst::new(&moved, Geometry::new(1e-6, 0.1e-6, 1.0), 27.0).unwrap();
        for (vds, vgs, vbs) in [(0.05, 0.8, 0.0), (1.0, 1.0, -0.3), (0.6, 0.3, 0.0)] {
            let (a, b) = (shifted.eval(vds, vgs, vbs).ids.v, card.eval(vds, vgs, vbs).ids.v);
            assert!((a / b - 1.0).abs() < 1e-9, "bias {vds} {vgs} {vbs}: {a} vs {b}");
        }
        let base = Inst::new(&nmos(), Geometry::new(1e-6, 0.1e-6, 1.0), 27.0).unwrap();
        assert!(shifted.eval(1.0, 1.0, 0.0).ids.v < base.eval(1.0, 1.0, 0.0).ids.v, "a higher Vth carries less current");
        let fast = Inst::with_shift(&nmos(), Geometry::new(1e-6, 0.1e-6, 1.0), 27.0, Shift { delvto: 0.0, mulu0: 1.2 }).unwrap();
        assert!(fast.eval(0.05, 1.0, 0.0).ids.v > 1.1 * base.eval(0.05, 1.0, 0.0).ids.v, "mulu0 raises the mobility");
    }

    #[test]
    fn binning_and_errors() {
        let mut c = nmos();
        c.p.insert("lvth0".into(), 0.01);
        let a = Inst::new(&c, Geometry::new(1e-6, 1e-6, 1.0), 27.0).unwrap();
        let b = Inst::new(&c, Geometry::new(1e-6, 0.1e-6, 1.0), 27.0).unwrap();
        assert!((a.vth0 - 0.41).abs() < 1e-12 && (b.vth0 - 0.5).abs() < 1e-12, "{} {}", a.vth0, b.vth0);
        c.p.insert("lint".into(), 0.1e-6);
        assert!(Inst::new(&c, Geometry::new(1e-6, 0.1e-6, 1.0), 27.0).unwrap_err().contains("length"));
        let w = card(false, &[("rdsmod", 1.0), ("igcmod", 1.0), ("mobmod", 3.0), ("version", 4.5)]).warnings();
        assert_eq!(w.len(), 3, "{w:?}");
    }
}
