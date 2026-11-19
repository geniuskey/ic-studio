//! Built-in Modified Nodal Analysis engine: OP, DC sweep, AC, noise and transient.
//!
//! Devices: R C L V I, diodes, MOS (level 1 and BSIM4), Gummel-Poon BJTs and linear
//! controlled sources, on a dense solver for small circuits and a sparse one for large ones
//! ([`crate::linalg`]). Decks needing other models (BSIM-CMG, PSP, behavioural sources) go to
//! ngspice through the Auto backend.

use num_complex::Complex64;

use ics_core::Analysis;
use ics_core::units::parse_spice;

use crate::SimError;
use crate::circuit::{BjtModel, Circuit, CtrlKind, DiodeModel, Element, MosModel, Node, Ref};
use crate::linalg::{Matrix, solve};
use crate::results::{Dataset, DeviceOp, VecKind, Vector};

const BOLTZMANN: f64 = 1.380649e-23;
const CHARGE: f64 = 1.602176634e-19;

#[derive(Clone)]
enum CapKind {
    Linear(f64),
    /// Depletion charge of up to two grading terms `(cj0, m)` (area and sidewall), plus the
    /// diffusion charge `tt · id` of a diode.
    Junction {
        terms: [(f64, f64); 2],
        pb: f64,
        fc: f64,
        diode: Option<DiodeModel>,
    },
    /// Meyer gate capacitance (part 0: g-s, 1: g-d, 2: g-b) of MOS element `elem`. It depends on
    /// all terminal voltages; over a time step its charge changes by the average of the step-start
    /// and step-end capacitance times the voltage change (as in SPICE).
    Meyer {
        elem: usize,
        part: usize,
    },
}

/// Charge storage between `a` and `b`; `v = v(a) - v(b)` (forward bias for junctions).
#[derive(Clone)]
struct Cap {
    a: Node,
    b: Node,
    kind: CapKind,
}

/// Depletion charge and capacitance of a junction at forward bias `v`, linearised above
/// `fc · pb` as in SPICE.
fn depletion(cj0: f64, m: f64, pb: f64, fc: f64, v: f64) -> (f64, f64) {
    if cj0 <= 0.0 {
        return (0.0, 0.0);
    }
    let m = m.min(0.99);
    let vfc = fc * pb;
    if v < vfc {
        let arg = 1.0 - v / pb;
        let s = arg.powf(-m);
        (pb * cj0 * (1.0 - arg * s) / (1.0 - m), cj0 * s)
    } else {
        let f1 = pb * (1.0 - (1.0 - fc).powf(1.0 - m)) / (1.0 - m);
        let f2 = (1.0 - fc).powf(1.0 + m);
        let f3 = 1.0 - fc * (1.0 + m);
        let q = cj0 * (f1 + (f3 * (v - vfc) + m / (2.0 * pb) * (v * v - vfc * vfc)) / f2);
        (q, cj0 / f2 * (f3 + m * v / pb))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Integ {
    BackwardEuler,
    Trapezoidal,
}

/// Accepted state of the reactive elements at the last time point.
#[derive(Clone)]
struct TranState {
    cap_q: Vec<f64>,
    cap_i: Vec<f64>,
    cap_v: Vec<f64>,
    /// Meyer capacitance for the next step (unused for other kinds).
    cap_c: Vec<f64>,
    /// Meyer capacitance predicted for the end of the step being taken.
    cap_c_end: Vec<f64>,
    ind_v: Vec<f64>,
    ind_i: Vec<f64>,
    /// Terminal charges and currents of the charge-based devices.
    qdev_q: Vec<[f64; 4]>,
    qdev_i: Vec<[f64; 4]>,
}

/// An accepted transient point: time, solution and reactive values (see `Engine::reactive`).
type Point = (f64, Vec<f64>, Vec<(f64, f64)>);

/// A linear combination of unknowns: (column, coefficient).
type Lin = Vec<(usize, f64)>;

fn lin_value(x: &[f64], l: &Lin) -> f64 {
    l.iter().map(|&(c, s)| s * x[c]).sum()
}

/// Consecutive sign flips of a branch current's second difference taken as trapezoidal ringing.
const RING_FLIPS: usize = 8;

#[derive(Clone, Copy)]
enum Mode<'s> {
    /// `force`: nodes held at a voltage through a large conductance (`.ic`, `.nodeset`).
    Dc {
        src_scale: f64,
        force: &'s [(Node, f64)],
    },
    Tran {
        t: f64,
        h: f64,
        method: Integ,
        state: &'s TranState,
    },
}

pub struct Engine {
    c: Circuit,
    nn: usize,
    /// Branch row for each element (V sources, inductors, E and H sources).
    branch: Vec<Option<usize>>,
    /// For F / H sources: the branch row of their controlling voltage source.
    ctrl: Vec<Option<usize>>,
    size: usize,
    caps: Vec<Cap>,
    inductors: Vec<usize>,
    /// For B sources: each reference as a signed sum of unknowns (`v(a, b)`: two node columns,
    /// `i(vname)`: one branch column), by element.
    lin: Vec<Vec<Lin>>,
    /// Mutual inductances: the two inductors' branch rows, their positions in `inductors`, and M.
    mutual: Vec<(usize, usize, usize, usize, f64)>,
    /// Charge-based MOS devices (BSIM4): element index and (d, g, s, b) nodes.
    qdevs: Vec<(usize, [Node; 4])>,
    vt: f64,
    pub log: Vec<String>,
    /// Set for the first Newton iteration of a cold operating point: junctions are linearised
    /// at their critical voltage instead of at zero (SPICE's MODEINITJCT), so that circuits with
    /// a zero-current solution (a bandgap) start towards the working one.
    init_jct: std::cell::Cell<bool>,
    /// Junction voltages (vbe, vbc / vd) each BJT and diode was last evaluated at, by element:
    /// the next iteration limits their change logarithmically (SPICE's pnjlim).
    jct: std::cell::RefCell<std::collections::HashMap<usize, [f64; 2]>>,
    /// Switch states (closed), by element: they change at the accepted points of an analysis.
    sw_on: std::cell::RefCell<Vec<bool>>,
    /// Set by `assemble` when a junction voltage was limited: the iteration has not converged.
    limited: std::cell::Cell<bool>,
    /// Newton steps of node voltages are clipped to 0.5 V when the circuit has MOS devices
    /// (they have no junction limiting); BJTs and diodes are limited by pnjlim alone, as in
    /// SPICE, which keeps circuits with several solutions on SPICE's path.
    damp: bool,
}

/// SPICE's `DEVpnjlim`: the junction voltage to evaluate at, given the raw new value, the last
/// one, `nvt` and the critical voltage.
fn pnjlim(vnew: f64, vold: f64, nvt: f64, vcrit: f64) -> (f64, bool) {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * nvt {
        let v = if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / nvt;
            if arg > 0.0 { vold + nvt * arg.ln() } else { vcrit }
        } else {
            nvt * (vnew / nvt).ln()
        };
        (v, true)
    } else {
        (vnew, false)
    }
}

fn vcrit(is: f64, nvt: f64) -> f64 {
    nvt * (nvt / (std::f64::consts::SQRT_2 * is.max(1e-300))).ln()
}

struct MosEval {
    id: f64,
    /// dI/d(vd, vg, vs, vb)
    j: [f64; 4],
    region: &'static str,
    vth: f64,
    gm: f64,
    gds: f64,
}

/// Terminal voltages in the normalized frame: NMOS polarity, drain the higher of d / s.
struct Frame {
    swapped: bool,
    vgs: f64,
    vds: f64,
    vth: f64,
    dvth_dvbs: f64,
}

fn frame(m: &MosModel, vd: f64, vg: f64, vs: f64, vb: f64) -> Frame {
    let sign = if m.pmos { -1.0 } else { 1.0 };
    let (mut d, g, mut s, b) = (sign * vd, sign * vg, sign * vs, sign * vb);
    let swapped = d < s;
    if swapped {
        std::mem::swap(&mut d, &mut s);
    }
    let vto = sign * m.vto; // normalized threshold (positive for enhancement)
    let sq = (m.phi - (b - s)).max(1e-3).sqrt();
    Frame { swapped, vgs: g - s, vds: d - s, vth: vto + m.gamma * (sq - m.phi.sqrt()), dvth_dvbs: -m.gamma / (2.0 * sq) }
}

/// Meyer intrinsic gate capacitances `[cgs, cgd, cgb]` (level 1: `vdsat = vgs - vth`).
fn meyer(m: &MosModel, w: f64, l: f64, mult: f64, vd: f64, vg: f64, vs: f64, vb: f64) -> [f64; 3] {
    let cox = m.cox() * w * m.leff(l) * mult;
    if cox == 0.0 {
        return [0.0; 3];
    }
    let f = frame(m, vd, vg, vs, vb);
    let vgst = f.vgs - f.vth;
    let phi = m.phi.max(1e-3);
    let k = 2.0 * cox / 3.0;
    let (cgs, cgd, cgb) = if vgst <= -phi {
        (0.0, 0.0, cox)
    } else if vgst <= -phi / 2.0 {
        (0.0, 0.0, -vgst * cox / phi)
    } else if vgst <= 0.0 {
        (k + vgst * cox / (0.75 * phi), 0.0, -vgst * cox / phi)
    } else if f.vds >= vgst {
        (k, 0.0, 0.0)
    } else {
        let vdd = 2.0 * vgst - f.vds;
        (k * (1.0 - ((vgst - f.vds) / vdd).powi(2)), k * (1.0 - (vgst / vdd).powi(2)), 0.0)
    };
    if f.swapped { [cgd, cgs, cgb] } else { [cgs, cgd, cgb] }
}

/// Exponential junction current `is (e^(v/nvt) - 1)` and its conductance, linear above 40 nvt.
fn exp_iv(is: f64, nvt: f64, v: f64) -> (f64, f64) {
    const MAXARG: f64 = 40.0;
    let arg = v / nvt;
    if arg > MAXARG {
        let e = MAXARG.exp();
        (is * (e * (1.0 + arg - MAXARG) - 1.0), is * e / nvt)
    } else {
        let e = arg.exp();
        (is * (e - 1.0), is * e / nvt)
    }
}

/// `is · (e^(v / nvt) − 1)` as a dual number, continued linearly above 40 nvt (as `exp_iv`).
fn exp_d(is: f64, nvt: f64, v: crate::bsim4::D) -> crate::bsim4::D {
    use crate::bsim4::D;
    const MAXARG: f64 = 40.0;
    let arg = v / nvt;
    if arg.v > MAXARG {
        let e = MAXARG.exp();
        is * (e * (arg + (1.0 - MAXARG)) - 1.0)
    } else {
        is * (arg.exp() - D::c(1.0))
    }
}

/// Gummel-Poon bipolar currents in the NPN frame: collector and base currents (into the device)
/// with their derivatives by (vbe, vbc).
pub(crate) struct BjtEval {
    pub ic: f64,
    pub ib: f64,
    pub dic: [f64; 2],
    pub dib: [f64; 2],
}

pub(crate) fn bjt_eval(m: &BjtModel, area: f64, vbe: f64, vbc: f64, vt: f64) -> BjtEval {
    use crate::bsim4::D;
    let (be, bc) = (D::var(vbe, 0), D::var(vbc, 1));
    let is = m.is * area;
    let f = exp_d(is, m.nf * vt, be);
    let r = exp_d(is, m.nr * vt, bc);
    // Base charge: Early effect (q1) and high injection (q2).
    let inv = |v: f64| if v.is_finite() && v > 0.0 { 1.0 / v } else { 0.0 };
    let q1 = D::c(1.0) / (D::c(1.0) - bc * inv(m.vaf) - be * inv(m.var));
    let q2 = f * inv(m.ikf * area) + r * inv(m.ikr * area);
    let qb = q1 * 0.5 * (D::c(1.0) + (D::c(1.0) + 4.0 * q2).sqrt());
    let ict = (f - r) / qb;
    let ibe = f / m.bf + exp_d(m.ise * area, m.ne * vt, be);
    let ibc = r / m.br + exp_d(m.isc * area, m.nc * vt, bc);
    let ic = ict - ibc;
    let ib = ibe + ibc;
    BjtEval { ic: ic.v, ib: ib.v, dic: [ic.d[0], ic.d[1]], dib: [ib.d[0], ib.d[1]] }
}

impl Engine {
    /// The linearised currents of bipolar `model` at `x`: (node, current into the device,
    /// d current / d node voltage for (c, b, e)) for its collector, base and emitter.
    fn bjt_stamp(&self, model: &BjtModel, area: f64, nodes: [Node; 3], x: &[f64]) -> [(Node, f64, [f64; 3]); 3] {
        let p = if model.pnp { -1.0 } else { 1.0 };
        let [c, b, e] = nodes;
        let (vc, vb, ve) = (self.v(x, c), self.v(x, b), self.v(x, e));
        let ev = bjt_eval(model, area, p * (vb - ve), p * (vb - vc), self.vt);
        // d/d(vc, vb, ve) of a current p·I(vbe, vbc) with vbe = p(vb - ve), vbc = p(vb - vc).
        let row = |i: f64, d: [f64; 2]| (p * i, [-d[1], d[0] + d[1], -d[0]]);
        let (ic, gc) = row(ev.ic, ev.dic);
        let (ib, gb) = row(ev.ib, ev.dib);
        let ge = [-(gc[0] + gb[0]), -(gc[1] + gb[1]), -(gc[2] + gb[2])];
        [(c, ic, gc), (b, ib, gb), (e, -(ic + ib), ge)]
    }
}

fn mos_eval(m: &MosModel, w: f64, l: f64, mult: f64, vd: f64, vg: f64, vs: f64, vb: f64) -> MosEval {
    if let Some(inst) = m.bsim4.as_ref().and_then(|b| b.inst.as_deref()) {
        return bsim4_eval(m.pmos, inst, mult, vd, vg, vs, vb);
    }
    let sign = if m.pmos { -1.0 } else { 1.0 };
    let Frame { swapped, vgs, vds, vth, dvth_dvbs } = frame(m, vd, vg, vs, vb);
    let beta = m.kp * w / m.leff(l) * mult;
    let vov = vgs - vth;
    let (id, gm, gds, region) = if vov <= 0.0 {
        (0.0, 0.0, 0.0, "cutoff")
    } else if vds < vov {
        let f = 1.0 + m.lambda * vds;
        let id0 = beta * (vov * vds - 0.5 * vds * vds);
        (id0 * f, beta * vds * f, beta * (vov - vds) * f + id0 * m.lambda, "triode")
    } else {
        let f = 1.0 + m.lambda * vds;
        let id0 = 0.5 * beta * vov * vov;
        (id0 * f, beta * vov * f, id0 * m.lambda, "saturation")
    };
    let gmb = -gm * dvth_dvbs;
    // partials in normalized, possibly swapped, frame: (eff_d, g, eff_s, b)
    let jn = [gds, gm, -(gm + gds + gmb), gmb];
    let (i, j) = if swapped { (-id, [-jn[2], -jn[1], -jn[0], -jn[3]]) } else { (id, jn) };
    MosEval { id: sign * i, j, region, vth: sign * vth, gm, gds }
}

