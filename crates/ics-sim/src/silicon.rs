//! Silicon-to-simulation correlation: simulate a cell at the bias of every measured curve of
//! its [`SiliconSetup`], compare at the measured points, and fit the setup's parameters to the
//! measurements by Levenberg-Marquardt.
//!
//! Residuals are relative: on a linear trace the error over the trace's largest measured
//! magnitude, on a log trace the log ratio of simulated to measured. The reported error of a
//! trace is their RMS (a fraction; 0.01 = 1 %).

use std::collections::BTreeMap;

use ics_core::optimize::Bounds;
use ics_core::silicon::{self, FitParam, Measured, SiliconSetup};
use ics_core::sweep::{SweepPoint, point_deck};
use ics_core::{CellRef, Project};

use crate::calc::{self, CalcValue};
use crate::{SimOptions, frontend};

/// One measured curve against its simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceFit {
    pub column: String,
    pub x: Vec<f64>,
    pub measured: Vec<f64>,
    /// Simulated value at every measured x (NaN outside the simulated range).
    pub simulated: Vec<f64>,
    pub log: bool,
    /// RMS relative error.
    pub error: f64,
}

/// The cell simulated with one set of parameter values.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub values: Vec<f64>,
    pub traces: Vec<Result<TraceFit, String>>,
    /// Sum of squared residuals (failed traces count as a large penalty).
    pub cost: f64,
}

impl Evaluation {
    /// RMS error over every trace.
    pub fn error(&self) -> f64 {
        let (mut s, mut n) = (0.0, 0usize);
        for t in self.traces.iter().flatten() {
            s += t.error * t.error * t.x.len() as f64;
            n += t.x.len();
        }
        if n == 0 { f64::NAN } else { (s / n as f64).sqrt() }
    }
}

const FAIL: f64 = 1e6;

/// Linear interpolation of (xs, ys) at `x`; NaN outside the range.
fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    if xs.len() < 2 {
        return if xs.first() == Some(&x) { ys[0] } else { f64::NAN };
    }
    let asc = xs[xs.len() - 1] >= xs[0];
    let tol = 1e-9 * (xs[xs.len() - 1] - xs[0]).abs();
    for i in 0..xs.len() - 1 {
        let (a, b) = if asc { (xs[i], xs[i + 1]) } else { (xs[i + 1], xs[i]) };
        if x >= a - tol && x <= b + tol {
            let t = if xs[i + 1] != xs[i] { (x - xs[i]) / (xs[i + 1] - xs[i]) } else { 0.0 };
            return ys[i] + t.clamp(0.0, 1.0) * (ys[i + 1] - ys[i]);
        }
    }
    f64::NAN
}

/// The residuals of a trace.
fn residuals(t: &TraceFit) -> Vec<f64> {
    let scale = t.measured.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-300);
    t.measured
        .iter()
        .zip(&t.simulated)
        .map(|(m, s)| {
            if !s.is_finite() {
                return 1.0;
            }
            if t.log {
                let floor = scale * 1e-12;
                (s.abs().max(floor) / m.abs().max(floor)).ln()
            } else {
                (s - m) / scale
            }
        })
        .collect()
}

/// A prepared correlation: the cell, measured data and parameter bounds.
pub struct Correlation<'a> {
    project: &'a Project,
    cell: CellRef,
    pub setup: SiliconSetup,
    data: Measured,
    bounds: Vec<Bounds>,
    opts: SimOptions,
}

impl<'a> Correlation<'a> {
    /// The cell's setup with its data file read (relative to the project directory).
    pub fn new(project: &'a Project, cell: &CellRef, opts: &SimOptions) -> Result<Self, String> {
        let c = project.cell(cell).ok_or_else(|| format!("no cell {cell}"))?;
        let setup = c.sim.silicon.clone();
        if setup.data.trim().is_empty() {
            return Err(format!("{cell}: no measured data (sim.silicon.data)"));
        }
        let rel = std::path::Path::new(setup.data.trim());
        let path = match &project.root {
            Some(root) if rel.is_relative() => root.join(rel),
            _ => rel.to_path_buf(),
        };
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let data = Measured::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::with_data(project, cell, setup, data, opts)
    }

