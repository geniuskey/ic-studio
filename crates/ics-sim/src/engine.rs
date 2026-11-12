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
const EPS_OX: f64 = 3.453e-11;

#[derive(Clone, Copy)]
struct Cap {
    a: Node,
    b: Node,
    c: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Integ {
    BackwardEuler,
    Trapezoidal,
}

#[derive(Clone)]
struct TranState {
    cap_v: Vec<f64>,
    cap_i: Vec<f64>,
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

fn mos_eval(m: &MosModel, w: f64, l: f64, mult: f64, vd: f64, vg: f64, vs: f64, vb: f64) -> MosEval {
    let sign = if m.pmos { -1.0 } else { 1.0 };
    let (mut d, g, mut s, b) = (sign * vd, sign * vg, sign * vs, sign * vb);
    let swapped = d < s;
    if swapped {
        std::mem::swap(&mut d, &mut s);
    }
    let vgs = g - s;
    let vds = d - s;
    let vbs = b - s;
    let vto = sign * m.vto; // normalized threshold (positive for enhancement)
    let sq = (m.phi - vbs).max(1e-3).sqrt();
    let vth = vto + m.gamma * (sq - m.phi.sqrt());
    let dvth_dvbs = -m.gamma / (2.0 * sq);
    let beta = m.kp * w / l * mult;
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
    let arg = vd / nvt;
    const MAXARG: f64 = 40.0;
    if arg > MAXARG {
        let e = MAXARG.exp();
        let id = m.is * (e * (1.0 + arg - MAXARG) - 1.0);
        (id, m.is * e / nvt)
    } else {
        let e = arg.exp();
        (m.is * (e - 1.0), m.is * e / nvt)
    }
}

impl Engine {
    pub fn new(c: Circuit) -> Result<Self, SimError> {
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
                Element::C { a, b, c, .. } => caps.push(Cap { a: *a, b: *b, c: *c }),
                Element::D { a, k, model, .. } if model.cjo > 0.0 => caps.push(Cap { a: *a, b: *k, c: model.cjo }),
                Element::M { d, g, s, model, w, l, m, .. } => {
                    let cox = EPS_OX / model.tox;
                    let cgs = (2.0 / 3.0) * cox * w * l * m + model.cgso * w * m;
                    let cgd = model.cgdo * w * m;
                    if cgs > 0.0 {
                        caps.push(Cap { a: *g, b: *s, c: cgs });
                    }
                    if cgd > 0.0 {
                        caps.push(Cap { a: *g, b: *d, c: cgd });
                    }
                }
                _ => {}
            }
        }
        if nn == 0 {
            return Err(SimError::Engine("circuit has no nodes besides ground".into()));
        }
        let vt = 1.380649e-23 * (c.temp + 273.15) / 1.602176634e-19;
        Ok(Self { c, nn, branch, size, caps, inductors, vt, log: vec![] })
    }