/// Intrinsic terminal charges (d, g, s, b) of a level-1 MOS with `capmod=1`: the long-channel
/// inversion charge split by Ward-Dutton (40 / 60 in saturation, half each at Vds = 0), the
/// gate-bulk charge below threshold integrated from the Meyer gate-bulk capacitance, and the
/// gate charge their negative sum, so that charge is conserved. The overdrive at each end goes
/// through a softplus of 50 mV, smoothing the onset of inversion.
fn level1_charges(m: &MosModel, w: f64, l: f64, mult: f64, v: [f64; 4]) -> [f64; 4] {
    let cox = m.cox() * w * m.leff(l) * mult;
    if cox == 0.0 {
        return [0.0; 4];
    }
    let f = frame(m, v[0], v[1], v[2], v[3]);
    const ETA: f64 = 0.05;
    let softplus = |x: f64| if x > 20.0 * ETA { x } else { ETA * (x / ETA).exp().ln_1p() };
    let u = f.vgs - f.vth;
    let (a, b) = (softplus(u), softplus(u - f.vds));
    let sum = (a + b).max(1e-30);
    let qi = -2.0 / 3.0 * cox * (a * a + a * b + b * b) / sum;
    let qd = -2.0 / 15.0 * cox * (2.0 * a.powi(3) + 4.0 * a * a * b + 6.0 * a * b * b + 3.0 * b.powi(3)) / (sum * sum);
    let qs = qi - qd;
    let phi = m.phi.max(1e-3);
    let qgb = if u <= -phi {
        cox * (u + phi) - cox * phi / 4.0
    } else if u <= -phi / 2.0 {
        -cox * (u + phi / 2.0).powi(2) / phi
    } else {
        0.0
    };
    let (qg, qb) = (qgb - qi, -qgb);
    let (qd, qs) = if f.swapped { (qs, qd) } else { (qd, qs) };
    let sign = if m.pmos { -1.0 } else { 1.0 };
    [qd, qg, qs, qb].map(|q| sign * q)
}

/// Terminal charges (d, g, s, b) of a charge-based MOS (BSIM4, or level 1 with `capmod=1`) at
/// terminal voltages `v` (d, g, s, b), times the multiplier `mult`, and their derivatives with
/// respect to `v`; None for other models.
fn mos_charges(model: &MosModel, w: f64, l: f64, mult: f64, v: [f64; 4]) -> Option<([f64; 4], [[f64; 4]; 4])> {
    if model.bsim4.is_none() {
        if model.capmod != 1 || model.cox() == 0.0 {
            return None;
        }
        let q = level1_charges(model, w, l, mult, v);
        let mut c = [[0.0; 4]; 4];
        const H: f64 = 1e-6;
        for j in 0..4 {
            let (mut up, mut dn) = (v, v);
            up[j] += H;
            dn[j] -= H;
            let (qu, qd) = (level1_charges(model, w, l, mult, up), level1_charges(model, w, l, mult, dn));
            for t in 0..4 {
                c[t][j] = (qu[t] - qd[t]) / (2.0 * H);
            }
        }
        return Some((q, c));
    }
    let inst = model.bsim4.as_ref()?.inst.as_deref()?;
    let sign = if model.pmos { -1.0 } else { 1.0 };
    let q = inst.charges(sign * (v[0] - v[2]), sign * (v[1] - v[2]), sign * (v[3] - v[2]));
    let mut qs = [0.0; 4];
    let mut c = [[0.0; 4]; 4];
    for t in 0..4 {
        qs[t] = sign * q[t].v * mult;
        // d/d(vd, vg, vb) from (Vds, Vgs, Vbs); the source takes the rest. The polarity signs
        // of charge and voltage cancel.
        let [dd, dg, db] = q[t].d.map(|x| x * mult);
        c[t] = [dd, dg, -(dd + dg + db), db];
    }
    Some((qs, c))
}

/// Leakage currents of a BSIM4 MOS at terminal voltages `v` (d, g, s, b), times `mult`: the
/// currents into each terminal, their derivatives with respect to `v`, and the gate currents to
/// source, drain and bulk; None without leakage.
/// Terminal currents (d, g, s, b), their Jacobian over (vd, vg, vs, vb), and the gate
/// currents to source, drain and bulk.
type LeakageStamp = ([f64; 4], [[f64; 4]; 4], [f64; 3]);

fn mos_leakage(model: &MosModel, mult: f64, v: [f64; 4]) -> Option<LeakageStamp> {
    let inst = model.bsim4.as_ref()?.inst.as_deref()?;
    if !inst.has_leakage() {
        return None;
    }
    let sign = if model.pmos { -1.0 } else { 1.0 };
    let lk = inst.leakage(sign * (v[0] - v[2]), sign * (v[1] - v[2]), sign * (v[3] - v[2]))?;
    let mut i = [0.0; 4];
    let mut g = [[0.0; 4]; 4];
    for t in 0..4 {
        let q = lk.terminal[t];
        i[t] = sign * q.v * mult;
        let [dd, dg, db] = q.d.map(|x| x * mult);
        g[t] = [dd, dg, -(dd + dg + db), db];
    }
    Some((i, g, [lk.gate_s * mult, lk.gate_d * mult, lk.gate_b * mult]))
}

fn bsim4_eval(pmos: bool, inst: &crate::bsim4::Inst, mult: f64, vd: f64, vg: f64, vs: f64, vb: f64) -> MosEval {
    let sign = if pmos { -1.0 } else { 1.0 };
    let (mut d, g, mut s, b) = (sign * vd, sign * vg, sign * vs, sign * vb);
    let swapped = d < s;
    if swapped {
        std::mem::swap(&mut d, &mut s);
    }
    let (vds, vgs) = (d - s, g - s);
    let e = inst.eval(vds, vgs, b - s);
    let [gds, gm, gmb] = e.ids.d.map(|x| x * mult);
    let id = e.ids.v * mult;
    let region = if vgs < e.vth {
        "subthreshold"
    } else if vds < e.vdsat {
        "triode"
    } else {
        "saturation"
    };
    let jn = [gds, gm, -(gm + gds + gmb), gmb];
    let (i, j) = if swapped { (-id, [-jn[2], -jn[1], -jn[0], -jn[3]]) } else { (id, jn) };
    MosEval { id: sign * i, j, region, vth: sign * e.vth, gm, gds }
}

fn diode_eval(m: &DiodeModel, vd: f64, vt: f64) -> (f64, f64) {
    let nvt = m.n * vt;
    let (mut id, mut gd) = exp_iv(m.is, nvt, vd);
    if m.bv.is_finite() {
        // Reverse breakdown: -ibv · e^(-(vd + bv) / nvt), so the current at -bv is -ibv.
        let (ib, gb) = exp_iv(m.ibv, nvt, -(vd + m.bv));
        id -= ib + m.ibv;
        gd += gb;
    }
    (id, gd)
}

/// Bulk junction diodes of a MOS: (anode, cathode, saturation current) for the drain and source
/// sides that are not tied to the bulk.
fn bulk_diodes(model: &MosModel, d: Node, s: Node, b: Node, m: f64, j: &crate::circuit::Junctions) -> Vec<(Node, Node, f64)> {
    [(d, j.ad), (s, j.as_)]
        .into_iter()
        .filter(|(t, _)| *t != b)
        .map(|(t, area)| {
            let is = if model.js > 0.0 && area > 0.0 { model.js * area } else { model.is } * m;
            if model.pmos { (t, b, is) } else { (b, t, is) }
        })
        .collect()
}

/// Stamp a linear controlled source: `nodes` are (+, −, control +, control −), `br` its own
/// branch row (E, H) and `ctl` the controlling source's branch (F, H).
#[allow(clippy::too_many_arguments)]
fn stamp_ctrl<T: crate::linalg::Scalar>(
    m: &mut Matrix<T>,
    kind: CtrlKind,
    nodes: [Node; 4],
    br: Option<usize>,
    ctl: Option<usize>,
    gain: f64,
    one: T,
    of: impl Fn(f64) -> T,
) {
    let idx = |n: Node| if n == 0 { None } else { Some(n - 1) };
    let [a, b, cp, cn] = nodes;
    let neg = |v: T| T::zero() - v;
    match kind {
        CtrlKind::E | CtrlKind::H => {
            let br = br.expect("E / H have a branch");
            for (n, s) in [(a, one), (b, neg(one))] {
                if let Some(i) = idx(n) {
                    m.add(i, br, s);
                    m.add(br, i, s);
                }
            }
            if kind == CtrlKind::E {
                for (n, s) in [(cp, neg(of(gain))), (cn, of(gain))] {
                    if let Some(j) = idx(n) {
                        m.add(br, j, s);
                    }
                }
            } else if let Some(c) = ctl {
                m.add(br, c, neg(of(gain)));
            }
        }
        CtrlKind::G => {
            for (r, rs) in [(a, one), (b, neg(one))] {
                let Some(i) = idx(r) else { continue };
                for (c, cs) in [(cp, of(gain)), (cn, neg(of(gain)))] {
                    if let Some(j) = idx(c) {
                        m.add(i, j, rs * cs);
                    }
                }
            }
        }
        CtrlKind::F => {
            if let Some(c) = ctl {
                for (r, rs) in [(a, of(gain)), (b, neg(of(gain)))] {
                    if let Some(i) = idx(r) {
                        m.add(i, c, rs);
                    }
                }
            }
        }
    }
}

impl Engine {
    pub fn new(mut c: Circuit) -> Result<Self, SimError> {
        let temp = c.temp;
        for e in &mut c.elements {
            match e {
                Element::M { name, model, w, l, junctions: j, .. } => {
                    *model = model.at_temp(temp);
                    if let Some(b) = &mut model.bsim4 {
                        let geo = crate::bsim4::Geometry { w: *w, l: *l, nf: b.nf, ad: j.ad, as_: j.as_, pd: j.pd, ps: j.ps };
                        let inst = crate::bsim4::Inst::with_shift(&b.card, geo, temp, b.shift)
                            .map_err(|e| SimError::Engine(format!("{name}: {e}")))?;
                        b.inst = Some(std::sync::Arc::new(inst));
                    }
                }
                Element::D { model, .. } => *model = model.at_temp(temp),
                Element::Q { model, .. } => *model = model.at_temp(temp),
                _ => {}
            }
        }
        let nn = c.node_count() - 1;
        let mut branch = vec![None; c.elements.len()];
        let mut size = nn;
        let mut caps = vec![];
        let mut inductors = vec![];
        let mut qdevs = vec![];
        for (i, e) in c.elements.iter().enumerate() {
            match e {
                Element::V { .. } => {
                    branch[i] = Some(size);
                    size += 1;
                }
                Element::L { .. } => {
                    branch[i] = Some(size);
                    size += 1;
                    inductors.push(i);
                }
                Element::Ctrl { kind: CtrlKind::E | CtrlKind::H, .. } | Element::B { voltage: true, .. } => {
                    branch[i] = Some(size);
                    size += 1;
                }
                Element::C { a, b, c, .. } => caps.push(Cap { a: *a, b: *b, kind: CapKind::Linear(*c) }),
                Element::Q { c: cn, b, e, model, area, .. } => {
                    // Base-emitter and base-collector junctions: depletion plus diffusion charge
                    // (tf · If, tr · Ir), as the diode's.
                    let sign = |a: Node, k: Node| if model.pnp { (k, a) } else { (a, k) };
                    for (a, k, cj, vj, mj, tt, n) in [
                        (*b, *e, model.cje, model.vje, model.mje, model.tf, model.nf),
                        (*b, *cn, model.cjc, model.vjc, model.mjc, model.tr, model.nr),
                    ] {
                        if cj > 0.0 || tt > 0.0 {
                            let (a, k) = sign(a, k);
                            let d = DiodeModel {
                                is: model.is * area,
                                n,
                                cjo: cj * area,
                                vj,
                                m: mj,
                                fc: model.fc,
                                tt,
                                rs: 0.0,
                                bv: f64::INFINITY,
                                ibv: 0.0,
                                kf: 0.0,
                                af: 1.0,
                                eg: 1.11,
                                xti: 3.0,
                                tnom: 27.0,
                            };
                            caps.push(Cap {
                                a,
                                b: k,
                                kind: CapKind::Junction {
                                    terms: [(d.cjo, mj), (0.0, 0.5)],
                                    pb: vj,
                                    fc: model.fc,
                                    diode: (tt > 0.0).then_some(d),
                                },
                            });
                        }
                    }
                }
                Element::D { a, k, model, .. } if model.cjo > 0.0 || model.tt > 0.0 => caps.push(Cap {
                    a: *a,
                    b: *k,
                    kind: CapKind::Junction {
                        terms: [(model.cjo, model.m), (0.0, 0.5)],
                        pb: model.vj,
                        fc: model.fc,
                        diode: (model.tt > 0.0).then(|| model.clone()),
                    },
                }),
                // BSIM4: gate, overlap and junction charges come from the model.
                Element::M { d, g, s, b, model, .. } if model.bsim4.is_some() => qdevs.push((i, [*d, *g, *s, *b])),
                Element::M { d, g, s, b, model, w, l, m, junctions: j, .. } => {
                    for (t, area, perim) in [(*d, j.ad, j.pd), (*s, j.as_, j.ps)] {
                        let terms = [(model.cj * area * m, model.mj), (model.cjsw * perim * m, model.mjsw)];
                        if terms[0].0 + terms[1].0 > 0.0 && t != *b {
                            let (a, k) = if model.pmos { (t, *b) } else { (*b, t) };
                            let kind = CapKind::Junction { terms, pb: model.pb, fc: model.fc, diode: None };
                            caps.push(Cap { a, b: k, kind });
                        }
                    }
                    for (t, c) in [(*s, model.cgso * w), (*d, model.cgdo * w), (*b, model.cgbo * model.leff(*l))] {
                        if c > 0.0 {
                            caps.push(Cap { a: *g, b: t, kind: CapKind::Linear(c * m) });
                        }
                    }
                    if model.cox() > 0.0 && model.capmod == 1 {
                        // Charge-conserving intrinsic charges; junctions and overlaps stay caps.
                        qdevs.push((i, [*d, *g, *s, *b]));
                    } else if model.cox() > 0.0 {
                        for (part, t) in [*s, *d, *b].into_iter().enumerate() {
                            caps.push(Cap { a: *g, b: t, kind: CapKind::Meyer { elem: i, part } });
                        }
                    }
                }
                _ => {}
            }
        }
        if nn == 0 {
            return Err(SimError::Engine("circuit has no nodes besides ground".into()));
        }
        let vt = BOLTZMANN * (c.temp + 273.15) / CHARGE;
        let mut ctrl = vec![None; c.elements.len()];
        for (i, e) in c.elements.iter().enumerate() {
            if let Element::Ctrl { name, kind: CtrlKind::F | CtrlKind::H, vctl, .. } = e {
                let k = c
                    .elements
                    .iter()
                    .position(|v| matches!(v, Element::V { .. }) && v.name().eq_ignore_ascii_case(vctl))
                    .ok_or_else(|| SimError::Engine(format!("{name}: no voltage source '{vctl}' to sense the current of")))?;
                ctrl[i] = branch[k];
            }
            if let Element::S { name, vctl, .. } = e
                && !vctl.is_empty()
            {
                let k = c
                    .elements
                    .iter()
                    .position(|v| matches!(v, Element::V { .. }) && v.name().eq_ignore_ascii_case(vctl))
                    .ok_or_else(|| SimError::Engine(format!("{name}: no voltage source '{vctl}' to sense the current of")))?;
                ctrl[i] = branch[k];
            }
        }
        // A branch current by element name, for i(...) in B sources.
        let branch_of = |n: &str| c.elements.iter().position(|e| e.name().eq_ignore_ascii_case(n)).and_then(|k| branch[k]);
        let mut lin = vec![vec![]; c.elements.len()];
        let mut mutual = vec![];
        for (i, e) in c.elements.iter().enumerate() {
            match e {
                Element::B { name, refs, .. } => {
                    for r in refs {
                        lin[i].push(match r {
                            Ref::V(p, n) => {
                                [(*p, 1.0), (*n, -1.0)].into_iter().filter(|q| q.0 != 0).map(|(n, s)| (n - 1, s)).collect()
                            }
                            Ref::I(v) => vec![(
                                branch_of(v)
                                    .ok_or_else(|| SimError::Engine(format!("{name}: no source or inductor '{v}' for i({v})")))?,
                                1.0,
                            )],
                        });
                    }
                }
                Element::K { name, l1, l2, k } => {
                    let find = |n: &str| {
                        inductors
                            .iter()
                            .position(|&q| c.elements[q].name().eq_ignore_ascii_case(n))
                            .ok_or_else(|| SimError::Engine(format!("{name}: no inductor '{n}'")))
                    };
                    let (p1, p2) = (find(l1)?, find(l2)?);
                    if k.is_nan() || k.abs() > 1.0 {
                        return Err(SimError::Engine(format!("{name}: coupling {k} must lie within [-1, 1]")));
                    }
                    let l = |p: usize| match &c.elements[inductors[p]] {
                        Element::L { l, .. } => *l,
                        _ => unreachable!(),
                    };
                    let m = k * (l(p1) * l(p2)).abs().sqrt();
                    mutual.push((branch[inductors[p1]].unwrap(), branch[inductors[p2]].unwrap(), p1, p2, m));
                }
                _ => {}
            }
        }
        Ok(Self {
            nn,
            branch,
            ctrl,
            size,
            caps,
            inductors,
            lin,
            mutual,
            qdevs,
            vt,
            log: vec![],
            init_jct: std::cell::Cell::new(false),
            jct: Default::default(),
            limited: std::cell::Cell::new(false),
            sw_on: std::cell::RefCell::new(c.elements.iter().map(|e| matches!(e, Element::S { on: true, .. })).collect()),
            damp: c.elements.iter().any(|e| matches!(e, Element::M { .. })),
            c,
        })
    }