    pub fn with_data(
        project: &'a Project,
        cell: &CellRef,
        setup: SiliconSetup,
        data: Measured,
        opts: &SimOptions,
    ) -> Result<Self, String> {
        let c = project.cell(cell).ok_or_else(|| format!("no cell {cell}"))?;
        if setup.traces.is_empty() {
            return Err("no traces: map measured columns to simulated expressions".into());
        }
        for t in &setup.traces {
            data.points(&setup.x, &t.column)?;
            silicon::parse_bias(&t.bias)?;
        }
        let bounds = setup.params.iter().map(|p| Bounds::of(&p.as_opt_var())).collect::<Result<Vec<_>, _>>()?;
        let opts = SimOptions { backend: c.sim.backend, ..opts.clone() };
        Ok(Self { project, cell: cell.clone(), setup, data, bounds, opts })
    }

    /// The deck of one bias with the parameter `values` applied.
    pub fn deck(&self, bias: &str, values: &[f64]) -> Result<String, String> {
        let c = self.project.cell(&self.cell).ok_or("no cell")?;
        let sch = c.schematic.as_ref().ok_or_else(|| format!("{} has no schematic", self.cell))?;
        let mut vars = silicon::parse_bias(bias)?;
        let mut model = vec![];
        for (p, v) in self.setup.params.iter().zip(values) {
            match p.model_param() {
                Some(mp) => model.push((mp, *v)),
                None => vars.push((p.target.trim().to_string(), format!("{v:.6e}"))),
            }
        }
        let pt = SweepPoint { corner: c.sim.corner.clone(), temp: c.sim.temp, vars, run: None, swept: 0 };
        let (mut deck, _) = point_deck(sch, &self.project.tech, &self.cell.to_string(), &c.sim, &pt)?;
        if !model.is_empty() {
            if frontend::needs_expansion(&deck) {
                deck = frontend::expand(&deck, self.opts.deck_dir.as_deref()).map_err(|e| e.to_string())?.deck;
            }
            for ((m, p), v) in model {
                deck = silicon::set_model_param(&deck, m, p, v)?;
            }
        }
        Ok(deck)
    }

    /// The parameters' present values: model cards and instance parameters of the cell, the
    /// middle of the bounds where unknown.
    pub fn start_values(&self) -> Vec<f64> {
        let base = self.deck("", &[]).ok();
        let base =
            base.map(|d| if frontend::needs_expansion(&d) { frontend::expand(&d, None).map(|e| e.deck).unwrap_or(d) } else { d });
        let sch = self.project.cell(&self.cell).and_then(|c| c.schematic.as_ref());
        self.setup
            .params
            .iter()
            .zip(&self.bounds)
            .map(|(p, b)| {
                let cur = match p.model_param() {
                    Some((m, q)) => base.as_deref().and_then(|d| silicon::model_param(d, m, q)),
                    None => p.target.split_once('.').and_then(|(i, q)| {
                        let inst = sch?.instances.iter().find(|x| x.name.eq_ignore_ascii_case(i))?;
                        ics_core::units::parse_spice(inst.param(&q.to_ascii_lowercase()))
                    }),
                };
                b.to_real(cur.map_or(0.5, |x| b.to_unit(x)))
            })
            .collect()
    }