    fn v(&self, x: &[f64], n: Node) -> f64 {
        if n == 0 { 0.0 } else { x[n - 1] }
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
                Element::M { d, g, s, b, model, w, l, m: mult, .. } => {
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
                let (geq, ieq) = match method {
                    Integ::Trapezoidal => {
                        let geq = 2.0 * cp.c / h;
                        (geq, geq * state.cap_v[k] + state.cap_i[k])
                    }
                    Integ::BackwardEuler => {
                        let geq = cp.c / h;
                        (geq, geq * state.cap_v[k])
                    }
                };
                cond(&mut m, cp.a, cp.b, geq);
                cur(&mut rhs, cp.a, cp.b, -ieq);
            }
        }
        (m, rhs)
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
                Element::M { name, d, g, s, b, model, w, l, m } => {
                    let v = |n| self.v(x, n);
                    let ev = mos_eval(model, *w, *l, *m, v(*d), v(*g), v(*s), v(*b));
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

    pub fn run_ac(&mut self, ppd: u32, fstart: f64, fstop: f64) -> Result<Dataset, SimError> {
        let x0 = self.op(None)?;
        let mut ds = self.new_dataset("ac", "AC analysis", Some(("frequency", VecKind::Frequency)));
        for v in ds.vectors.iter_mut().skip(1) {
            v.im = Some(vec![]);
        }
        let decades = (fstop / fstart).log10();
        let npts = (decades * ppd as f64).ceil().max(1.0) as usize + 1;
        let idx = |n: Node| if n == 0 { None } else { Some(n - 1) };
        for p in 0..npts {
            let f = fstart * 10f64.powf(p as f64 / ppd as f64);
            if f > fstop * 1.000001 {
                break;
            }
            let w = 2.0 * std::f64::consts::PI * f;
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
            for n in 0..self.nn {
                m.add(n, n, Complex64::new(GMIN, 0.0));
            }
            for (ei, e) in self.c.elements.iter().enumerate() {
                match e {
                    Element::R { a, b, r, .. } => y(&mut m, *a, *b, Complex64::new(1.0 / r, 0.0)),
                    Element::C { .. } => {}
                    Element::L { a, b, l, .. } => {
                        let br = self.branch[ei].unwrap();
                        if let Some(i) = idx(*a) {
                            m.add(i, br, Complex64::new(1.0, 0.0));
                            m.add(br, i, Complex64::new(1.0, 0.0));
                        }
                        if let Some(j) = idx(*b) {
                            m.add(j, br, Complex64::new(-1.0, 0.0));
                            m.add(br, j, Complex64::new(-1.0, 0.0));
                        }
                        m.add(br, br, Complex64::new(0.0, -w * l));
                    }
                    Element::V { a, b, src, .. } => {
                        let br = self.branch[ei].unwrap();
                        if let Some(i) = idx(*a) {
                            m.add(i, br, Complex64::new(1.0, 0.0));
                            m.add(br, i, Complex64::new(1.0, 0.0));
                        }
                        if let Some(j) = idx(*b) {
                            m.add(j, br, Complex64::new(-1.0, 0.0));
                            m.add(br, j, Complex64::new(-1.0, 0.0));
                        }
                        rhs[br] += Complex64::from_polar(src.ac_mag, src.ac_phase.to_radians());
                    }
                    Element::I { a, b, src, .. } => {
                        let v = Complex64::from_polar(src.ac_mag, src.ac_phase.to_radians());
                        if let Some(i) = idx(*a) {
                            rhs[i] -= v;
                        }
                        if let Some(j) = idx(*b) {
                            rhs[j] += v;
                        }
                    }
                    Element::D { a, k, model, .. } => {
                        let vd = self.v(&x0, *a) - self.v(&x0, *k);
                        let (_, gd) = diode_eval(model, vd, self.vt);
                        y(&mut m, *a, *k, Complex64::new(gd, 0.0));
                    }
                    Element::M { d, g, s, b, model, w: wd, l, m: mult, .. } => {
                        let nodes = [*d, *g, *s, *b];
                        let vs: Vec<f64> = nodes.iter().map(|&n| self.v(&x0, n)).collect();
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
            for cp in &self.caps {
                y(&mut m, cp.a, cp.b, Complex64::new(0.0, w * cp.c));
            }
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
        let mut state = TranState {
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
        while t < tstop * (1.0 - 1e-12) {
            while bp_i < bps.len() && bps[bp_i] <= t * (1.0 + 1e-12) + 1e-30 {
                bp_i += 1;
            }
            let next_bp = bps.get(bp_i).copied().unwrap_or(tstop);
            let mut hs = h.min(hmax).min(next_bp - t);
            let mut tries = 0;
            let iters;
            let xn = loop {
                let mut xt = x.clone();
                let mode = Mode::Tran { t: t + hs, h: hs, method, state: &state };
                match self.newton(&mut xt, mode, GMIN, 60) {
                    Ok(it) => {
                        iters = it;
                        break xt;
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
            for (k, cp) in self.caps.iter().enumerate() {
                let vnew = self.v(&xn, cp.a) - self.v(&xn, cp.b);
                let inew = match method {
                    Integ::Trapezoidal => 2.0 * cp.c / hs * (vnew - state.cap_v[k]) - state.cap_i[k],
                    Integ::BackwardEuler => cp.c / hs * (vnew - state.cap_v[k]),
                };
                state.cap_v[k] = vnew;
                state.cap_i[k] = inew;
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
            // Step control: grow when Newton converges quickly.
            let at_bp = (next_bp - t).abs() <= hmax * 1e-9;
            if at_bp {
                method = Integ::BackwardEuler;
                h = (hmax / 20.0).max(hs);
            } else {
                method = Integ::Trapezoidal;
                if iters <= 4 {
                    h = (hs * 2.0).min(hmax);
                } else if iters > 10 {
                    h = hs / 2.0;
                }
            }
            if steps > 5_000_000 {
                return Err(SimError::Engine("transient exceeded 5M steps".into()));
            }
        }
        Ok(ds)
    }

    pub fn run(&mut self, a: &Analysis) -> Result<Dataset, SimError> {
        let num = |s: &str| parse_spice(s).ok_or_else(|| SimError::Engine(format!("invalid number '{s}'")));
        match a {
            Analysis::Op => self.run_op(),
            Analysis::Dc { source, start, stop, step } => self.run_dc(source, num(start)?, num(stop)?, num(step)?),
            Analysis::Ac { points_per_decade, fstart, fstop } => self.run_ac(*points_per_decade, num(fstart)?, num(fstop)?),
            Analysis::Tran { tstep, tstop } => self.run_tran(num(tstep)?, num(tstop)?),
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
    fn diode_forward() {
        let deck = "* d\nV1 a 0 5\nR1 a k 1k\nD1 k 0 dm\n.model dm d (is=1e-14 n=1)\n.end";
        let mut e = engine(deck);
        let ds = e.run_op().unwrap();
        let vk = ds.get("v(k)").unwrap().re[0];
        assert!(vk > 0.6 && vk < 0.75, "{vk}");
    }

    #[test]
    fn lc_tran_runs() {
        let mut e = engine("* lc\nI1 0 a PULSE(0 1m 0 1n 1n 1u 2u)\nL1 a 0 1u\nC1 a 0 1n\nR1 a 0 1k\n.end");
        let ds = e.run_tran(1e-9, 1e-6).unwrap();
        assert!(ds.len() > 100);
    }
}