    /// The control value of switch `ei` at `x`: its control voltage, or (W) the sensed current.
    fn switch_ctl(&self, ei: usize, x: &[f64]) -> f64 {
        match (&self.c.elements[ei], self.ctrl[ei]) {
            (_, Some(br)) => x[br],
            (Element::S { cp, cn, .. }, None) => self.v(x, *cp) - self.v(x, *cn),
            _ => 0.0,
        }
    }

    /// The step switches allow after the step from `x0` to `x` of `h`, as ngspice's switch
    /// truncation: a control value heading for the threshold may cover half the distance left in
    /// one step, and in the transition band a quarter of the band.
    fn switch_step(&self, x0: &[f64], x: &[f64], h: f64) -> f64 {
        let mut step = f64::INFINITY;
        for (ei, e) in self.c.elements.iter().enumerate() {
            let Element::S { model, .. } = e else { continue };
            let (c0, c) = (self.switch_ctl(ei, x0), self.switch_ctl(ei, x));
            let rate = (c - c0) / h;
            if rate == 0.0 {
                continue;
            }
            let w = model.band(self.ctrl[ei].is_some());
            let dist = model.center(self.sw_on.borrow()[ei]) - c;
            if dist.abs() <= 2.0 * w {
                step = step.min(0.25 * w / rate.abs());
            } else if dist * rate > 0.0 {
                step = step.min(0.5 * dist.abs() / rate.abs());
            }
        }
        step
    }

    /// Update the switch states from their control values at the accepted point `x`; whether
    /// any changed.
    fn settle_switches(&self, x: &[f64]) -> bool {
        let mut changed = false;
        for (ei, e) in self.c.elements.iter().enumerate() {
            let Element::S { model, .. } = e else { continue };
            let on = self.sw_on.borrow()[ei];
            let now = self.switch_ctl(ei, x) > model.center(on);
            if now != on {
                self.sw_on.borrow_mut()[ei] = now;
                changed = true;
            }
        }
        changed
    }

    /// A B source's value at `x` and time `t`, its derivatives by reference and the reference values.
    fn b_eval(&self, ei: usize, expr: &str, x: &[f64], t: f64) -> (f64, Vec<f64>, Vec<f64>) {
        let u: Vec<f64> = self.lin[ei].iter().map(|l| lin_value(x, l)).collect();
        let f = |u: &[f64]| {
            let mut p: crate::expr::Params = u.iter().enumerate().map(|(k, v)| (format!("__x{k}"), *v)).collect();
            p.insert("time".into(), t);
            crate::expr::eval(expr, &p).unwrap_or(f64::NAN)
        };
        let f0 = f(&u);
        let d = (0..u.len())
            .map(|k| {
                let h = 1e-6 * (1.0 + u[k].abs());
                let mut up = u.clone();
                let mut dn = u.clone();
                up[k] += h;
                dn[k] -= h;
                (f(&up) - f(&dn)) / (2.0 * h)
            })
            .collect();
        (f0, d, u)
    }

    /// Bulk junction diodes of a MOS at `x`: (anode, cathode, current, conductance).
    fn mos_junctions(
        &self,
        model: &MosModel,
        d: Node,
        s: Node,
        b: Node,
        mult: f64,
        j: &crate::circuit::Junctions,
        x: &[f64],
    ) -> Vec<(Node, Node, f64, f64)> {
        if let Some(inst) = model.bsim4.as_ref().and_then(|b4| b4.inst.as_deref()) {
            // BSIM4: currents from bulk into source and drain in the device polarity.
            let sign = if model.pmos { -1.0 } else { 1.0 };
            let (vs, vd, vb) = (self.v(x, s), self.v(x, d), self.v(x, b));
            let ((ibs, gbs), (ibd, gbd)) = inst.junction_currents(sign * (vb - vs), sign * (vb - vd));
            return [(s, ibs, gbs), (d, ibd, gbd)]
                .into_iter()
                .filter(|&(t, _, _)| t != b)
                .map(|(t, i, g)| (b, t, sign * i * mult, g * mult))
                .collect();
        }
        bulk_diodes(model, d, s, b, mult, j)
            .into_iter()
            .map(|(an, ca, is)| {
                let (id, gd) = exp_iv(is, self.vt, self.v(x, an) - self.v(x, ca));
                (an, ca, id, gd)
            })
            .collect()
    }

    fn v(&self, x: &[f64], n: Node) -> f64 {
        if n == 0 { 0.0 } else { x[n - 1] }
    }

    fn meyer_at(&self, elem: usize, part: usize, x: &[f64]) -> f64 {
        match &self.c.elements[elem] {
            Element::M { d, g, s, b, model, w, l, m, .. } => {
                let v = |n| self.v(x, n);
                meyer(model, *w, *l, *m, v(*d), v(*g), v(*s), v(*b))[part]
            }
            _ => 0.0,
        }
    }

    /// Charge and capacitance of cap `k` at solution `x`. Within a transient step (`state`) a
    /// Meyer capacitance is the average of its step-start value and its predicted step-end value;
    /// without a state it is evaluated at `x`.
    fn cap_qc(&self, k: usize, x: &[f64], state: Option<&TranState>) -> (f64, f64) {
        let cp = &self.caps[k];
        let v = self.v(x, cp.a) - self.v(x, cp.b);
        match &cp.kind {
            CapKind::Linear(c) => (c * v, *c),
            CapKind::Junction { terms, pb, fc, diode } => {
                let (mut q, mut c) = (0.0, 0.0);
                for &(cj0, m) in terms {
                    let (dq, dc) = depletion(cj0, m, *pb, *fc, v);
                    q += dq;
                    c += dc;
                }
                if let Some(d) = diode {
                    let (id, gd) = diode_eval(d, v, self.vt);
                    q += d.tt * id;
                    c += d.tt * gd;
                }
                (q, c)
            }
            CapKind::Meyer { elem, part } => match state {
                Some(s) => {
                    let c = 0.5 * (s.cap_c[k] + s.cap_c_end[k]);
                    (s.cap_q[k] + c * (v - s.cap_v[k]), c)
                }
                None => {
                    let c = self.meyer_at(*elem, *part, x);
                    (c * v, c)
                }
            },
        }
    }