    /// Simulate every bias with `values` and compare with the measured curves.
    pub fn evaluate(&self, values: &[f64]) -> Evaluation {
        let mut by_bias: BTreeMap<String, Result<crate::SimResults, String>> = BTreeMap::new();
        let c = self.project.cell(&self.cell);
        let analyses: Vec<_> = c.map(|c| c.sim.enabled().cloned().collect()).unwrap_or_default();
        for t in &self.setup.traces {
            let key = t.bias.trim().to_string();
            if by_bias.contains_key(&key) {
                continue;
            }
            let r = self.deck(&key, values).and_then(|d| crate::simulate(&d, &analyses, &self.opts).map_err(|e| e.to_string()));
            by_bias.insert(key, r);
        }
        let analysis = if self.setup.analysis.trim().is_empty() { "dc" } else { self.setup.analysis.trim() };
        let mut cost = 0.0;
        let traces: Vec<Result<TraceFit, String>> = self
            .setup
            .traces
            .iter()
            .map(|t| {
                let (x, measured) = self.data.points(&self.setup.x, &t.column)?;
                let res = by_bias[t.bias.trim()].as_ref().map_err(|e| format!("simulation failed: {e}"))?;
                let ds = res.dataset(analysis).ok_or_else(|| format!("no {analysis} results: enable the analysis"))?;
                let sx = ds.scale_vector().ok_or_else(|| format!("the {analysis} results have no sweep"))?.re.clone();
                let w = match calc::eval(&t.expr, ds)? {
                    CalcValue::Wave(w) => w,
                    CalcValue::Scalar(_) => return Err(format!("'{}' gives a number; give a waveform", t.expr)),
                };
                let sy = if w.im.is_some() { w.mag() } else { w.re.clone() };
                let simulated = x.iter().map(|&xv| interp(&sx, &sy, xv)).collect();
                let mut tf = TraceFit { column: t.column.clone(), x, measured, simulated, log: t.log, error: 0.0 };
                let r = residuals(&tf);
                tf.error = (r.iter().map(|v| v * v).sum::<f64>() / r.len().max(1) as f64).sqrt();
                cost += r.iter().map(|v| v * v).sum::<f64>();
                Ok(tf)
            })
            .collect();
        let failed = traces.iter().filter(|t| t.is_err()).count();
        Evaluation { values: values.to_vec(), traces, cost: cost + FAIL * failed as f64 }
    }

    fn residual_vector(&self, values: &[f64]) -> (Evaluation, Vec<f64>) {
        let e = self.evaluate(values);
        let mut r = vec![];
        for (t, tr) in self.setup.traces.iter().zip(&e.traces) {
            match tr {
                Ok(tf) => r.extend(residuals(tf)),
                Err(_) => r.extend(std::iter::repeat_n(1e3, self.data.points(&self.setup.x, &t.column).map_or(1, |p| p.0.len()))),
            }
        }
        (e, r)
    }

