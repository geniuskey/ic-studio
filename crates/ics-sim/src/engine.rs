//! Built-in Modified Nodal Analysis engine: OP, DC sweep, AC and transient.
//!
//! Intended for quick, dependency-free simulation of small circuits. For production
//! accuracy and full device models, use the ngspice backend.

use num_complex::Complex64;

use ics_core::Analysis;
use ics_core::units::parse_spice;

use crate::SimError;
use crate::circuit::{Circuit, DiodeModel, Element, MosModel, Node};
use crate::linalg::{Matrix, solve};
use crate::results::{Dataset, DeviceOp, VecKind, Vector};

const GMIN: f64 = 1e-12;
const ABSTOL_V: f64 = 1e-6;
const ABSTOL_I: f64 = 1e-12;
const RELTOL: f64 = 1e-4;
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
    /// all terminal voltages, so within a time step it is held at its value at the step start.
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
    ind_v: Vec<f64>,
    ind_i: Vec<f64>,
}

#[derive(Clone, Copy)]
enum Mode<'s> {
    Dc { src_scale: f64 },
    Tran { t: f64, h: f64, method: Integ, state: &'s TranState },
}

pub struct Engine {
    c: Circuit,
    nn: usize,
    /// Branch row for each element (V sources and inductors).
    branch: Vec<Option<usize>>,
    size: usize,
    caps: Vec<Cap>,
    inductors: Vec<usize>,
    vt: f64,
    pub log: Vec<String>,
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

fn mos_eval(m: &MosModel, w: f64, l: f64, mult: f64, vd: f64, vg: f64, vs: f64, vb: f64) -> MosEval {
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

impl Engine {
    pub fn new(mut c: Circuit) -> Result<Self, SimError> {
        let temp = c.temp;
        for e in &mut c.elements {
            match e {
                Element::M { model, .. } => *model = model.at_temp(temp),
                Element::D { model, .. } => *model = model.at_temp(temp),
                _ => {}
            }
        }
        let nn = c.node_count() - 1;
        let mut branch = vec![None; c.elements.len()];
        let mut size = nn;
        let mut caps = vec![];
        let mut inductors = vec![];
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
                Element::C { a, b, c, .. } => caps.push(Cap { a: *a, b: *b, kind: CapKind::Linear(*c) }),
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
                    if model.cox() > 0.0 {
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
        Ok(Self { c, nn, branch, size, caps, inductors, vt, log: vec![] })
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
    /// Meyer capacitance stays at its step-start value; without one it is evaluated at `x`.
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
                Some(s) => (s.cap_q[k] + s.cap_c[k] * (v - s.cap_v[k]), s.cap_c[k]),
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
            Mode::Dc { src_scale } => (None, src_scale),
            Mode::Tran { t, .. } => (Some(t), 1.0),
        };
        for n in 0..self.nn {
            m.add(n, n, gmin);
        }
        for (ei, e) in self.c.elements.iter().enumerate() {
            match e {
                Element::R { a, b, r, .. } => cond(&mut m, *a, *b, 1.0 / r),
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
                    let vd = self.v(x, *a) - self.v(x, *k);
                    let (id, gd) = diode_eval(model, vd, self.vt);
                    cond(&mut m, *a, *k, gd);
                    cur(&mut rhs, *a, *k, id - gd * vd);
                }
                Element::M { d, g, s, b, model, w, l, m: mult, junctions, .. } => {
                    for (an, ca, is) in bulk_diodes(model, *d, *s, *b, *mult, junctions) {
                        let vd = self.v(x, an) - self.v(x, ca);
                        let (id, gd) = exp_iv(is, self.vt, vd);
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
                    // small drain-source conductance for robustness
                    cond(&mut m, *d, *s, gmin);
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
        }
        (m, rhs)
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
            let (mut m, mut rhs) = self.assemble(x, mode, gmin);
            if !solve(&mut m, &mut rhs) {
                return Err(SimError::Engine("singular matrix (floating node or voltage source loop?)".into()));
            }
            let mut converged = true;
            for i in 0..self.size {
                let mut dx = rhs[i] - x[i];
                let is_v = i < self.nn;
                if is_v && dx.abs() > 0.5 {
                    dx = 0.5 * dx.signum(); // damp large voltage steps
                    converged = false;
                }
                let tol = if is_v { ABSTOL_V } else { ABSTOL_I } + RELTOL * rhs[i].abs().max(x[i].abs());
                if dx.abs() > tol {
                    converged = false;
                }
                x[i] += dx;
            }
            if !x.iter().all(|v| v.is_finite()) {
                return Err(SimError::Engine("numerical overflow".into()));
            }
            if converged && it > 0 {
                return Ok(it + 1);
            }
        }
        Err(SimError::NoConvergence)
    }

    /// DC operating point with gmin and source stepping fallbacks.
    pub fn op(&mut self, x0: Option<&[f64]>) -> Result<Vec<f64>, SimError> {
        let mut x = x0.map(|v| v.to_vec()).unwrap_or_else(|| vec![0.0; self.size]);
        if self.newton(&mut x, Mode::Dc { src_scale: 1.0 }, GMIN, 150).is_ok() {
            return Ok(x);
        }
        self.log.push("OP: direct Newton failed, trying gmin stepping".into());
        let mut x = vec![0.0; self.size];
        let mut ok = true;
        for k in (0..=10).rev() {
            let g = GMIN * 10f64.powi(k);
            if self.newton(&mut x, Mode::Dc { src_scale: 1.0 }, g, 150).is_err() {
                ok = false;
                break;
            }
        }
        if ok {
            return Ok(x);
        }
        self.log.push("OP: gmin stepping failed, trying source stepping".into());
        let mut x = vec![0.0; self.size];
        for k in 0..=20 {
            let s = k as f64 / 20.0;
            self.newton(&mut x, Mode::Dc { src_scale: s }, GMIN, 200)?;
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
                    let [cgs, cgd, cgb] = meyer(model, *w, *l, *m, v(*d), v(*g), v(*s), v(*b));
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
                            ("cgs".into(), cgs + model.cgso * w * m),
                            ("cgd".into(), cgd + model.cgdo * w * m),
                            ("cgb".into(), cgb + model.cgbo * model.leff(*l) * m),
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
        let n = ((stop - start) / step).abs().floor() as usize + 1;
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
            m.add(n, n, Complex64::new(GMIN, 0.0));
        }
        for (ei, e) in self.c.elements.iter().enumerate() {
            match e {
                Element::R { a, b, r, .. } => y(&mut m, *a, *b, Complex64::new(1.0 / r, 0.0)),
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
                    for (an, ca, is) in bulk_diodes(model, *d, *s, *b, *mult, junctions) {
                        let (_, gd) = exp_iv(is, self.vt, self.v(x0, an) - self.v(x0, ca));
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
                    y(&mut m, *d, *s, Complex64::new(GMIN, 0.0));
                }
            }
        }
        for (cp, c) in self.caps.iter().zip(caps_at_op) {
            y(&mut m, cp.a, cp.b, Complex64::new(0.0, w * c));
        }
        (m, rhs)
    }

    fn log_frequencies(ppd: u32, fstart: f64, fstop: f64) -> Vec<f64> {
        let npts = ((fstop / fstart).log10() * ppd as f64).ceil().max(1.0) as usize + 1;
        (0..npts).map(|p| fstart * 10f64.powf(p as f64 / ppd as f64)).take_while(|f| *f <= fstop * 1.000001).collect()
    }

    pub fn run_ac(&mut self, ppd: u32, fstart: f64, fstop: f64) -> Result<Dataset, SimError> {
        let x0 = self.op(None)?;
        let caps_at_op: Vec<f64> = (0..self.caps.len()).map(|k| self.cap_qc(k, &x0, None).1).collect();
        let mut ds = self.new_dataset("ac", "AC analysis", Some(("frequency", VecKind::Frequency)));
        for v in ds.vectors.iter_mut().skip(1) {
            v.im = Some(vec![]);
        }
        for f in Self::log_frequencies(ppd, fstart, fstop) {
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
                Element::D { a, k, model, .. } => {
                    let (id, _) = diode_eval(model, self.v(x0, *a) - self.v(x0, *k), self.vt);
                    let flicker = model.kf * id.abs().powf(model.af) / f;
                    out.push((*a, *k, 2.0 * CHARGE * id.abs() + flicker));
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
            let mut mt = m.clone();
            for r in 0..self.size {
                for c in 0..self.size {
                    mt.a[r * self.size + c] = m.a[c * self.size + r];
                }
            }
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
        let hmax = tstep.min(tstop / 100.0);
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
            let r = self.op(None);
            self.c.elements = saved;
            r?
        };
        let mut x = x;
        let qc: Vec<(f64, f64)> = (0..self.caps.len()).map(|k| self.cap_qc(k, &x, None)).collect();
        let mut state = TranState {
            cap_q: qc.iter().map(|p| p.0).collect(),
            cap_c: qc.iter().map(|p| p.1).collect(),
            cap_v: self.caps.iter().map(|c| self.v(&x, c.a) - self.v(&x, c.b)).collect(),
            cap_i: vec![0.0; self.caps.len()],
            ind_v: vec![0.0; self.inductors.len()],
            ind_i: self.inductors.iter().map(|&i| x[self.branch[i].unwrap()]).collect(),
        };
        let mut ds = self.new_dataset("tran", "Transient", Some(("time", VecKind::Time)));
        self.push_point(&mut ds, Some(0.0), &x);

        let mut t = 0.0;
        let mut h = hmax / 10.0;
        let mut method = Integ::BackwardEuler;
        let mut bp_i = 0;
        let mut steps = 0usize;
        // Accepted points since the last breakpoint, for the truncation error estimate.
        let mut hist: Vec<(f64, Vec<f64>)> = vec![(0.0, x.clone())];
        while t < tstop * (1.0 - 1e-12) {
            while bp_i < bps.len() && bps[bp_i] <= t * (1.0 + 1e-12) + 1e-30 {
                bp_i += 1;
            }
            let next_bp = bps.get(bp_i).copied().unwrap_or(tstop);
            let mut hs = h.min(hmax).min(next_bp - t);
            let mut tries = 0;
            let mut rejects = 0;
            let (xn, iters, lte) = loop {
                let mut xt = x.clone();
                let mode = Mode::Tran { t: t + hs, h: hs, method, state: &state };
                match self.newton(&mut xt, mode, GMIN, 60) {
                    Ok(it) => {
                        let r = if method == Integ::Trapezoidal { self.lte_ratio(&hist, t + hs, &xt) } else { None };
                        if let Some(r) = r
                            && r > 1.0
                            && rejects < 20
                            && hs > hmax * 1e-7
                        {
                            rejects += 1;
                            hs *= (0.9 * r.cbrt().recip()).max(0.25);
                            continue;
                        }
                        break (xt, it, r);
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
            for (k, &ei) in self.inductors.iter().enumerate() {
                if let Element::L { a, b, .. } = &self.c.elements[ei] {
                    state.ind_v[k] = self.v(&xn, *a) - self.v(&xn, *b);
                    state.ind_i[k] = xn[self.branch[ei].unwrap()];
                }
            }
            t += hs;
            x = xn;
            self.push_point(&mut ds, Some(t), &x);
            steps += 1;
            // Step control: the truncation error estimate sets the next step once three points
            // past a breakpoint exist; before that, and when Newton struggles, iteration counts do.
            let at_bp = (next_bp - t).abs() <= hmax * 1e-9;
            if at_bp {
                method = Integ::BackwardEuler;
                h = hs.clamp(hmax * 1e-4, hmax / 20.0);
                hist.clear();
            } else {
                method = Integ::Trapezoidal;
                h = match lte {
                    Some(r) => hs * (0.9 * r.max(1e-9).cbrt().recip()).min(2.0),
                    None if iters <= 4 => hs * 2.0,
                    None => hs,
                };
                if iters > 10 {
                    h = h.min(hs / 2.0);
                }
                h = h.min(hmax);
            }
            hist.push((t, x.clone()));
            if hist.len() > 3 {
                hist.remove(0);
            }
            if steps > 5_000_000 {
                return Err(SimError::Engine("transient exceeded 5M steps".into()));
            }
        }
        Ok(ds)
    }

    /// Local truncation error of a trapezoidal step to `(tn, xn)` relative to its tolerance (above
    /// 1: reject), from the difference to the quadratic extrapolation of the last three points.
    /// For the trapezoidal rule the error is about a third of that difference.
    fn lte_ratio(&self, hist: &[(f64, Vec<f64>)], tn: f64, xn: &[f64]) -> Option<f64> {
        const TRTOL: f64 = 7.0;
        const LTE_RELTOL: f64 = 1e-3;
        let [(t0, x0), (t1, x1), (t2, x2)] = hist else { return None };
        let l0 = (tn - t1) * (tn - t2) / ((t0 - t1) * (t0 - t2));
        let l1 = (tn - t0) * (tn - t2) / ((t1 - t0) * (t1 - t2));
        let l2 = (tn - t0) * (tn - t1) / ((t2 - t0) * (t2 - t1));
        let r = (0..self.nn)
            .map(|i| {
                let pred = l0 * x0[i] + l1 * x1[i] + l2 * x2[i];
                let tol = TRTOL * (LTE_RELTOL * xn[i].abs().max(x2[i].abs()) + ABSTOL_V);
                (xn[i] - pred).abs() / 3.0 / tol
            })
            .fold(0.0, f64::max);
        r.is_finite().then_some(r)
    }

    pub fn run(&mut self, a: &Analysis) -> Result<Dataset, SimError> {
        let num = |s: &str| parse_spice(s).ok_or_else(|| SimError::Engine(format!("invalid number '{s}'")));
        match a {
            Analysis::Op => self.run_op(),
            Analysis::Dc { source, start, stop, step } => self.run_dc(source, num(start)?, num(stop)?, num(step)?),
            Analysis::Ac { points_per_decade, fstart, fstop } => self.run_ac(*points_per_decade, num(fstart)?, num(fstop)?),
            Analysis::Tran { tstep, tstop } => self.run_tran(num(tstep)?, num(tstop)?),
            Analysis::Noise { output, source, points_per_decade, fstart, fstop } => {
                self.run_noise(output, source, *points_per_decade, num(fstart)?, num(fstop)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert!((got - exact).abs() < 0.01, "t={at:e}: {got} vs {exact}");
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