    /// Assemble the linearized system around `x`.
    fn assemble(&self, x: &[f64], mode: Mode, gmin: f64) -> (Matrix<f64>, Vec<f64>) {
        let mut m = Matrix::<f64>::new(self.size);
        let mut rhs = vec![0.0; self.size];
        let idx = |n: Node| if n == 0 { None } else { Some(n - 1) };
        let cond = |m: &mut Matrix<f64>, a: Node, b: Node, g: f64| {
            if let Some(i) = idx(a) {
                m.add(i, i, g);
            }
            if let Some(j) = idx(b) {
                m.add(j, j, g);
            }
            if let (Some(i), Some(j)) = (idx(a), idx(b)) {
                m.add(i, j, -g);
                m.add(j, i, -g);
            }
        };
        // Current `val` flowing out of node a and into node b.
        let cur = |rhs: &mut Vec<f64>, a: Node, b: Node, val: f64| {
            if let Some(i) = idx(a) {
                rhs[i] -= val;
            }
            if let Some(j) = idx(b) {
                rhs[j] += val;
            }
        };
        let (t, src_scale) = match mode {
            Mode::Dc { src_scale, .. } => (None, src_scale),
            Mode::Tran { t, .. } => (Some(t), 1.0),
        };
        for n in 0..self.nn {
            m.add(n, n, gmin);
        }
        if let Mode::Dc { force, .. } = mode {
            // As ngspice: a 1e10 S conductance to the held voltage.
            for &(n, v) in force {
                m.add(n - 1, n - 1, 1e10);
                rhs[n - 1] += 1e10 * v;
            }
        }
        for (ei, e) in self.c.elements.iter().enumerate() {
            match e {
                Element::R { a, b, r, .. } => cond(&mut m, *a, *b, 1.0 / r),
                Element::Ctrl { kind, a, b, cp, cn, gain, .. } => {
                    stamp_ctrl(&mut m, *kind, [*a, *b, *cp, *cn], self.branch[ei], self.ctrl[ei], *gain, 1.0, |g| g);
                }
                Element::B { a, b, voltage, expr, .. } => {
                    let (f0, d, u0) = self.b_eval(ei, expr, x, t.unwrap_or(0.0));
                    let k0 = f0 - d.iter().zip(&u0).map(|(d, u)| d * u).sum::<f64>();
                    if *voltage {
                        let br = self.branch[ei].unwrap();
                        for (n, s) in [(*a, 1.0), (*b, -1.0)] {
                            if let Some(i) = idx(n) {
                                m.add(i, br, s);
                                m.add(br, i, s);
                            }
                        }
                        for (k, l) in self.lin[ei].iter().enumerate() {
                            for &(col, s) in l {
                                m.add(br, col, -d[k] * s);
                            }
                        }
                        rhs[br] += k0;
                    } else {
                        for (n, rs) in [(*a, 1.0), (*b, -1.0)] {
                            let Some(i) = idx(n) else { continue };
                            for (k, l) in self.lin[ei].iter().enumerate() {
                                for &(col, s) in l {
                                    m.add(i, col, rs * d[k] * s);
                                }
                            }
                        }
                        cur(&mut rhs, *a, *b, k0);
                    }
                }
                Element::K { .. } => {}
                Element::S { a, b, cp, cn, model, .. } => {
                    let current = self.ctrl[ei].is_some();
                    let (cl, cv): (Lin, f64) = match self.ctrl[ei] {
                        Some(br) => (vec![(br, 1.0)], x[br]),
                        None => {
                            let l: Lin =
                                [(*cp, 1.0), (*cn, -1.0)].into_iter().filter(|q| q.0 != 0).map(|(n, s)| (n - 1, s)).collect();
                            let v = lin_value(x, &l);
                            (l, v)
                        }
                    };
                    let (g, dg) = model.g(cv, current, self.sw_on.borrow()[ei]);
                    let vab = self.v(x, *a) - self.v(x, *b);
                    cond(&mut m, *a, *b, g);
                    for (n, rs) in [(*a, 1.0), (*b, -1.0)] {
                        if let Some(i) = idx(n) {
                            for &(col, s) in &cl {
                                m.add(i, col, rs * vab * dg * s);
                            }
                        }
                    }
                    cur(&mut rhs, *a, *b, -vab * dg * cv);
                }
                Element::Q { c, b, e, model, area, .. } => {
                    let nodes = [*c, *b, *e];
                    // Junction voltages to linearise at: critical voltages on a cold start, else
                    // the node voltages with the change since the last iteration limited.
                    let p = if model.pnp { -1.0 } else { 1.0 };
                    let (vbn, ven, vcn) = (self.v(x, *b), self.v(x, *e), self.v(x, *c));
                    let (raw_be, raw_bc) = (p * (vbn - ven), p * (vbn - vcn));
                    let (vbe, vbc) = if self.init_jct.get() {
                        (vcrit(model.is * area, model.nf * self.vt).min(0.9), 0.0)
                    } else if let Some([ob, oc]) = self.jct.borrow().get(&ei).copied() {
                        let (be, l1) = pnjlim(raw_be, ob, model.nf * self.vt, vcrit(model.is * area, model.nf * self.vt));
                        let (bc, l2) = pnjlim(raw_bc, oc, model.nr * self.vt, vcrit(model.is * area, model.nr * self.vt));
                        if l1 || l2 {
                            self.limited.set(true);
                        }
                        (be, bc)
                    } else {
                        (raw_be, raw_bc)
                    };
                    self.jct.borrow_mut().insert(ei, [vbe, vbc]);
                    // Node voltages that give those junction voltages (the emitter kept).
                    let vb0 = ven + p * vbe;
                    let v0 = [vb0 - p * vbc, vb0, ven];
                    let xs: Vec<f64> = (0..self.size).map(|k| x[k]).collect();
                    let mut xl = xs;
                    for (n, val) in nodes.iter().zip(v0) {
                        if *n > 0 {
                            xl[*n - 1] = val;
                        }
                    }
                    for (n, i, g) in self.bjt_stamp(model, *area, nodes, &xl) {
                        let Some(r) = idx(n) else { continue };
                        let mut lin = i;
                        for (k, &col) in nodes.iter().enumerate() {
                            lin -= g[k] * v0[k];
                            if let Some(j) = idx(col) {
                                m.add(r, j, g[k]);
                            }
                        }
                        rhs[r] -= lin;
                    }
                    // A tiny conductance across each junction, as for diodes, keeps cut-off
                    // transistors from leaving nodes floating.
                    cond(&mut m, *b, *e, gmin);
                    cond(&mut m, *b, *c, gmin);
                }
                Element::C { .. } => {}
                Element::L { a, b, l, .. } => {
                    let br = self.branch[ei].unwrap();
                    if let Some(i) = idx(*a) {
                        m.add(i, br, 1.0);
                        m.add(br, i, 1.0);
                    }
                    if let Some(j) = idx(*b) {
                        m.add(j, br, -1.0);
                        m.add(br, j, -1.0);
                    }
                    if let Mode::Tran { h, method, state, .. } = mode {
                        let k = self.inductors.iter().position(|&q| q == ei).unwrap();
                        let (req, rhs_v) = match method {
                            Integ::Trapezoidal => {
                                let req = 2.0 * l / h;
                                (req, -req * state.ind_i[k] - state.ind_v[k])
                            }
                            Integ::BackwardEuler => {
                                let req = l / h;
                                (req, -req * state.ind_i[k])
                            }
                        };
                        m.add(br, br, -req);
                        rhs[br] += rhs_v;
                    }
                }
                Element::V { a, b, src, .. } => {
                    let br = self.branch[ei].unwrap();
                    if let Some(i) = idx(*a) {
                        m.add(i, br, 1.0);
                        m.add(br, i, 1.0);
                    }
                    if let Some(j) = idx(*b) {
                        m.add(j, br, -1.0);
                        m.add(br, j, -1.0);
                    }
                    rhs[br] += src.at(t) * src_scale;
                }
                Element::I { a, b, src, .. } => cur(&mut rhs, *a, *b, src.at(t) * src_scale),
                Element::D { a, k, model, .. } => {
                    let nvt = model.n * self.vt;
                    let raw = self.v(x, *a) - self.v(x, *k);
                    let vd = if self.init_jct.get() {
                        vcrit(model.is, nvt).min(0.9)
                    } else if let Some([old, _]) = self.jct.borrow().get(&ei).copied() {
                        let (v, lim) = pnjlim(raw, old, nvt, vcrit(model.is, nvt));
                        if lim {
                            self.limited.set(true);
                        }
                        v
                    } else {
                        raw
                    };
                    self.jct.borrow_mut().insert(ei, [vd, 0.0]);
                    let (id, gd) = diode_eval(model, vd, self.vt);
                    cond(&mut m, *a, *k, gd);
                    cur(&mut rhs, *a, *k, id - gd * vd);
                }
                Element::M { d, g, s, b, model, w, l, m: mult, junctions, .. } => {
                    for (an, ca, id, gd) in self.mos_junctions(model, *d, *s, *b, *mult, junctions, x) {
                        let vd = self.v(x, an) - self.v(x, ca);
                        cond(&mut m, an, ca, gd);
                        cur(&mut rhs, an, ca, id - gd * vd);
                    }
                    let nodes = [*d, *g, *s, *b];
                    let vs: Vec<f64> = nodes.iter().map(|&n| self.v(x, n)).collect();
                    let ev = mos_eval(model, *w, *l, *mult, vs[0], vs[1], vs[2], vs[3]);
                    let i_eq = ev.id - (0..4).map(|k| ev.j[k] * vs[k]).sum::<f64>();
                    for (k, &n) in nodes.iter().enumerate() {
                        if let Some(col) = idx(n) {
                            if let Some(rd) = idx(*d) {
                                m.add(rd, col, ev.j[k]);
                            }
                            if let Some(rs) = idx(*s) {
                                m.add(rs, col, -ev.j[k]);
                            }
                        }
                    }
                    cur(&mut rhs, *d, *s, i_eq);
                    if let Some((i, g, _)) = mos_leakage(model, *mult, [vs[0], vs[1], vs[2], vs[3]]) {
                        for t in 0..4 {
                            let Some(r) = idx(nodes[t]) else { continue };
                            let mut lin = i[t];
                            for j in 0..4 {
                                lin -= g[t][j] * vs[j];
                                if let Some(col) = idx(nodes[j]) {
                                    m.add(r, col, g[t][j]);
                                }
                            }
                            rhs[r] -= lin;
                        }
                    }
                    // gmin across the bulk junctions, as ngspice: an internal node of an off
                    // stack settles at the bulk, not between the stack's ends.
                    cond(&mut m, *d, *b, gmin);
                    cond(&mut m, *s, *b, gmin);
                }
            }
        }
        if let Mode::Tran { h, method, state, .. } = mode {
            for (k, cp) in self.caps.iter().enumerate() {
                let v = self.v(x, cp.a) - self.v(x, cp.b);
                let (q, c) = self.cap_qc(k, x, Some(state));
                let (i, geq) = Self::cap_current(method, h, q, c, state, k);
                cond(&mut m, cp.a, cp.b, geq);
                cur(&mut rhs, cp.a, cp.b, i - geq * v);
            }
            for (k, (_, nodes)) in self.qdevs.iter().enumerate() {
                let (q, c) = self.qdev_qc(k, x);
                let f = match method {
                    Integ::Trapezoidal => 2.0 / h,
                    Integ::BackwardEuler => 1.0 / h,
                };
                let vs = nodes.map(|n| self.v(x, n));
                for t in 0..4 {
                    let Some(r) = idx(nodes[t]) else { continue };
                    let i = Self::charge_current(method, h, q[t], state.qdev_q[k][t], state.qdev_i[k][t]);
                    let mut lin = i;
                    for j in 0..4 {
                        let g = f * c[t][j];
                        lin -= g * vs[j];
                        if let Some(col) = idx(nodes[j]) {
                            m.add(r, col, g);
                        }
                    }
                    rhs[r] -= lin;
                }
            }
            let f = match method {
                Integ::Trapezoidal => 2.0 / h,
                Integ::BackwardEuler => 1.0 / h,
            };
            for &(b1, b2, p1, p2, mm) in &self.mutual {
                m.add(b1, b2, -f * mm);
                m.add(b2, b1, -f * mm);
                rhs[b1] -= f * mm * state.ind_i[p2];
                rhs[b2] -= f * mm * state.ind_i[p1];
            }
        }
        (m, rhs)
    }

    /// Current into a terminal whose charge goes from `q0` (with current `i0`) to `q` over a step of `h`.
    fn charge_current(method: Integ, h: f64, q: f64, q0: f64, i0: f64) -> f64 {
        match method {
            Integ::Trapezoidal => 2.0 / h * (q - q0) - i0,
            Integ::BackwardEuler => (q - q0) / h,
        }
    }

    /// Terminal charges (d, g, s, b) of charge-based device `k` at `x` and their derivatives with
    /// respect to the terminal voltages (d, g, s, b).
    fn qdev_qc(&self, k: usize, x: &[f64]) -> ([f64; 4], [[f64; 4]; 4]) {
        let (ei, nodes) = self.qdevs[k];
        match &self.c.elements[ei] {
            Element::M { model, w, l, m, .. } => mos_charges(model, *w, *l, *m, nodes.map(|n| self.v(x, n))).unwrap_or_default(),
            _ => Default::default(),
        }
    }

    /// Current through a cap holding charge `q` (capacitance `c`) at the end of a step of `h`,
    /// and its conductance, by charge-based integration.
    fn cap_current(method: Integ, h: f64, q: f64, c: f64, state: &TranState, k: usize) -> (f64, f64) {
        match method {
            Integ::Trapezoidal => (2.0 / h * (q - state.cap_q[k]) - state.cap_i[k], 2.0 * c / h),
            Integ::BackwardEuler => ((q - state.cap_q[k]) / h, c / h),
        }
    }

    fn newton(&self, x: &mut [f64], mode: Mode, gmin: f64, max_iter: usize) -> Result<usize, SimError> {
        for it in 0..max_iter {
            self.limited.set(false);
            let (mut m, mut rhs) = self.assemble(x, mode, gmin);
            self.init_jct.set(false);
            let limited = self.limited.get();
            if !solve(&mut m, &mut rhs) {
                return Err(SimError::Engine("singular matrix (floating node or voltage source loop?)".into()));
            }
            let mut converged = true;
            for i in 0..self.size {
                let mut dx = rhs[i] - x[i];
                let is_v = i < self.nn;
                if self.damp && is_v && dx.abs() > 0.5 {
                    dx = 0.5 * dx.signum(); // damp large voltage steps
                    converged = false;
                }
                let o = &self.c.options;
                let tol = if is_v { o.vntol } else { o.abstol } + o.reltol * rhs[i].abs().max(x[i].abs());
                if dx.abs() > tol {
                    converged = false;
                }
                x[i] += dx;
            }
            if !x.iter().all(|v| v.is_finite()) {
                return Err(SimError::Engine("numerical overflow".into()));
            }
            if converged && !limited && it > 0 {
                return Ok(it + 1);
            }
        }
        Err(SimError::NoConvergence)
    }

    /// DC operating point with gmin and source stepping fallbacks; `.nodeset` voltages give the
    /// starting point.
    pub fn op(&mut self, x0: Option<&[f64]>) -> Result<Vec<f64>, SimError> {
        let mut x = x0.map(|v| v.to_vec()).unwrap_or_else(|| vec![0.0; self.size]);
        let has_junctions = self.c.elements.iter().any(|e| matches!(e, Element::Q { .. } | Element::D { .. }));
        self.init_jct.set(x0.is_none() && self.c.nodeset.is_empty() && has_junctions);
        if x0.is_none() && !self.c.nodeset.is_empty() {
            let nodeset = self.c.nodeset.clone();
            if let Ok(xs) = self.op_forced(&nodeset) {
                x = xs;
            }
        }
        self.op_forced_from(x, &[])
    }

    /// Operating point with `force` nodes held at their voltages.
    fn op_forced(&mut self, force: &[(Node, f64)]) -> Result<Vec<f64>, SimError> {
        self.op_forced_from(vec![0.0; self.size], force)
    }

    fn op_forced_from(&mut self, x: Vec<f64>, force: &[(Node, f64)]) -> Result<Vec<f64>, SimError> {
        // Solve again while switches change state at the solution.
        let mut x = self.op_solve(x, force)?;
        for _ in 0..20 {
            if !self.settle_switches(&x) {
                break;
            }
            x = self.op_solve(x, force)?;
        }
        Ok(x)
    }

    fn op_solve(&mut self, mut x: Vec<f64>, force: &[(Node, f64)]) -> Result<Vec<f64>, SimError> {
        let (gmin0, itl) = (self.c.options.gmin, self.c.options.itl1);
        let start = x.clone();
        if self.newton(&mut x, Mode::Dc { src_scale: 1.0, force }, gmin0, itl).is_ok() {
            return Ok(x);
        }
        self.log.push("OP: direct Newton failed, trying gmin stepping".into());
        // From the start point first (a `.nodeset` state picks the solution of circuits with
        // several, such as a bandgap's zero-current state), then from zero.
        let starts = if start.iter().any(|v| *v != 0.0) { vec![start, vec![0.0; self.size]] } else { vec![start] };
        for s0 in starts {
            let mut x = s0;
            let mut ok = true;
            for k in (0..=10).rev() {
                let g = gmin0 * 10f64.powi(k);
                if self.newton(&mut x, Mode::Dc { src_scale: 1.0, force }, g, itl).is_err() {
                    ok = false;
                    break;
                }
            }
            if ok {
                return Ok(x);
            }
        }
        self.log.push("OP: gmin stepping failed, trying source stepping".into());
        let mut x = vec![0.0; self.size];
        for k in 0..=20 {
            let s = k as f64 / 20.0;
            self.newton(&mut x, Mode::Dc { src_scale: s, force }, gmin0, itl.max(200))?;
        }
        Ok(x)
    }

    fn vector_names(&self) -> Vec<(String, VecKind)> {
        let mut v: Vec<(String, VecKind)> = self.c.nodes.iter().skip(1).map(|n| (format!("v({n})"), VecKind::Voltage)).collect();
        for (i, e) in self.c.elements.iter().enumerate() {
            if self.branch[i].is_some() {
                v.push((format!("i({})", e.name()), VecKind::Current));
            }
        }
        v
    }

    fn new_dataset(&self, analysis: &str, title: &str, scale: Option<(&str, VecKind)>) -> Dataset {
        let mut vectors = vec![];
        if let Some((n, k)) = scale {
            vectors.push(Vector::real(n, k, vec![]));
        }
        for (n, k) in self.vector_names() {
            vectors.push(Vector::real(n, k, vec![]));
        }
        Dataset { analysis: analysis.into(), title: title.into(), scale: scale.map(|_| 0), vectors, devices: vec![] }
    }

    fn push_point(&self, ds: &mut Dataset, scale: Option<f64>, x: &[f64]) {
        let mut k = 0;
        if let Some(s) = scale {
            ds.vectors[0].re.push(s);
            k = 1;
        }
        for i in 0..self.size {
            ds.vectors[k + i].re.push(x[i]);
        }
    }

    pub fn device_ops(&self, x: &[f64]) -> Vec<DeviceOp> {
        let mut out = vec![];
        for e in &self.c.elements {
            match e {
                Element::M { name, d, g, s, b, model, w, l, m, .. } => {
                    let v = |n| self.v(x, n);
                    let ev = mos_eval(model, *w, *l, *m, v(*d), v(*g), v(*s), v(*b));
                    let [cgs, cgd, cgb] = match mos_charges(model, *w, *l, *m, [v(*d), v(*g), v(*s), v(*b)]) {
                        // Gate capacitances: -dQg/dV of the other terminals (overlaps included
                        // for BSIM4; added for level 1).
                        Some((_, c)) if model.bsim4.is_some() => [-c[1][2], -c[1][0], -c[1][3]],
                        Some((_, c)) => [
                            -c[1][2] + model.cgso * w * m,
                            -c[1][0] + model.cgdo * w * m,
                            -c[1][3] + model.cgbo * model.leff(*l) * m,
                        ],
                        None => {
                            let [cgs, cgd, cgb] = meyer(model, *w, *l, *m, v(*d), v(*g), v(*s), v(*b));
                            [cgs + model.cgso * w * m, cgd + model.cgdo * w * m, cgb + model.cgbo * model.leff(*l) * m]
                        }
                    };
                    out.push(DeviceOp {
                        name: name.clone(),
                        region: Some(ev.region.to_string()),
                        values: vec![
                            ("id".into(), ev.id),
                            ("vgs".into(), v(*g) - v(*s)),
                            ("vds".into(), v(*d) - v(*s)),
                            ("vth".into(), ev.vth),
                            ("gm".into(), ev.gm),
                            ("gds".into(), ev.gds),
                            ("cgs".into(), cgs),
                            ("cgd".into(), cgd),
                            ("cgb".into(), cgb),
                        ],
                    });
                }
                Element::Q { name, c, b, e, model, area } => {
                    let [(_, ic, gc), (_, ib, _), _] = self.bjt_stamp(model, *area, [*c, *b, *e], x);
                    let p = if model.pnp { -1.0 } else { 1.0 };
                    let (vbe, vce) = (self.v(x, *b) - self.v(x, *e), self.v(x, *c) - self.v(x, *e));
                    let (vbe_n, vbc_n) = (p * vbe, p * (self.v(x, *b) - self.v(x, *c)));
                    let region = match (vbe_n > 0.4, vbc_n > 0.4) {
                        (true, false) => "active",
                        (true, true) => "saturation",
                        (false, false) => "cutoff",
                        (false, true) => "reverse",
                    };
                    out.push(DeviceOp {
                        name: name.clone(),
                        region: Some(region.into()),
                        values: vec![
                            ("ic".into(), ic),
                            ("ib".into(), ib),
                            ("vbe".into(), vbe),
                            ("vce".into(), vce),
                            ("gm".into(), gc[1]),
                            ("beta".into(), if ib.abs() > 0.0 { ic / ib } else { f64::NAN }),
                        ],
                    });
                }
                Element::D { name, a, k, model } => {
                    let vd = self.v(x, *a) - self.v(x, *k);
                    let (id, gd) = diode_eval(model, vd, self.vt);
                    out.push(DeviceOp {
                        name: name.clone(),
                        region: None,
                        values: vec![("vd".into(), vd), ("id".into(), id), ("gd".into(), gd)],
                    });
                }
                Element::R { name, a, b, r } => {
                    let vr = self.v(x, *a) - self.v(x, *b);
                    out.push(DeviceOp {
                        name: name.clone(),
                        region: None,
                        values: vec![("v".into(), vr), ("i".into(), vr / r), ("p".into(), vr * vr / r)],
                    });
                }
                _ => {}
            }
        }
        out
    }