    /// Fit the parameters by Levenberg-Marquardt in the bounds' unit coordinates, starting from
    /// [`Correlation::start_values`]. `progress(iteration, best)` returning false stops.
    pub fn fit(&self, max_iter: usize, progress: &mut dyn FnMut(usize, &Evaluation) -> bool) -> Result<FitReport, String> {
        let n = self.setup.params.len();
        if n == 0 {
            return Err("no parameters to fit".into());
        }
        let real = |u: &[f64]| u.iter().zip(&self.bounds).map(|(u, b)| b.to_real(*u)).collect::<Vec<_>>();
        let mut u: Vec<f64> = self.start_values().iter().zip(&self.bounds).map(|(x, b)| b.to_unit(*x)).collect();
        let (start, mut r) = self.residual_vector(&real(&u));
        let mut best = start.clone();
        let mut lambda = 1e-2;
        let mut evals = 1;
        let mut iterations = 0;
        let h = 1e-4;
        while iterations < max_iter {
            iterations += 1;
            // Jacobian by forward differences, the columns simulated in parallel.
            let cols: Vec<Vec<f64>> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..n)
                    .map(|j| {
                        let mut up = u.clone();
                        let step = if up[j] + h <= 1.0 { h } else { -h };
                        up[j] += step;
                        let x = real(&up);
                        let r0 = &r;
                        s.spawn(move || {
                            let (_, rj) = self.residual_vector(&x);
                            rj.iter().zip(r0).map(|(a, b)| (a - b) / step).collect::<Vec<f64>>()
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            evals += n;
            let m = r.len();
            let mut jtj = vec![0.0; n * n];
            let mut jtr = vec![0.0; n];
            for a in 0..n {
                for k in 0..m {
                    jtr[a] += cols[a][k] * r[k];
                }
                for b in 0..=a {
                    let v: f64 = (0..m).map(|k| cols[a][k] * cols[b][k]).sum();
                    jtj[a * n + b] = v;
                    jtj[b * n + a] = v;
                }
            }
            let mut improved = false;
            for _ in 0..8 {
                let mut a = jtj.clone();
                for d in 0..n {
                    a[d * n + d] += lambda * jtj[d * n + d].max(1e-12);
                }
                let Some(delta) = solve(&mut a, &jtr.iter().map(|v| -v).collect::<Vec<_>>(), n) else {
                    lambda *= 10.0;
                    continue;
                };
                let un: Vec<f64> = u.iter().zip(&delta).map(|(u, d)| (u + d).clamp(0.0, 1.0)).collect();
                let (e, rn) = self.residual_vector(&real(&un));
                evals += 1;
                if e.cost < best.cost {
                    let gain = (best.cost - e.cost) / best.cost.max(1e-300);
                    u = un;
                    r = rn;
                    best = e;
                    lambda = (lambda / 3.0).max(1e-9);
                    improved = gain > 1e-6;
                    break;
                }
                lambda *= 4.0;
            }
            if !progress(iterations, &best) || !improved {
                break;
            }
        }
        Ok(FitReport { start, best, iterations, evaluations: evals })
    }

    /// The model cards of the fitted model parameters at `values`, ready for a model file.
    pub fn fitted_cards(&self, values: &[f64]) -> Result<String, String> {
        let mut models: Vec<&str> = self.setup.params.iter().filter_map(|p| p.model_param().map(|m| m.0)).collect();
        models.dedup();
        if models.is_empty() {
            return Ok(String::new());
        }
        let bias = self.setup.traces.first().map_or("", |t| t.bias.as_str());
        let deck = self.deck(bias, values)?;
        let mut out = String::new();
        for m in models {
            for c in silicon::model_cards(&deck, m) {
                out += &c;
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// The fit parameters.
    pub fn params(&self) -> &[FitParam] {
        &self.setup.params
    }
}

/// Gaussian elimination with partial pivoting.
fn solve(a: &mut [f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut b = b.to_vec();
    for c in 0..n {
        let p = (c..n).max_by(|i, j| a[i * n + c].abs().total_cmp(&a[j * n + c].abs()))?;
        if a[p * n + c].abs() < 1e-300 {
            return None;
        }
        for k in 0..n {
            a.swap(c * n + k, p * n + k);
        }
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r * n + c] / a[c * n + c];
            for k in c..n {
                a[r * n + k] -= f * a[c * n + k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        x[r] = (b[r] - (r + 1..n).map(|k| a[r * n + k] * x[k]).sum::<f64>()) / a[r * n + r];
    }
    Some(x)
}

/// The result of a fit.
#[derive(Debug, Clone, PartialEq)]
pub struct FitReport {
    pub start: Evaluation,
    pub best: Evaluation,
    pub iterations: usize,
    pub evaluations: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::silicon::Trace;

    /// A single NMOS with gate and drain sources, as a characterisation cell.
    pub(crate) fn nmos_iv() -> (Project, CellRef) {
        let mut p = ics_core::examples::demo_project();
        let net = "* nmos iv\nVG g 0 DC 0.9\nVD d 0 DC 0\nM1 d g 0 0 nmos w=2u l=1u\n.dc VD 0 1.8 0.05\n.end\n";
        let imp = ics_core::spice_import::import(net, &p.tech).unwrap();
        let r = p.add_cell("work", ics_core::Cell::new("nmos_iv")).unwrap();
        let c = p.cell_mut(&r).unwrap();
        imp.apply_to(&mut c.sim);
        c.schematic = Some(imp.schematic);
        (p, r)
    }

    /// The Id-Vd curves of `cor`'s traces simulated with `values`, as measured data.
    pub(crate) fn synthetic(cor: &Correlation, values: &[f64]) -> Measured {
        let vd: Vec<f64> = (0..=36).map(|k| k as f64 * 0.05).collect();
        let mut columns = vec!["vd".to_string()];
        let mut rows: Vec<Vec<f64>> = vd.iter().map(|v| vec![*v]).collect();
        let c = cor.project.cell(&cor.cell).unwrap();
        let analyses: Vec<_> = c.sim.enabled().cloned().collect();
        for t in &cor.setup.traces {
            columns.push(t.column.clone());
            let deck = cor.deck(&t.bias, values).unwrap();
            let res = crate::simulate(&deck, &analyses, &SimOptions::default()).unwrap();
            let ds = res.dataset("dc").unwrap();
            let CalcValue::Wave(w) = calc::eval(&t.expr, ds).unwrap() else { panic!() };
            let sx = &ds.scale_vector().unwrap().re;
            for (row, v) in rows.iter_mut().zip(&vd) {
                row.push(interp(sx, &w.re, *v));
            }
        }
        Measured { columns, rows }
    }

    pub(crate) fn iv_setup() -> SiliconSetup {
        let traces = ["0.7", "0.9", "1.2"]
            .iter()
            .map(|v| Trace { column: format!("id_vg{v}"), expr: "-i(vd)".into(), bias: format!("VG.dc={v}"), log: false })
            .collect();
        let params = vec![
            FitParam { target: "model:nmos.vto".into(), min: "0.3".into(), max: "0.7".into(), log: false },
            FitParam { target: "model:nmos.kp".into(), min: "100u".into(), max: "500u".into(), log: true },
            FitParam { target: "model:nmos.lambda".into(), min: "0.01".into(), max: "0.3".into(), log: true },
        ];
        SiliconSetup { data: String::new(), analysis: "dc".into(), x: "vd".into(), traces, params }
    }

    /// Id-Vd curves "measured" with vto = 0.52, kp = 240u, lambda = 0.05 are fitted back from
    /// the generic model (0.45, 280u, 0.08).
    #[test]
    fn fits_model_parameters_to_measurements() {
        let (p, r) = nmos_iv();
        let setup = iv_setup();
        let blank = Measured {
            columns: vec!["vd".into(), "id_vg0.7".into(), "id_vg0.9".into(), "id_vg1.2".into()],
            rows: vec![vec![0.0; 4]],
        };
        let truth = Correlation::with_data(&p, &r, setup.clone(), blank, &SimOptions::default()).unwrap();
        let data = synthetic(&truth, &[0.52, 240e-6, 0.05]);
        let cor = Correlation::with_data(&p, &r, setup, data, &SimOptions::default()).unwrap();
        let start = cor.start_values();
        assert!(
            (start[0] - 0.45).abs() < 1e-9 && (start[1] - 280e-6).abs() < 1e-12 && (start[2] - 0.08).abs() < 1e-9,
            "{start:?}"
        );
        let rep = cor.fit(40, &mut |_, _| true).unwrap();
        assert!(rep.start.error() > 0.05, "{}", rep.start.error());
        assert!(rep.best.error() < 1e-3, "{} after {} iterations", rep.best.error(), rep.iterations);
        let v = &rep.best.values;
        assert!((v[0] - 0.52).abs() < 2e-3 && (v[1] / 240e-6 - 1.0).abs() < 1e-2 && (v[2] / 0.05 - 1.0).abs() < 2e-2, "{v:?}");
        let cards = cor.fitted_cards(v).unwrap();
        assert!(cards.starts_with(".model nmos nmos") && cards.contains("vto=5.2"), "{cards}");
    }

    #[test]
    fn interpolation() {
        assert_eq!(interp(&[0.0, 1.0, 2.0], &[0.0, 10.0, 0.0], 1.5), 5.0);
        assert_eq!(interp(&[2.0, 1.0, 0.0], &[0.0, 10.0, 20.0], 0.5), 15.0);
        assert!(interp(&[0.0, 1.0], &[0.0, 1.0], 1.5).is_nan());
    }
}