    pub fn run_op(&mut self) -> Result<Dataset, SimError> {
        let x = self.op(None)?;
        let mut ds = self.new_dataset("op", "Operating Point", None);
        self.push_point(&mut ds, None, &x);
        ds.devices = self.device_ops(&x);
        Ok(ds)
    }

    pub fn run_dc(&mut self, source: &str, start: f64, stop: f64, step: f64) -> Result<Dataset, SimError> {
        let si = self
            .c
            .elements
            .iter()
            .position(|e| e.name().eq_ignore_ascii_case(source) && matches!(e, Element::V { .. } | Element::I { .. }))
            .ok_or_else(|| SimError::Engine(format!("DC sweep source '{source}' not found")))?;
        // Steps that land on `stop` up to rounding (e.g. -1.2 to 1.2 by 0.05) include it.
        let steps = ((stop - start) / step).abs();
        let n = (steps + 1e-9 * steps.max(1.0)).floor() as usize + 1;
        if n > 1_000_000 {
            return Err(SimError::Engine("DC sweep has too many points".into()));
        }
        let step = if stop >= start { step.abs() } else { -step.abs() };
        let mut ds =
            self.new_dataset("dc", &format!("DC sweep of {source}"), Some((&source.to_ascii_lowercase(), VecKind::Sweep)));
        let mut x: Option<Vec<f64>> = None;
        for k in 0..n {
            let val = start + step * k as f64;
            match &mut self.c.elements[si] {
                Element::V { src, .. } | Element::I { src, .. } => src.dc = val,
                _ => unreachable!(),
            }
            let sol = self.op(x.as_deref())?;
            self.push_point(&mut ds, Some(val), &sol);
            x = Some(sol);
        }
        Ok(ds)
    }

    /// Small-signal system at angular frequency `w` around the operating point `x0`. The right
    /// side holds the AC sources, or only element `only` at unit magnitude.
    fn ac_system(&self, x0: &[f64], caps_at_op: &[f64], w: f64, only: Option<usize>) -> (Matrix<Complex64>, Vec<Complex64>) {
        let idx = |n: Node| if n == 0 { None } else { Some(n - 1) };
        let one = Complex64::new(1.0, 0.0);
        let mut m = Matrix::<Complex64>::new(self.size);
        let mut rhs = vec![Complex64::new(0.0, 0.0); self.size];
        let y = |m: &mut Matrix<Complex64>, a: Node, b: Node, g: Complex64| {
            if let Some(i) = idx(a) {
                m.add(i, i, g);
            }
            if let Some(j) = idx(b) {
                m.add(j, j, g);
            }
            if let (Some(i), Some(j)) = (idx(a), idx(b)) {
                m.add(i, j, -g);
                m.add(j, i, -g);
            }
        };
        let branch = |m: &mut Matrix<Complex64>, a: Node, b: Node, br: usize| {
            if let Some(i) = idx(a) {
                m.add(i, br, one);
                m.add(br, i, one);
            }
            if let Some(j) = idx(b) {
                m.add(j, br, -one);
                m.add(br, j, -one);
            }
        };
        let excitation = |ei: usize, src: &crate::circuit::Source| match only {
            Some(k) if k == ei => one,
            Some(_) => Complex64::new(0.0, 0.0),
            None => Complex64::from_polar(src.ac_mag, src.ac_phase.to_radians()),
        };
        for n in 0..self.nn {
            m.add(n, n, Complex64::new(self.c.options.gmin, 0.0));
        }
        for (ei, e) in self.c.elements.iter().enumerate() {
            match e {
                Element::R { a, b, r, .. } => y(&mut m, *a, *b, Complex64::new(1.0 / r, 0.0)),
                Element::Q { c, b, e, model, area, .. } => {
                    let nodes = [*c, *b, *e];
                    for (n, _, g) in self.bjt_stamp(model, *area, nodes, x0) {
                        let Some(r) = idx(n) else { continue };
                        for (k, &col) in nodes.iter().enumerate() {
                            if let Some(j) = idx(col) {
                                m.add(r, j, Complex64::new(g[k], 0.0));
                            }
                        }
                    }
                    y(&mut m, *b, *e, Complex64::new(self.c.options.gmin, 0.0));
                    y(&mut m, *b, *c, Complex64::new(self.c.options.gmin, 0.0));
                }
                Element::Ctrl { kind, a, b, cp, cn, gain, .. } => stamp_ctrl(
                    &mut m,
                    *kind,
                    [*a, *b, *cp, *cn],
                    self.branch[ei],
                    self.ctrl[ei],
                    *gain,
                    Complex64::new(1.0, 0.0),
                    |g| Complex64::new(g, 0.0),
                ),
                Element::B { a, b, voltage, expr, .. } => {
                    let (_, d, _) = self.b_eval(ei, expr, x0, 0.0);
                    if *voltage {
                        let br = self.branch[ei].unwrap();
                        branch(&mut m, *a, *b, br);
                        for (k, l) in self.lin[ei].iter().enumerate() {
                            for &(col, s) in l {
                                m.add(br, col, Complex64::new(-d[k] * s, 0.0));
                            }
                        }
                    } else {
                        for (n, rs) in [(*a, 1.0), (*b, -1.0)] {
                            let Some(i) = idx(n) else { continue };
                            for (k, l) in self.lin[ei].iter().enumerate() {
                                for &(col, s) in l {
                                    m.add(i, col, Complex64::new(rs * d[k] * s, 0.0));
                                }
                            }
                        }
                    }
                }
                Element::K { .. } => {}
                Element::S { a, b, cp, cn, model, .. } => {
                    let current = self.ctrl[ei].is_some();
                    let (cl, cv): (Lin, f64) = match self.ctrl[ei] {
                        Some(br) => (vec![(br, 1.0)], x0[br]),
                        None => {
                            let l: Lin =
                                [(*cp, 1.0), (*cn, -1.0)].into_iter().filter(|q| q.0 != 0).map(|(n, s)| (n - 1, s)).collect();
                            let v = lin_value(x0, &l);
                            (l, v)
                        }
                    };
                    let (g, dg) = model.g(cv, current, self.sw_on.borrow()[ei]);
                    let vab = self.v(x0, *a) - self.v(x0, *b);
                    y(&mut m, *a, *b, Complex64::new(g, 0.0));
                    for (n, rs) in [(*a, 1.0), (*b, -1.0)] {
                        if let Some(i) = idx(n) {
                            for &(col, s) in &cl {
                                m.add(i, col, Complex64::new(rs * vab * dg * s, 0.0));
                            }
                        }
                    }
                }
                Element::C { .. } => {}
                Element::L { a, b, l, .. } => {
                    let br = self.branch[ei].unwrap();
                    branch(&mut m, *a, *b, br);
                    m.add(br, br, Complex64::new(0.0, -w * l));
                }
                Element::V { a, b, src, .. } => {
                    let br = self.branch[ei].unwrap();
                    branch(&mut m, *a, *b, br);
                    rhs[br] += excitation(ei, src);
                }
                Element::I { a, b, src, .. } => {
                    let v = excitation(ei, src);
                    if let Some(i) = idx(*a) {
                        rhs[i] -= v;
                    }
                    if let Some(j) = idx(*b) {
                        rhs[j] += v;
                    }
                }
                Element::D { a, k, model, .. } => {
                    let vd = self.v(x0, *a) - self.v(x0, *k);
                    let (_, gd) = diode_eval(model, vd, self.vt);
                    y(&mut m, *a, *k, Complex64::new(gd, 0.0));
                }
                Element::M { d, g, s, b, model, w: wd, l, m: mult, junctions, .. } => {
                    for (an, ca, _, gd) in self.mos_junctions(model, *d, *s, *b, *mult, junctions, x0) {
                        y(&mut m, an, ca, Complex64::new(gd, 0.0));
                    }
                    let nodes = [*d, *g, *s, *b];
                    let vs: Vec<f64> = nodes.iter().map(|&n| self.v(x0, n)).collect();
                    let ev = mos_eval(model, *wd, *l, *mult, vs[0], vs[1], vs[2], vs[3]);
                    for (k, &n) in nodes.iter().enumerate() {
                        if let Some(col) = idx(n) {
                            if let Some(rd) = idx(*d) {
                                m.add(rd, col, Complex64::new(ev.j[k], 0.0));
                            }
                            if let Some(rs) = idx(*s) {
                                m.add(rs, col, Complex64::new(-ev.j[k], 0.0));
                            }
                        }
                    }
                    if let Some((_, g, _)) = mos_leakage(model, *mult, [vs[0], vs[1], vs[2], vs[3]]) {
                        for t in 0..4 {
                            let Some(r) = idx(nodes[t]) else { continue };
                            for j in 0..4 {
                                if let Some(col) = idx(nodes[j]) {
                                    m.add(r, col, Complex64::new(g[t][j], 0.0));
                                }
                            }
                        }
                    }
                    y(&mut m, *d, *b, Complex64::new(self.c.options.gmin, 0.0));
                    y(&mut m, *s, *b, Complex64::new(self.c.options.gmin, 0.0));
                }
            }
        }
        for (cp, c) in self.caps.iter().zip(caps_at_op) {
            y(&mut m, cp.a, cp.b, Complex64::new(0.0, w * c));
        }
        for &(b1, b2, _, _, mm) in &self.mutual {
            m.add(b1, b2, Complex64::new(0.0, -w * mm));
            m.add(b2, b1, Complex64::new(0.0, -w * mm));
        }
        for (k, (_, nodes)) in self.qdevs.iter().enumerate() {
            let (_, c) = self.qdev_qc(k, x0);
            for t in 0..4 {
                let Some(r) = idx(nodes[t]) else { continue };
                for j in 0..4 {
                    if let Some(col) = idx(nodes[j]) {
                        m.add(r, col, Complex64::new(0.0, w * c[t][j]));
                    }
                }
            }
        }
        (m, rhs)
    }

    fn log_frequencies(ppd: u32, fstart: f64, fstop: f64) -> Vec<f64> {
        let npts = ((fstop / fstart).log10() * ppd as f64).ceil().max(1.0) as usize + 1;
        (0..npts).map(|p| fstart * 10f64.powf(p as f64 / ppd as f64)).take_while(|f| *f <= fstop * 1.000001).collect()
    }

    pub fn run_ac(&mut self, ppd: u32, fstart: f64, fstop: f64) -> Result<Dataset, SimError> {
        self.run_ac_sweep(ics_core::sim_setup::AcSweep::Dec, ppd, fstart, fstop)
    }

    /// AC analysis over `sweep`'s frequencies (`n` per decade / octave, or `n` in all for lin).
    pub fn run_ac_sweep(
        &mut self,
        sweep: ics_core::sim_setup::AcSweep,
        n: u32,
        fstart: f64,
        fstop: f64,
    ) -> Result<Dataset, SimError> {
        let x0 = self.op(None)?;
        let caps_at_op: Vec<f64> = (0..self.caps.len()).map(|k| self.cap_qc(k, &x0, None).1).collect();
        let mut ds = self.new_dataset("ac", "AC analysis", Some(("frequency", VecKind::Frequency)));
        for v in ds.vectors.iter_mut().skip(1) {
            v.im = Some(vec![]);
        }
        for f in sweep.frequencies(n, fstart, fstop) {
            let (mut m, mut rhs) = self.ac_system(&x0, &caps_at_op, 2.0 * std::f64::consts::PI * f, None);
            if !solve(&mut m, &mut rhs) {
                return Err(SimError::Engine(format!("AC: singular matrix at f={f:.3e}")));
            }
            ds.vectors[0].re.push(f);
            for i in 0..self.size {
                ds.vectors[1 + i].re.push(rhs[i].re);
                ds.vectors[1 + i].im.as_mut().unwrap().push(rhs[i].im);
            }
        }
        Ok(ds)
    }

    /// Noise current sources at the operating point `x0` and frequency `f`: (from, to, PSD in
    /// A²/Hz). Resistor and MOS channel thermal noise, diode shot noise, MOS and diode flicker
    /// noise (SPICE level 1: `kf id^af / (f cox leff²)`).
    fn noise_sources(&self, x0: &[f64], f: f64) -> Vec<(Node, Node, f64)> {
        let kt = BOLTZMANN * (self.c.temp + 273.15);
        let mut out = vec![];
        for e in &self.c.elements {
            match e {
                Element::R { a, b, r, .. } => out.push((*a, *b, 4.0 * kt / r.abs())),
                Element::Q { c, b, e, model, area, .. } => {
                    // Shot noise of the collector and base currents, flicker on the base.
                    let [(_, ic, _), (_, ib, _), _] = self.bjt_stamp(model, *area, [*c, *b, *e], x0);
                    out.push((*c, *e, 2.0 * CHARGE * ic.abs()));
                    out.push((*b, *e, 2.0 * CHARGE * ib.abs() + model.kf * ib.abs().powf(model.af) / f));
                }
                Element::D { a, k, model, .. } => {
                    let (id, _) = diode_eval(model, self.v(x0, *a) - self.v(x0, *k), self.vt);
                    let flicker = model.kf * id.abs().powf(model.af) / f;
                    out.push((*a, *k, 2.0 * CHARGE * id.abs() + flicker));
                }
                Element::M { d, g, s, b, model, m, .. } if model.bsim4.as_ref().is_some_and(|b4| b4.inst.is_some()) => {
                    let inst = model.bsim4.as_ref().and_then(|b4| b4.inst.as_deref()).unwrap();
                    let sign = if model.pmos { -1.0 } else { 1.0 };
                    let v = |n| sign * self.v(x0, n);
                    let (mut vd, mut vs) = (v(*d), v(*s));
                    if vd < vs {
                        std::mem::swap(&mut vd, &mut vs);
                    }
                    let (thermal, flicker) = inst.noise(vd - vs, v(*g) - vs, v(*b) - vs, f);
                    out.push((*d, *s, (thermal + flicker) * m));
                    let x = |n| self.v(x0, n);
                    if let Some((_, _, [gs, gd, gb])) = mos_leakage(model, *m, [x(*d), x(*g), x(*s), x(*b)]) {
                        // Shot noise of the gate currents.
                        for (t, i) in [(*s, gs), (*d, gd), (*b, gb)] {
                            out.push((*g, t, 2.0 * CHARGE * i.abs()));
                        }
                    }
                }
                Element::M { d, g, s, b, model, w, l, m, .. } => {
                    let v = |n| self.v(x0, n);
                    let ev = mos_eval(model, *w, *l, *m, v(*d), v(*g), v(*s), v(*b));
                    let thermal = 8.0 / 3.0 * kt * ev.gm.abs();
                    let cox = if model.cox() > 0.0 { model.cox() } else { 3.453e-4 };
                    let leff = model.leff(*l);
                    let flicker = model.kf * m * (ev.id.abs() / m).powf(model.af) / (f * cox * leff * leff);
                    out.push((*d, *s, thermal + flicker));
                }
                _ => {}
            }
        }
        out
    }

    /// Output noise at node `output` and the equivalent input noise of source `input`, as
    /// spectral densities in V/sqrt(Hz) (`onoise_spectrum`, `inoise_spectrum` as in ngspice).
    pub fn run_noise(&mut self, output: &str, input: &str, ppd: u32, fstart: f64, fstop: f64) -> Result<Dataset, SimError> {
        let out_node = self
            .c
            .nodes
            .iter()
            .position(|n| n.eq_ignore_ascii_case(output.trim().trim_start_matches("v(").trim_end_matches(')')))
            .filter(|&n| n > 0)
            .ok_or_else(|| SimError::Engine(format!("noise: output node '{output}' not found")))?;
        let src = self
            .c
            .elements
            .iter()
            .position(|e| e.name().eq_ignore_ascii_case(input) && matches!(e, Element::V { .. } | Element::I { .. }))
            .ok_or_else(|| SimError::Engine(format!("noise: input source '{input}' not found")))?;
        let x0 = self.op(None)?;
        let caps_at_op: Vec<f64> = (0..self.caps.len()).map(|k| self.cap_qc(k, &x0, None).1).collect();
        let mut ds = Dataset {
            analysis: "noise".into(),
            title: format!("Noise at v({output}) referred to {input}"),
            scale: Some(0),
            vectors: vec![
                Vector::real("frequency", VecKind::Frequency, vec![]),
                Vector::real("onoise_spectrum", VecKind::Voltage, vec![]),
                Vector::real("inoise_spectrum", VecKind::Voltage, vec![]),
            ],
            devices: vec![],
        };
        let mut total = 0.0;
        let mut prev: Option<(f64, f64)> = None;
        for f in Self::log_frequencies(ppd, fstart, fstop) {
            let w = 2.0 * std::f64::consts::PI * f;
            let (m, mut gain) = self.ac_system(&x0, &caps_at_op, w, Some(src));
            // Adjoint: z = Y^-T e_out gives the output voltage per unit current into each node.
            let mut mt = m.transposed();
            let mut z = vec![Complex64::new(0.0, 0.0); self.size];
            z[out_node - 1] = Complex64::new(1.0, 0.0);
            let mut m = m;
            if !solve(&mut m, &mut gain) || !solve(&mut mt, &mut z) {
                return Err(SimError::Engine(format!("noise: singular matrix at f={f:.3e}")));
            }
            let zv = |n: Node| if n == 0 { Complex64::new(0.0, 0.0) } else { z[n - 1] };
            let sv: f64 = self.noise_sources(&x0, f).iter().map(|(a, b, s)| (zv(*a) - zv(*b)).norm_sqr() * s).sum();
            let h = gain[out_node - 1].norm();
            ds.vectors[0].re.push(f);
            ds.vectors[1].re.push(sv.sqrt());
            ds.vectors[2].re.push(sv.sqrt() / h.max(1e-300));
            if let Some((f0, s0)) = prev {
                total += 0.5 * (s0 + sv) * (f - f0);
            }
            prev = Some((f, sv));
        }
        self.log.push(format!("noise: total output noise {:.4e} V rms over {fstart:.3e}..{fstop:.3e} Hz", total.sqrt()));
        Ok(ds)
    }

    pub fn run_tran(&mut self, tstep: f64, tstop: f64) -> Result<Dataset, SimError> {
        self.run_tran_limited(tstep, tstop, None)
    }

    /// Transient analysis whose time step stays below `tmax` when given.
    pub fn run_tran_limited(&mut self, tstep: f64, tstop: f64, tmax: Option<f64>) -> Result<Dataset, SimError> {
        let hmax = tstep.min(tstop / 100.0).min(tmax.filter(|t| *t > 0.0).unwrap_or(f64::INFINITY));
        if tstop / hmax > 2.0e6 {
            return Err(SimError::Engine("transient would need more than 2M points; increase tstep".into()));
        }
        // Breakpoints from source waveforms.
        let mut bps: Vec<f64> = vec![];
        for e in &self.c.elements {
            if let Element::V { src, .. } | Element::I { src, .. } = e
                && let Some(w) = &src.tran
            {
                bps.extend(w.breakpoints(tstop));
            }
        }
        bps.push(tstop);
        bps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        bps.dedup_by(|a, b| (*a - *b).abs() < hmax * 1e-6);

        // Initial condition: DC operating point with sources evaluated at t=0.
        let x = {
            // Evaluate transient sources at t = 0 for the initial DC solution.
            let saved = self.c.elements.clone();
            for e in &mut self.c.elements {
                if let Element::V { src, .. } | Element::I { src, .. } = e
                    && let Some(w) = &src.tran
                {
                    src.dc = w.value(0.0);
                }
            }
            let r = if self.c.uic {
                // Use initial conditions, no operating point: capacitor and MOS charge terminals
                // start at their `.ic` voltage (0 without one), as if held by voltage sources at
                // t = 0+; the other nodes follow from them. Nodes set by voltage sources keep
                // the source's value.
                let mut held = vec![false; self.c.node_count()];
                for cp in &self.caps {
                    held[cp.a] = true;
                    held[cp.b] = true;
                }
                for (_, nodes) in &self.qdevs {
                    for &n in nodes {
                        held[n] = true;
                    }
                }
                for e in &self.c.elements {
                    if let Element::V { a, b, .. } = e {
                        held[*a] = false;
                        held[*b] = false;
                    }
                }
                let mut force: Vec<(Node, f64)> = (1..held.len()).filter(|&n| held[n]).map(|n| (n, 0.0)).collect();
                for &(n, v) in &self.c.ic {
                    match force.iter_mut().find(|f| f.0 == n) {
                        Some(f) => f.1 = v,
                        None => force.push((n, v)),
                    }
                }
                self.op_forced(&force)
            } else if self.c.ic.is_empty() {
                self.op(None)
            } else {
                let ic = self.c.ic.clone();
                self.op_forced(&ic)
            };
            self.c.elements = saved;
            r?
        };
        let mut x = x;
        let qc: Vec<(f64, f64)> = (0..self.caps.len()).map(|k| self.cap_qc(k, &x, None)).collect();
        let mut state = TranState {
            cap_q: qc.iter().map(|p| p.0).collect(),
            cap_c: qc.iter().map(|p| p.1).collect(),
            cap_c_end: qc.iter().map(|p| p.1).collect(),
            cap_v: self.caps.iter().map(|c| self.v(&x, c.a) - self.v(&x, c.b)).collect(),
            cap_i: vec![0.0; self.caps.len()],
            ind_v: vec![0.0; self.inductors.len()],
            ind_i: self.inductors.iter().map(|&i| x[self.branch[i].unwrap()]).collect(),
            qdev_q: (0..self.qdevs.len()).map(|k| self.qdev_qc(k, &x).0).collect(),
            qdev_i: vec![[0.0; 4]; self.qdevs.len()],
        };
        let mut ds = self.new_dataset("tran", "Transient", Some(("time", VecKind::Time)));
        self.push_point(&mut ds, Some(0.0), &x);

        let mut t = 0.0;
        // As ngspice: the first step, and the first after each breakpoint, is a tenth of the
        // planned step or of the gap to the next breakpoint.
        let mut h = 0.1 * (hmax / 10.0).min(bps.iter().copied().find(|&b| b > hmax * 1e-9).unwrap_or(tstop));
        let mut method = Integ::BackwardEuler;
        let mut bp_i = 0;
        let mut steps = 0usize;
        // Accepted points since the last breakpoint, for the truncation error estimate.
        let mut hist: Vec<Point> = vec![(0.0, x.clone(), self.reactive(&x, None))];
        // Recent branch currents, to detect trapezoidal ringing.
        let mut ring: Vec<Vec<f64>> = vec![x[self.nn..].to_vec()];
        while t < tstop * (1.0 - 1e-12) {
            while bp_i < bps.len() && bps[bp_i] <= t * (1.0 + 1e-12) + 1e-30 {
                bp_i += 1;
            }
            let next_bp = bps.get(bp_i).copied().unwrap_or(tstop);
            let mut hs = h.min(hmax).min(next_bp - t);
            let mut tries = 0;
            let mut rejects = 0;
            let (xn, iters, lte, rq) = loop {
                // Meyer capacitance at the step end from the solution extrapolated over the last step,
                // fixed during the Newton iteration so the charge stays linear within the step.
                let xp: Vec<f64> = match hist.len() {
                    n if n >= 2 => {
                        let ((t0, x0, _), (t1, x1, _)) = (&hist[n - 2], &hist[n - 1]);
                        let k = hs / (t1 - t0);
                        x1.iter().zip(x0).map(|(a, b)| a + (a - b) * k).collect()
                    }
                    _ => x.clone(),
                };
                for (k, cp) in self.caps.iter().enumerate() {
                    if let CapKind::Meyer { elem, part } = cp.kind {
                        state.cap_c_end[k] = self.meyer_at(elem, part, &xp);
                    }
                }
                let mut xt = x.clone();
                let mode = Mode::Tran { t: t + hs, h: hs, method, state: &state };
                match self.newton(&mut xt, mode, self.c.options.gmin, self.c.options.itl4) {
                    Ok(it) => {
                        let rq = self.reactive(&xt, Some((&state, method, hs)));
                        let del = self.lte_step(&hist, t + hs, &xt, &rq, method);
                        if let Some(d) = del
                            && d < 0.9 * hs
                            && rejects < 20
                            && hs > hmax * 1e-7
                        {
                            rejects += 1;
                            hs = d.max(hs / 8.0);
                            continue;
                        }
                        break (xt, it, del, rq);
                    }
                    Err(e) => {
                        tries += 1;
                        hs /= 8.0;
                        method = Integ::BackwardEuler;
                        if tries > 10 || hs < 1e-18 {
                            return Err(SimError::Engine(format!("transient failed at t={:.4e}: {e}", t)));
                        }
                    }
                }
            };
            // Update reactive element state.
            for k in 0..self.caps.len() {
                let cp = &self.caps[k];
                let (q, c) = self.cap_qc(k, &xn, Some(&state));
                let (i, _) = Self::cap_current(method, hs, q, c, &state, k);
                state.cap_q[k] = q;
                state.cap_i[k] = i;
                state.cap_v[k] = self.v(&xn, cp.a) - self.v(&xn, cp.b);
                if let CapKind::Meyer { elem, part } = cp.kind {
                    state.cap_c[k] = self.meyer_at(elem, part, &xn);
                }
            }
            for k in 0..self.qdevs.len() {
                let (q, _) = self.qdev_qc(k, &xn);
                for t in 0..4 {
                    state.qdev_i[k][t] = Self::charge_current(method, hs, q[t], state.qdev_q[k][t], state.qdev_i[k][t]);
                }
                state.qdev_q[k] = q;
            }
            for (k, &ei) in self.inductors.iter().enumerate() {
                if let Element::L { a, b, .. } = &self.c.elements[ei] {
                    state.ind_v[k] = self.v(&xn, *a) - self.v(&xn, *b);
                    state.ind_i[k] = xn[self.branch[ei].unwrap()];
                }
            }
            t += hs;
            x = xn;
            self.settle_switches(&x);
            self.push_point(&mut ds, Some(t), &x);
            steps += 1;
            ring.push(x[self.nn..].to_vec());
            if ring.len() > RING_FLIPS + 3 {
                ring.remove(0);
            }
            // Step control: the truncation error estimate sets the next step once three points
            // past a breakpoint exist; before that, and when Newton struggles, iteration counts do.
            let at_bp = (next_bp - t).abs() <= hmax * 1e-9;
            if at_bp {
                method = Integ::BackwardEuler;
                let gap = bps.get(bp_i + 1).copied().unwrap_or(tstop) - t;
                h = hs.min(0.1 * h.min(hmax).min(if gap > 0.0 { gap } else { hmax })).clamp(hmax * 1e-4, hmax / 20.0);
                hist.clear();
                ring.clear();
            } else {
                // A backward Euler step damps trapezoidal ringing in the branch currents.
                method = if self.rings(&ring) { Integ::BackwardEuler } else { Integ::Trapezoidal };
                if method == Integ::BackwardEuler {
                    ring.clear();
                }
                h = match lte {
                    Some(d) => d.min(2.0 * hs),
                    None if iters <= 4 => hs * 2.0,
                    None => hs,
                };
                if iters > 10 {
                    h = h.min(hs / 2.0);
                }
                if let Some(prev) = hist.last() {
                    h = h.min(self.switch_step(&prev.1, &x, hs).max(hmax * 1e-6));
                }
                h = h.min(hmax);
            }
            hist.push((t, x.clone(), rq));
            if hist.len() > 3 {
                hist.remove(0);
            }
            if steps > 5_000_000 {
                return Err(SimError::Engine("transient exceeded 5M steps".into()));
            }
        }
        Ok(ds)
    }

    /// Whether a branch current rings: its second differences over the accepted points `pts`
    /// (branch currents only, oldest first) flip sign at every step and all exceed its truncation
    /// error tolerance. An edge flips them once or twice; trapezoidal ringing keeps flipping.
    fn rings(&self, pts: &[Vec<f64>]) -> bool {
        let o = &self.c.options;
        let Some(last) = pts.last() else { return false };
        (0..last.len()).any(|i| {
            let tol = o.trtol * (o.lte_reltol * last[i].abs() + o.abstol);
            let d: Vec<f64> = pts.windows(3).map(|w| w[2][i] - 2.0 * w[1][i] + w[0][i]).collect();
            d.len() > RING_FLIPS && d.windows(2).all(|p| p[0] * p[1] < 0.0) && d.iter().all(|v| v.abs() > tol)
        })
    }

    /// Charges (capacitors, charge-based device terminals) and fluxes (inductors) at `x`, each
    /// with its current (an inductor: its voltage), for the truncation error estimate. `step`:
    /// the state, method and length of the step that ends at `x` (none: the initial point).
    fn reactive(&self, x: &[f64], step: Option<(&TranState, Integ, f64)>) -> Vec<(f64, f64)> {
        let mut out = Vec::with_capacity(self.caps.len() + 4 * self.qdevs.len() + self.inductors.len());
        for k in 0..self.caps.len() {
            let (q, c) = self.cap_qc(k, x, step.map(|s| s.0));
            let i = step.map_or(0.0, |(st, m, h)| Self::cap_current(m, h, q, c, st, k).0);
            out.push((q, i));
        }
        for k in 0..self.qdevs.len() {
            let (q, _) = self.qdev_qc(k, x);
            for t in 0..4 {
                let i = step.map_or(0.0, |(st, m, h)| Self::charge_current(m, h, q[t], st.qdev_q[k][t], st.qdev_i[k][t]));
                out.push((q[t], i));
            }
        }
        let base = out.len();
        for &ei in &self.inductors {
            if let Element::L { a, b, l, .. } = &self.c.elements[ei] {
                out.push((l * x[self.branch[ei].unwrap()], self.v(x, *a) - self.v(x, *b)));
            }
        }
        for &(b1, b2, p1, p2, m) in &self.mutual {
            out[base + p1].0 += m * x[b2];
            out[base + p2].0 += m * x[b1];
        }
        out
    }

    /// The step the truncation error allows for a step to `tn` with reactive values `rq`, after
    /// the accepted points `hist` (oldest first), as ngspice's CKTterr: a divided difference of
    /// each charge of order one above the method's, against a tolerance of `abstol + reltol·|i|`
    /// on its current or `reltol·max(|q|, chgtol) / h` on its charge.
    fn lte_step(&self, hist: &[Point], tn: f64, xn: &[f64], rq: &[(f64, f64)], method: Integ) -> Option<f64> {
        let o = &self.c.options;
        let (order, factor) = match method {
            Integ::Trapezoidal => (2, 1.0 / 12.0),
            Integ::BackwardEuler => (1, 0.5),
        };
        if hist.len() < order + 1 {
            return None;
        }
        let pts = &hist[hist.len() - order - 1..];
        let mut ts: Vec<f64> = pts.iter().map(|p| p.0).collect();
        ts.push(tn);
        let h = tn - ts[order];
        let mut del = f64::INFINITY;
        let mut d = vec![0.0; order + 2];
        for (j, &(qn, inn)) in rq.iter().enumerate() {
            for (k, p) in pts.iter().enumerate() {
                d[k] = p.2[j].0;
            }
            d[order + 1] = qn;
            for lvl in 1..=order + 1 {
                for k in (lvl..=order + 1).rev() {
                    d[k] = (d[k] - d[k - 1]) / (ts[k] - ts[k - lvl]);
                }
            }
            let (qp, ip) = pts[order].2[j];
            let volttol = o.abstol + o.lte_reltol * inn.abs().max(ip.abs());
            let chargetol = o.lte_reltol * qn.abs().max(qp.abs()).max(o.chgtol) / h;
            let tol = volttol.max(chargetol);
            let dj = o.trtol * tol / o.abstol.max(factor * d[order + 1].abs());
            del = del.min(if order == 2 { dj.sqrt() } else { dj });
        }
        // Node voltages too, against a tolerance of at least a millivolt (finer, Newton noise on
        // nodes near 0 V reads as error): the difference to the quadratic extrapolation of the
        // last three points, a third of which is the step's error. It catches what the charges
        // miss, such as trapezoidal ringing in a MOS gate current.
        if order == 2 {
            let floor = 1e-3;
            let [(t0, x0, _), (t1, x1, _), (t2, x2, _)] = pts else { unreachable!() };
            let l0 = (tn - t1) * (tn - t2) / ((t0 - t1) * (t0 - t2));
            let l1 = (tn - t0) * (tn - t2) / ((t1 - t0) * (t1 - t2));
            let l2 = (tn - t0) * (tn - t1) / ((t2 - t0) * (t2 - t1));
            let r = (0..self.nn)
                .map(|i| {
                    let pred = l0 * x0[i] + l1 * x1[i] + l2 * x2[i];
                    let tol = o.trtol * (o.lte_reltol * xn[i].abs().max(x2[i].abs()) + floor);
                    (xn[i] - pred).abs() / 3.0 / tol
                })
                .fold(0.0, f64::max);
            if r > 0.0 {
                del = del.min(h * r.cbrt().recip());
            }
        }
        del.is_finite().then_some(del)
    }

    pub fn run(&mut self, a: &Analysis) -> Result<Dataset, SimError> {
        // Each analysis starts from the switches' initial states.
        *self.sw_on.borrow_mut() = self.c.elements.iter().map(|e| matches!(e, Element::S { on: true, .. })).collect();
        let num = |s: &str| parse_spice(s).ok_or_else(|| SimError::Engine(format!("invalid number '{s}'")));
        match a {
            Analysis::Op => self.run_op(),
            Analysis::Dc { source, start, stop, step } => self.run_dc(source, num(start)?, num(stop)?, num(step)?),
            Analysis::Ac { points_per_decade, fstart, fstop, sweep } => {
                self.run_ac_sweep(*sweep, *points_per_decade, num(fstart)?, num(fstop)?)
            }
            Analysis::Tran { tstep, tstop, tstart, tmax, .. } => {
                let opt = |s: &String| if s.trim().is_empty() { Ok(None) } else { num(s).map(Some) };
                let (tstart, tmax) = (opt(tstart)?, opt(tmax)?);
                let mut ds = self.run_tran_limited(num(tstep)?, num(tstop)?, tmax)?;
                if let Some(t0) = tstart.filter(|t| *t > 0.0) {
                    let keep: Vec<bool> = ds.vectors[0].re.iter().map(|t| *t >= t0).collect();
                    for v in &mut ds.vectors {
                        let mut k = keep.iter();
                        v.re.retain(|_| *k.next().unwrap_or(&true));
                        if let Some(im) = &mut v.im {
                            let mut k = keep.iter();
                            im.retain(|_| *k.next().unwrap_or(&true));
                        }
                    }
                }
                Ok(ds)
            }
            Analysis::Noise { output, source, points_per_decade, fstart, fstop } => {
                self.run_noise(output, source, *points_per_decade, num(fstart)?, num(fstop)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bipolar_transistors() {
        // A diode-connected NPN at 100 µA: vbe = vt ln(ic / is) for an ideal device, and the
        // mirrored collector carries the same current; beta = bf.
        let deck = "* t\nI1 0 b 100u\nQ1 b b 0 qn\nQ2 c b 0 qn\nV2 c 0 1\n.model qn npn (is=1e-16 bf=100)\n.end";
        let mut e = engine(deck);
        let op = e.run_op().unwrap();
        let vbe = op.get("v(b)").unwrap().re[0];
        let vt = 8.617333262e-5 * 300.15;
        let ic = 100e-6 * 100.0 / 102.0; // two base currents from the reference
        assert!((vbe - vt * (ic / 1e-16f64).ln()).abs() < 1e-3, "vbe {vbe}");
        let i2 = -op.get("i(v2)").unwrap().re[0];
        assert!((i2 / ic - 1.0).abs() < 1e-3, "mirrored {i2} vs {ic}");
        // PNP: the same circuit upside down.
        let pnp = "* t\nV1 vcc 0 2\nI1 b 0 100u\nQ1 b b vcc qp\n.model qp pnp (is=1e-16 bf=100)\n.end";
        let op = engine(pnp).run_op().unwrap();
        let veb = 2.0 - op.get("v(b)").unwrap().re[0];
        assert!((veb - vt * (100e-6 * 100.0 / 101.0 / 1e-16f64).ln()).abs() < 1e-3, "veb {veb}");
        // Derivatives match finite differences (Early effect, knee, leakage on).
        let m = BjtModel {
            pnp: false,
            is: 1e-15,
            bf: 150.0,
            br: 2.0,
            nf: 1.0,
            nr: 1.0,
            vaf: 60.0,
            var: 20.0,
            ikf: 0.02,
            ikr: 0.01,
            ise: 1e-14,
            ne: 1.6,
            isc: 1e-15,
            nc: 2.0,
            rb: 0.0,
            re: 0.0,
            rc: 0.0,
            cje: 0.0,
            vje: 0.75,
            mje: 0.33,
            cjc: 0.0,
            vjc: 0.75,
            mjc: 0.33,
            fc: 0.5,
            tf: 0.0,
            tr: 0.0,
            kf: 0.0,
            af: 1.0,
            eg: 1.11,
            xti: 3.0,
            xtb: 0.0,
            tnom: 27.0,
        };
        let h = 1e-7;
        for (vbe, vbc) in [(0.7, -2.0), (0.8, 0.5), (0.3, -1.0)] {
            let ev = bjt_eval(&m, 1.0, vbe, vbc, vt);
            let f = |a, b| bjt_eval(&m, 1.0, a, b, vt);
            let num = (f(vbe + h, vbc).ic - f(vbe - h, vbc).ic) / (2.0 * h);
            assert!((ev.dic[0] - num).abs() <= 1e-4 * num.abs().max(1e-12), "{vbe} {vbc}: {} vs {num}", ev.dic[0]);
            let num = (f(vbe, vbc + h).ib - f(vbe, vbc - h).ib) / (2.0 * h);
            assert!((ev.dib[1] - num).abs() <= 1e-4 * num.abs().max(1e-12), "{vbe} {vbc}: {} vs {num}", ev.dib[1]);
        }
    }

    #[test]
    fn controlled_sources() {
        // 1 mA through Vs (into a 1k load) senses F and H; E and G sense v(in) = 1 V.
        let deck = "* t\nV1 in 0 dc 1 ac 1\nVs in s 0\nRs s 0 1k\nE1 e 0 in 0 2\nRe e 0 1k\nG1 0 g in 0 1m\nRg g 0 2k\n\
                    F1 0 f Vs 3\nRf f 0 1k\nH1 h 0 Vs 500\nRh h 0 1k\n.end";
        let op = engine(deck).run_op().unwrap();
        let v = |n: &str| op.get(n).unwrap().re[0];
        assert!((v("v(e)") - 2.0).abs() < 1e-6, "VCVS");
        assert!((v("v(g)") - 2.0).abs() < 1e-6, "VCCS 1 mS into 2k");
        assert!((v("v(f)") - 3.0).abs() < 1e-6, "CCCS 3 x 1 mA into 1k");
        assert!((v("v(h)") - 0.5).abs() < 1e-6, "CCVS 500 ohm x 1 mA");
        let ac = engine(deck).run_ac(1, 1e3, 1e3).unwrap();
        assert!((ac.get("v(f)").unwrap().re[0] - 3.0).abs() < 1e-6, "the same in AC");
        assert!(parse_deck("* t\nE1 a 0 POLY(1) b 0 0 1\nR1 a 0 1\n.end").is_err());
        assert!(Engine::new(parse_deck("* t\nF1 a 0 Vx 1\nR1 a 0 1\n.end").unwrap()).is_err(), "no Vx");
    }

    #[test]
    fn off_stack_internal_node_sits_at_the_bulk() {
        // Both NMOS off: only gmin defines `mid`. As in ngspice it goes across the bulk
        // junctions, so `mid` sits at the bulk (0 V), not between vdd and ground.
        let d = "* t\nV1 vdd 0 1.8\nM1 vdd 0 mid 0 nmos w=1u l=1u\nM2 mid 0 0 0 nmos w=1u l=1u\n\
                 .model nmos nmos (level=1 vto=0.45 kp=280u)\n.end";
        let v = engine(d).run_op().unwrap().get("v(mid)").unwrap().re[0];
        assert!(v.abs() < 1e-3, "{v}");
    }

    #[test]
    fn switch_hysteresis_keeps_its_state() {
        // vt 0.5, vh 0.2: closes above 0.7, opens below 0.3. At 0.5 the state decides.
        let d = |vc: f64, init: &str| {
            format!("* t\nV1 a 0 1\nVc c 0 {vc}\nS1 a o c 0 sw {init}\nR1 o 0 1k\n.model sw sw vt=0.5 vh=0.2 ron=1 roff=1g\n.end")
        };
        let v = |deck: String| engine(&deck).run_op().unwrap().get("v(o)").unwrap().re[0];
        assert!(v(d(0.5, "off")) < 1e-5 && v(d(0.5, "on")) > 0.99);
        assert!(v(d(0.8, "off")) > 0.99 && v(d(0.2, "on")) < 1e-5);
    }

    #[test]
    fn behavioural_sources_switches_and_coupling() {
        let deck = "* t\nV1 in 0 2\nVs in s 0\nRs s 0 1k\nB1 b 0 V={v(in)*v(in) - 1}\nRb b 0 1k\n\
                    B2 0 c I=i(Vs)*2\nRc c 0 1k\n.end";
        let op = engine(deck).run_op().unwrap();
        let v = |n: &str| op.get(n).unwrap().re[0];
        assert!((v("v(b)") - 3.0).abs() < 1e-6, "v(in)^2 - 1");
        assert!((v("v(c)") - 4.0).abs() < 1e-6, "2 x 2 mA into 1k");
        for bad in ["B1 a 0 v(a)", "B1 a 0 V=foo(v(a))", "B1 a 0 V=v(a", "S1 a 0 c 0 nomodel", "W1 a 0 V1 sw\n.model sw sw"] {
            assert!(parse_deck(&format!("* t\nV1 c 0 1\n{bad}\nR1 a 0 1\n.end")).is_err(), "{bad}");
        }
        assert!(Engine::new(parse_deck("* t\nB1 a 0 V=i(vx)\nR1 a 0 1\n.end").unwrap()).is_err(), "no vx");
        assert!(Engine::new(parse_deck("* t\nV1 a 0 1\nL1 a 0 1u\nK1 L1 L9 0.5\n.end").unwrap()).is_err(), "no L9");
        assert!(Engine::new(parse_deck("* t\nV1 a 0 1\nL1 a 0 1u\nL2 a 0 1u\nK1 L1 L2 1.5\n.end").unwrap()).is_err(), "|k| > 1");
        // A voltage switch: on above vt, off below.
        let sw = |vc: f64| {
            let d = format!("* t\nV1 a 0 1\nVc c 0 {vc}\nS1 a o c 0 sw\nR1 o 0 1k\n.model sw sw vt=0.5 ron=1 roff=1g\n.end");
            engine(&d).run_op().unwrap().get("v(o)").unwrap().re[0]
        };
        assert!(sw(1.0) > 0.99 && sw(0.0) < 1e-5);
    }
    use crate::circuit::parse_deck;

    fn engine(deck: &str) -> Engine {
        Engine::new(parse_deck(deck).unwrap()).unwrap()
    }

    #[test]
    fn divider_op() {
        let mut e = engine("* t\nV1 a 0 10\nR1 a b 1k\nR2 b 0 3k\n.end");
        let ds = e.run_op().unwrap();
        let vb = ds.get("v(b)").unwrap().re[0];
        assert!((vb - 7.5).abs() < 1e-6, "{vb}");
        let i = ds.get("i(v1)").unwrap().re[0];
        assert!((i + 2.5e-3).abs() < 1e-9, "{i}");
    }

    #[test]
    fn rc_ac_corner() {
        let mut e = engine("* t\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.end");
        let fc = 1.0 / (2.0 * std::f64::consts::PI * 1e-3);
        let ds = e.run_ac(100, fc, fc * 1.001).unwrap();
        let db = ds.get("v(out)").unwrap().db()[0];
        assert!((db + 3.0103).abs() < 0.01, "{db}");
    }

    #[test]
    fn mos_junction_caps() {
        let deck = |j: &str| {
            format!(
                "* t\nV1 in 0 DC 0 AC 1\nR1 in out 1meg\nM1 out 0 0 0 nm w=1u l=1u {j}\n.model nm nmos (vto=0.7 cj=1m cjsw=0.5n)\n.end"
            )
        };
        let fc = 1.0 / (2.0 * std::f64::consts::PI * 1e6 * 2e-15);
        let db = |j: &str| engine(&deck(j)).run_ac(100, fc, fc * 1.001).unwrap().get("v(out)").unwrap().db()[0];
        assert!((db("ad=1p pd=2u") + 3.0103).abs() < 0.01);
        assert!((db("as=1p ps=2u") + 3.0103).abs() > 1.0, "the source junction is shorted to the bulk");
        assert!(db("").abs() < 0.01);
    }

    #[test]
    fn rc_tran_step() {
        let mut e = engine("* t\nV1 in 0 DC 0 PULSE(0 1 0 1n 1n 1 2)\nR1 in out 1k\nC1 out 0 1n\n.end");
        let ds = e.run_tran(10e-9, 5e-6).unwrap();
        let t = &ds.vectors[0].re;
        let v = &ds.get("v(out)").unwrap().re;
        // find value at ~1 tau
        let k = t.iter().position(|&x| x >= 1e-6).unwrap();
        let expected = 1.0 - (-(t[k] - 0.5e-9) / 1e-6f64).exp();
        assert!((v[k] - expected).abs() < 0.01, "{} vs {expected}", v[k]);
    }

    #[test]
    fn inverter_dc_transfer() {
        let deck = format!(
            "* inv\nV1 vdd 0 1.8\nV2 in 0 0\nM1 out in vdd vdd pmos w=2u l=180n\nM2 out in 0 0 nmos w=1u l=180n\n{}\n.end",
            ics_core::netlist::DEFAULT_MODELS
        );
        let mut e = engine(&deck);
        let ds = e.run_dc("v2", 0.0, 1.8, 0.05).unwrap();
        let out = &ds.get("v(out)").unwrap().re;
        assert!(out[0] > 1.75, "{}", out[0]);
        assert!(*out.last().unwrap() < 0.05, "{}", out.last().unwrap());
    }

    #[test]
    fn level1_currents_match_the_square_law() {
        let id = |vg: f64, vd: f64| {
            let deck = format!(
                "* m\nVD d 0 {vd}\nVG g 0 {vg}\nM1 d g 0 0 nm w=2u l=1u\n.model nm nmos (vto=0.5 kp=100u lambda=0.1)\n.end"
            );
            -engine(&deck).run_op().unwrap().get("i(vd)").unwrap().re[0]
        };
        let beta = 100e-6 * 2.0;
        let sat = 0.5 * beta * 0.5f64.powi(2) * (1.0 + 0.1 * 1.5);
        assert!((id(1.0, 1.5) - sat).abs() < 1e-9 * sat.max(1.0) + 1e-12, "{} vs {sat}", id(1.0, 1.5));
        let tri = beta * (0.5 * 0.2 - 0.5 * 0.04) * (1.0 + 0.1 * 0.2);
        assert!((id(1.0, 0.2) - tri).abs() < 1e-6 * tri, "{} vs {tri}", id(1.0, 0.2));
        assert!(id(0.4, 1.0).abs() < 1e-9);
    }

    #[test]
    fn depletion_charge_is_continuous_and_its_derivative_is_the_capacitance() {
        let (cj0, m, pb, fc) = (1e-12, 0.4, 0.8, 0.5);
        for v in [-3.0, -0.5, 0.0, 0.3, 0.399, 0.401, 0.6, 1.0] {
            let h = 1e-6;
            let (q1, _) = depletion(cj0, m, pb, fc, v - h);
            let (q2, _) = depletion(cj0, m, pb, fc, v + h);
            let (_, c) = depletion(cj0, m, pb, fc, v);
            assert!(((q2 - q1) / (2.0 * h) - c).abs() < 1e-6 * c, "v={v}: {} vs {c}", (q2 - q1) / (2.0 * h));
        }
        assert_eq!(depletion(cj0, m, pb, fc, 0.0), (0.0, cj0));
    }

    #[test]
    fn meyer_capacitances_by_region() {
        let model = crate::circuit::MosModel { tox: 10e-9, vto: 0.5, ..dummy_mos() };
        let cox = model.cox() * 1e-6 * 1e-6;
        let c = |vd: f64, vg: f64| meyer(&model, 1e-6, 1e-6, 1.0, vd, vg, 0.0, 0.0);
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9 * cox);
        assert!(close(c(0.0, 1.5), [cox / 2.0, cox / 2.0, 0.0]), "triode at vds=0: {:?}", c(0.0, 1.5));
        assert!(close(c(1.8, 1.0), [2.0 * cox / 3.0, 0.0, 0.0]), "saturation: {:?}", c(1.8, 1.0));
        assert!(close(c(1.0, -1.0), [0.0, 0.0, cox]), "accumulation: {:?}", c(1.0, -1.0));
        // Source and drain swap with the bias: cgd takes the saturated 2/3 Cox.
        assert!(close(meyer(&model, 1e-6, 1e-6, 1.0, 0.0, 1.0, 1.8, 0.0), [0.0, 2.0 * cox / 3.0, 0.0]));
    }

    fn dummy_mos() -> crate::circuit::MosModel {
        match parse_deck("* t\n.model nm nmos ()\nM1 d g 0 0 nm\n.end").unwrap().elements.pop() {
            Some(Element::M { model, .. }) => model,
            _ => unreachable!(),
        }
    }

    #[test]
    fn dc_sweeps_include_the_stop_value() {
        let ds = engine("* t\nV1 a 0 0\nR1 a 0 1k\n.end").run_dc("V1", -1.2, 1.2, 0.05).unwrap();
        let v = &ds.get("v1").unwrap().re;
        assert_eq!(v.len(), 49);
        assert!((v[48] - 1.2).abs() < 1e-12, "{}", v[48]);
    }

    #[test]
    fn level1_charge_model_conserves_the_channel_charge() {
        // A gate pulse over a MOS whose drain and source float on 10 fF: once the gate is back
        // at 0 V the channel is empty, so its electrons are back on the drain and the source.
        // Meyer capacitances lose that charge; the charge model (capmod=1) keeps it.
        let deck = |capmod: u8| {
            format!(
                "* t\nVG g 0 PULSE(0 1.8 10n 1n 1n 20n 1)\nVB b 0 0.5\nRD d b 1e9\nRS s b 1e9\nCD d 0 10f\nCS s 0 10f\n\
                 M1 d g s 0 nm w=1u l=1u\n.model nm nmos (level=1 vto=0.45 kp=100u tox=4n capmod={capmod})\n.end"
            )
        };
        // Drain voltage before the pulse and 30 ns after it.
        let drift = |capmod: u8| {
            let ds = engine(&deck(capmod)).run_tran(0.1e-9, 60e-9).unwrap();
            let v = &ds.get("v(d)").unwrap().re;
            v[v.len() - 1] - v[0]
        };
        // 1 GOhm leaks 0.25 nA for 20 ns while the drain is high: 0.5 mV.
        let kept = drift(1);
        assert!(kept.abs() < 1e-3, "capmod=1 drifts {kept}");
        // Meyer's error depends on where the time points fall, so its sign and size differ
        // between platforms (libm rounding moves the steps): only its size is checked.
        let meyer = drift(0);
        assert!(meyer.abs() > 0.01, "Meyer was expected to lose charge conservation: {meyer}");
    }

    #[test]
    fn nonlinear_junction_charge_is_conserved() {
        // 1 uA for 1 us into a reverse-biased m=0.5, vj=1 junction of 1 pF: 2 cj0 (sqrt(1 + v) - 1) = I t
        // gives v = 1.25 V where a linear 1 pF would reach 1 V.
        let deck = "* t\nI1 0 a PULSE(0 1u 0 1p 1p 1 2)\nD1 0 a dm\n.model dm d (is=1e-30 cjo=1p vj=1 m=0.5)\n.end";
        let ds = engine(deck).run_tran(5e-9, 1e-6).unwrap();
        let v = *ds.get("v(a)").unwrap().re.last().unwrap();
        assert!((v - 1.25).abs() < 0.005, "{v}");
    }

    #[test]
    fn gate_capacitance_sets_the_input_pole() {
        // Saturated 10u/10u NMOS with tox=10n: Cin = 2/3 Cox W L = 230 fF behind 1 MOhm.
        let deck = "* t\nVD d 0 1.8\nVG in 0 DC 1 AC 1\nRG in g 1meg\nM1 d g 0 0 nm w=10u l=10u\n.model nm nmos (vto=0.5 kp=100u tox=10n)\n.end";
        let cin = 2.0 / 3.0 * 3.453e-11 / 10e-9 * 1e-10;
        let fc = 1.0 / (2.0 * std::f64::consts::PI * 1e6 * cin);
        let db = engine(deck).run_ac(10, fc, fc * 1.001).unwrap().get("v(g)").unwrap().db()[0];
        assert!((db + 3.0103).abs() < 0.02, "{db}");
    }

    const KT: f64 = BOLTZMANN * 300.15;

    #[test]
    fn divider_thermal_noise() {
        let mut e = engine("* t\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nR2 out 0 1k\n.end");
        let ds = e.run_noise("out", "v1", 1, 1e3, 1e4).unwrap();
        let on = ds.get("onoise_spectrum").unwrap().re[0];
        let expect = (4.0 * KT * 500.0).sqrt();
        assert!((on / expect - 1.0).abs() < 1e-6, "{on} vs {expect}");
        let inn = ds.get("inoise_spectrum").unwrap().re[0];
        assert!((inn / (2.0 * expect) - 1.0).abs() < 1e-6, "{inn}");
    }

    #[test]
    fn rc_noise_integrates_to_kt_over_c() {
        let mut e = engine("* t\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1p\n.end");
        let ds = e.run_noise("out", "v1", 50, 1e3, 1e14).unwrap();
        let (f, s) = (&ds.vectors[0].re, &ds.get("onoise_spectrum").unwrap().re);
        let total: f64 = (1..f.len()).map(|k| 0.5 * (s[k] * s[k] + s[k - 1] * s[k - 1]) * (f[k] - f[k - 1])).sum();
        let expect = KT / 1e-12;
        assert!((total / expect - 1.0).abs() < 0.01, "{total} vs {expect}");
    }

    #[test]
    fn mos_flicker_noise_falls_as_one_over_f() {
        let deck = "* t\nVDD vdd 0 1.8\nVIN g 0 DC 1 AC 1\nRD vdd d 10k\nM1 d g 0 0 nm w=10u l=1u\n\
                    .model nm nmos (vto=0.5 kp=100u tox=10n kf=1e-24)\n.end";
        let ds = engine(deck).run_noise("d", "vin", 1, 1.0, 10.0).unwrap();
        let s = &ds.get("onoise_spectrum").unwrap().re;
        let ratio = (s[0] / s[1]).powi(2);
        assert!((ratio - 10.0).abs() < 0.2, "PSD ratio over a decade {ratio}");
    }

    #[test]
    fn temperature_scales_threshold_mobility_and_diodes() {
        let mos = |t: f64| {
            let deck = format!(
                "* t\n.temp {t}\n.model nm nmos (vto=0.5 kp=100u gamma=0.4 phi=0.7)\nVD d 0 1.8\nVG g 0 1.8\nM1 d g 0 0 nm w=1u l=1u\n.op\n.end"
            );
            let ds = engine(&deck).run_op().unwrap();
            let get = |k: &str| ds.devices[0].values.iter().find(|v| v.0 == k).unwrap().1;
            (get("vth"), get("id"))
        };
        let ((vth27, id27), (vth127, id127)) = (mos(27.0), mos(127.0));
        assert!((vth27 - 0.5).abs() < 1e-12, "nominal threshold unchanged: {vth27}");
        let dvdt = (vth127 - vth27) / 100.0;
        assert!((-3e-3..-0.5e-3).contains(&dvdt), "threshold tempco {dvdt} V/K");
        assert!(id127 < id27, "mobility loss dominates in strong inversion: {id27} -> {id127}");
        let vd = |t: f64| {
            let deck = format!("* d\n.temp {t}\nI1 0 k 1m\nD1 k 0 dm\n.model dm d (is=1e-14)\n.op\n.end");
            engine(&deck).run_op().unwrap().get("v(k)").unwrap().re[0]
        };
        let dvdt = (vd(77.0) - vd(27.0)) / 50.0;
        assert!((-2.5e-3..-1.5e-3).contains(&dvdt), "diode forward voltage tempco {dvdt} V/K");
    }

    #[test]
    fn diode_forward() {
        let deck = "* d\nV1 a 0 5\nR1 a k 1k\nD1 k 0 dm\n.model dm d (is=1e-14 n=1)\n.end";
        let mut e = engine(deck);
        let ds = e.run_op().unwrap();
        let vk = ds.get("v(k)").unwrap().re[0];
        assert!(vk > 0.6 && vk < 0.75, "{vk}");
    }

    #[test]
    fn truncation_error_control_resolves_fast_edges_under_a_coarse_tstep() {
        // tau = 1 ns but tstep = 10 ns: only the error estimate keeps the steps small enough.
        let mut e = engine("* t\nV1 in 0 DC 0 PULSE(0 1 0 1p 1p 1 2)\nR1 in out 1k\nC1 out 0 1p\n.end");
        let ds = e.run_tran(10e-9, 1e-6).unwrap();
        let (t, v) = (&ds.vectors[0].re, &ds.get("v(out)").unwrap().re);
        for &at in &[0.5e-9, 1e-9, 2e-9, 4e-9] {
            let k = t.partition_point(|&x| x < at);
            let f = (at - t[k - 1]) / (t[k] - t[k - 1]);
            let got = v[k - 1] + f * (v[k] - v[k - 1]);
            let exact = 1.0 - (-(at - 0.5e-12) / 1e-9f64).exp();
            // ngspice's own points on this deck are off by up to 0.014 near 2 ns.
            assert!((got - exact).abs() < 0.02, "t={at:e}: {got} vs {exact}");
        }
        assert!(t.len() < 2000, "steps grow again once settled: {}", t.len());
    }

    #[test]
    fn lc_tran_runs() {
        let mut e = engine("* lc\nI1 0 a PULSE(0 1m 0 1n 1n 1u 2u)\nL1 a 0 1u\nC1 a 0 1n\nR1 a 0 1k\n.end");
        let ds = e.run_tran(1e-9, 1e-6).unwrap();
        assert!(ds.len() > 100);
    }
}
