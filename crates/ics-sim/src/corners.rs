//! Corner models from silicon: fit model parameters ("knobs") so that simulated test structures
//! reproduce the WAT data's means (the nominal, `tt`) and each corner's targets
//! ([`ics_core::process_data::corner_targets`]), and write the result as a corner library
//! (`.lib tt` ... `.endl` sections of `.model` cards).

use ics_core::optimize::Bounds;
use ics_core::process_data::{
    CornerTargets, MeasuredStats, ProcessSpec, ProcessVariation, WatData, backward_propagation, corner_targets,
    forward_propagation, measured_stats,
};
use ics_core::silicon::{self, FitParam};
use ics_core::{CellRef, Project};

use crate::calc::{self, CalcValue};
use crate::silicon::{cell_deck, levenberg_marquardt, std_errors};
use crate::{SimOptions, frontend};

/// One fitted corner.
#[derive(Debug, Clone, PartialEq)]
pub struct CornerFit {
    pub name: String,
    /// Knob values.
    pub values: Vec<f64>,
    /// Standard errors of the values (NaN where unknown, e.g. as many targets as knobs).
    pub std_errors: Vec<f64>,
    /// (parameter, target, achieved) for every simulated parameter.
    pub params: Vec<(String, f64, Result<f64, String>)>,
    /// RMS error of the parameters in units of their data sigma.
    pub error_sigma: f64,
}

/// The global process variation of the knobs, by backward propagation of the WAT covariance.
#[derive(Debug, Clone, PartialEq)]
pub struct VariationFit {
    pub variation: ProcessVariation,
    /// Row-major (measured parameter × knob) sensitivities ∂parameter/∂knob at the nominal.
    pub sensitivity: Vec<f64>,
    /// (parameter, σ of the data, σ the variation reproduces).
    pub params: Vec<(String, f64, f64)>,
    /// (parameter a, parameter b, correlation of the data, reproduced correlation).
    pub correlations: Vec<(String, String, f64, f64)>,
}

/// A simulated WAT parameter: its test-structure cell and expression.
struct Etest {
    name: String,
    cell: CellRef,
    analysis: String,
    expr: String,
}

/// A corner extraction: the spec's test structures imported into a copy of the project.
pub struct Extraction {
    project: Project,
    etests: Vec<Etest>,
    pub knobs: Vec<FitParam>,
    bounds: Vec<Bounds>,
    pub targets: Vec<CornerTargets>,
    /// Mean and covariance of the measured parameters.
    pub stats: MeasuredStats,
    opts: SimOptions,
}

impl Extraction {
    pub fn new(project: &Project, spec: &ProcessSpec, data: &WatData, opts: &SimOptions) -> Result<Self, String> {
        let ex = spec.extraction.as_ref().ok_or("the spec has no [extraction]")?;
        if ex.knobs.is_empty() {
            return Err("no knobs to fit".into());
        }
        if ex.knobs.iter().any(|k| k.model_param().is_none()) {
            return Err("every knob must be a model parameter (model:NAME.PARAM)".into());
        }
        let targets = corner_targets(data, spec)?;
        let mut p = project.clone();
        let lib = "__etest__";
        if p.library_mut(lib).is_none() {
            p.add_library(lib)?;
        }
        let mut etests = vec![];
        for (k, ps) in spec.params.iter().enumerate() {
            let Some(m) = &ps.measure else { continue };
            let text = format!("* {}\n{}\n.end\n", ps.name, m.netlist.trim());
            let imp = ics_core::spice_import::import(&text, &p.tech).map_err(|e| format!("{}: test structure: {e}", ps.name))?;
            let r = p.add_cell(lib, ics_core::Cell::new(format!("etest_{k}")))?;
            let c = p.cell_mut(&r).expect("added");
            imp.apply_to(&mut c.sim);
            c.sim.backend = ics_core::Backend::Builtin;
            c.schematic = Some(imp.schematic);
            etests.push(Etest {
                name: ps.name.clone(),
                cell: r,
                analysis: m.analysis.trim().to_ascii_lowercase(),
                expr: m.expr.clone(),
            });
        }
        let bounds = ex.knobs.iter().map(|k| Bounds::of(&k.as_opt_var())).collect::<Result<Vec<_>, _>>()?;
        let opts = SimOptions { backend: ics_core::Backend::Builtin, ..opts.clone() };
        let stats = measured_stats(data, spec)?;
        Ok(Self { project: p, etests, knobs: ex.knobs.clone(), bounds, targets, stats, opts })
    }

    fn model(&self, values: &[f64]) -> Vec<(String, String, f64)> {
        self.knobs
            .iter()
            .zip(values)
            .filter_map(|(k, v)| k.model_param().map(|(m, p)| (m.to_string(), p.to_string(), *v)))
            .collect()
    }

    /// Simulate every test structure with the knob `values`.
    pub fn measure(&self, values: &[f64]) -> Vec<(String, Result<f64, String>)> {
        let model = self.model(values);
        self.etests
            .iter()
            .map(|e| {
                let r = (|| {
                    let deck = cell_deck(&self.project, &e.cell, vec![], &model, self.opts.deck_dir.as_deref())?;
                    let c = self.project.cell(&e.cell).ok_or("no cell")?;
                    let analyses: Vec<_> = c.sim.enabled().cloned().collect();
                    let res = crate::simulate(&deck, &analyses, &self.opts).map_err(|e| e.to_string())?;
                    let ds = res.dataset(&e.analysis).ok_or_else(|| format!("no {} results", e.analysis))?;
                    match calc::eval(&e.expr, ds)? {
                        CalcValue::Scalar(x) if x.is_finite() => Ok(x),
                        CalcValue::Wave(w) if w.re.len() == 1 && w.re[0].is_finite() => Ok(w.re[0]),
                        _ => Err(format!("'{}' gives no single number", e.expr)),
                    }
                })();
                (e.name.clone(), r)
            })
            .collect()
    }

    /// The technology's present value of every knob, clamped into its bounds.
    pub fn start_values(&self) -> Vec<f64> {
        let deck = self.etests.first().and_then(|e| cell_deck(&self.project, &e.cell, vec![], &[], None).ok()).map(|d| {
            if frontend::needs_expansion(&d) {
                frontend::expand(&d, self.opts.deck_dir.as_deref()).map(|e| e.deck).unwrap_or(d)
            } else {
                d
            }
        });
        self.knobs
            .iter()
            .zip(&self.bounds)
            .map(|(k, b)| {
                let cur = k.model_param().and_then(|(m, p)| silicon::model_param(deck.as_deref()?, m, p));
                b.to_real(cur.map_or(0.5, |x| b.to_unit(x)))
            })
            .collect()
    }

    /// Fit the knobs to one corner's targets from `start`.
    pub fn fit_corner(&self, t: &CornerTargets, start: &[f64], max_iter: usize) -> CornerFit {
        let real = |u: &[f64]| u.iter().zip(&self.bounds).map(|(u, b)| b.to_real(*u)).collect::<Vec<_>>();
        let eval = |u: &[f64]| {
            let m = self.measure(&real(u));
            let r: Vec<f64> = t
                .targets
                .iter()
                .map(|(n, target, _, sd)| {
                    let scale = if *sd > 0.0 { *sd } else { target.abs().max(1e-30) * 0.01 };
                    match m.iter().find(|(x, _)| x == n).map(|(_, v)| v) {
                        Some(Ok(v)) => (v - target) / scale,
                        _ => 1e3,
                    }
                })
                .collect();
            let cost = r.iter().map(|v| v * v).sum::<f64>();
            (m, r, cost)
        };
        let u0: Vec<f64> = start.iter().zip(&self.bounds).map(|(x, b)| b.to_unit(*x)).collect();
        let (_, best, lm) = levenberg_marquardt(u0, max_iter, &eval, &mut |_, _| true);
        let values = real(&lm.u);
        let params: Vec<(String, f64, Result<f64, String>)> = t
            .targets
            .iter()
            .map(|(n, target, _, _)| {
                let got = best.iter().find(|(x, _)| x == n).map_or(Err("not measured".to_string()), |(_, v)| v.clone());
                (n.clone(), *target, got)
            })
            .collect();
        let errs: Vec<f64> = t
            .targets
            .iter()
            .zip(&params)
            .map(|((_, target, _, sd), (_, _, got))| match got {
                Ok(v) if *sd > 0.0 => (v - target) / sd,
                Ok(_) => 0.0,
                Err(_) => f64::INFINITY,
            })
            .collect();
        let error_sigma = (errs.iter().map(|e| e * e).sum::<f64>() / errs.len().max(1) as f64).sqrt();
        CornerFit { name: t.name.clone(), std_errors: std_errors(&self.bounds, &values, &lm), values, params, error_sigma }
    }

    /// Fit the nominal from the technology's model, then every corner from the nominal.
    /// `progress(corner)` is called after each.
    pub fn run(&self, max_iter: usize, progress: &mut dyn FnMut(&CornerFit)) -> Vec<CornerFit> {
        let mut out: Vec<CornerFit> = vec![];
        let start = self.start_values();
        for t in &self.targets {
            let from = out.first().map_or(start.clone(), |tt| tt.values.clone());
            let f = self.fit_corner(t, &from, max_iter);
            progress(&f);
            out.push(f);
        }
        out
    }

    /// The global variation of the knobs around the nominal `tt` that reproduces the measured
    /// spread and correlation of the parameters (backward propagation of variance through the
    /// sensitivities of the simulated test structures).
    pub fn variation(&self, tt: &CornerFit, source: &str) -> Result<VariationFit, String> {
        let (m, n) = (self.stats.names.len(), self.knobs.len());
        let value = |r: &[(String, Result<f64, String>)], name: &str| -> Result<f64, String> {
            r.iter().find(|(x, _)| x == name).ok_or_else(|| format!("{name}: not measured"))?.1.clone()
        };
        let mut sens = vec![0.0; m * n];
        for j in 0..n {
            let x = tt.values[j];
            let h = (x.abs() * 1e-3).max(1e-12);
            let (mut lo, mut hi) = (tt.values.clone(), tt.values.clone());
            lo[j] -= h;
            hi[j] += h;
            let (a, b) = (self.measure(&lo), self.measure(&hi));
            for (i, name) in self.stats.names.iter().enumerate() {
                sens[i * n + j] = (value(&b, name)? - value(&a, name)?) / (2.0 * h);
            }
        }
        let cov = backward_propagation(&sens, m, n, &self.stats.cov)
            .ok_or("the knobs are not identifiable from the measured parameters (singular sensitivities)")?;
        let params = self.knobs.iter().filter_map(|k| k.model_param().map(|(m, p)| format!("{m}.{p}"))).collect::<Vec<_>>();
        let variation = ProcessVariation { source: source.to_string(), params, mean: tt.values.clone(), cov };
        let pred = forward_propagation(&sens, m, n, &variation.cov);
        let sd = |c: &[f64], i: usize| c[i * m + i].max(0.0).sqrt();
        let corr = |c: &[f64], i: usize, j: usize| {
            let d = sd(c, i) * sd(c, j);
            if d > 0.0 { c[i * m + j] / d } else { 0.0 }
        };
        let names = &self.stats.names;
        let mut correlations = vec![];
        for i in 0..m {
            for j in i + 1..m {
                correlations.push((names[i].clone(), names[j].clone(), corr(&self.stats.cov, i, j), corr(&pred, i, j)));
            }
        }
        Ok(VariationFit {
            params: (0..m).map(|i| (names[i].clone(), sd(&self.stats.cov, i), sd(&pred, i))).collect(),
            variation,
            sensitivity: sens,
            correlations,
        })
    }

    /// The fitted corners as a SPICE corner library: one `.lib NAME` section per corner with
    /// the `.model` cards of every model the knobs touch, and with a `variation`, an `mc` section
    /// whose knobs are the nominal plus the Cholesky factor of their covariance times standard
    /// normal parameters `ics_z1` ... (0 unless a Monte Carlo loop sets them), so the section
    /// simulates as `tt` until a loop draws them.
    pub fn library(&self, fits: &[CornerFit], variation: Option<&ProcessVariation>, title: &str) -> Result<String, String> {
        let mut models: Vec<String> = self.knobs.iter().filter_map(|k| k.model_param().map(|m| m.0.to_string())).collect();
        models.sort();
        models.dedup();
        let e = self.etests.first().ok_or("no test structure")?;
        let mut out = format!("* {title}\n* Corner models fitted to WAT data by IC Studio (ics corners).\n");
        for f in fits {
            let deck = cell_deck(&self.project, &e.cell, vec![], &self.model(&f.values), self.opts.deck_dir.as_deref())?;
            out += &format!("\n.lib {}\n", f.name);
            for m in &models {
                for c in silicon::model_cards(&deck, m) {
                    out += &c;
                    out.push('\n');
                }
            }
            out += &format!(".endl {}\n", f.name);
        }
        if let Some(v) = variation {
            let l = v.factor().ok_or("the variation's covariance is not positive semi-definite")?;
            let n = v.params.len();
            let id = |i: usize| {
                let (m, p) = v.param(i);
                format!("ics_{m}_{p}").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>()
            };
            let mut deck = cell_deck(&self.project, &e.cell, vec![], &[], self.opts.deck_dir.as_deref())?;
            out += "\n.lib mc\n* Global process variation from the WAT covariance. ics_z1 ... are independent standard\n";
            out += "* normal draws, 0 (the nominal) unless a Monte Carlo loop sets them before each run, e.g. in\n";
            out += "* ngspice: let z = sgauss(0)  alterparam ics_z1 = $&z  reset  (one shared draw per run; an\n";
            out += "* agauss() here would be drawn anew at every reference and lose the correlations).\n";
            for i in 0..n {
                out += &format!(".param ics_z{}=0\n", i + 1);
            }
            for i in 0..n {
                let mut expr = format!("{:.6e}", v.mean[i]);
                for j in 0..=i {
                    if l[i * n + j] != 0.0 {
                        expr += &format!("{:+.6e}*ics_z{}", l[i * n + j], j + 1);
                    }
                }
                out += &format!(".param {}={{{expr}}}\n", id(i));
                let (m, p) = v.param(i);
                deck = ics_core::silicon::set_model_param_text(&deck, m, p, &format!("{{{}}}", id(i)))?;
            }
            for m in &models {
                for c in silicon::model_cards(&deck, m) {
                    out += &c;
                    out.push('\n');
                }
            }
            out += ".endl mc\n";
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::process_data::GENERIC180_SPEC;

    #[test]
    fn corners_from_the_example_wat_data() {
        let p = ics_core::examples::demo_project();
        let spec = ProcessSpec::from_toml(GENERIC180_SPEC).unwrap();
        let csv = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../process/generic180_wat.csv")).unwrap();
        let data = WatData::parse(&csv).unwrap();
        let ex = Extraction::new(&p, &spec, &data, &SimOptions::default()).unwrap();
        assert_eq!(ex.targets.len(), 5);
        // The generic model is far from the data (idsat_n 1600 vs 590 µA/µm) before the fit.
        let before = ex.measure(&ex.start_values());
        let idn = before.iter().find(|(n, _)| n == "idsat_n").unwrap().1.clone().unwrap();
        assert!(idn > 1000.0, "{before:?}");
        let fits = ex.run(40, &mut |_| {});
        let tt = &fits[0];
        assert!(tt.error_sigma < 0.05, "{tt:?}");
        // ff is faster than tt, ss slower, for both devices.
        let get = |f: &CornerFit, n: &str| f.params.iter().find(|p| p.0 == n).unwrap().2.clone().unwrap();
        let (ff, ss) = (fits.iter().find(|f| f.name == "ff").unwrap(), fits.iter().find(|f| f.name == "ss").unwrap());
        assert!(get(ff, "idsat_n") > get(tt, "idsat_n") && get(ss, "idsat_n") < get(tt, "idsat_n"));
        assert!(get(ff, "idsat_p") > get(tt, "idsat_p") && get(ss, "idsat_p") < get(tt, "idsat_p"));
        assert!(get(ff, "vtn") < get(tt, "vtn"), "the threshold follows its correlation with idsat");
        for f in &fits {
            assert!(f.error_sigma < 0.2, "{} misses its targets: {f:?}", f.name);
        }
        let lib = ex.library(&fits, None, "generic180").unwrap();
        assert!(lib.contains(".lib tt\n.model nmos nmos") && lib.contains(".endl sf"), "{lib}");
        assert_eq!(lib.matches(".model pmos").count(), 5);

        // Four knobs, four parameters: the variation reproduces the data's spread exactly.
        let v = ex.variation(tt, "generic180 WAT").unwrap();
        v.variation.validate().unwrap();
        assert_eq!(v.variation.params, ["nmos.vto", "nmos.kp", "pmos.vto", "pmos.kp"]);
        for (n, data, got) in &v.params {
            assert!((got / data - 1.0).abs() < 1e-3, "{n}: {got} vs {data}");
        }
        for (a, b, data, got) in &v.correlations {
            assert!((got - data).abs() < 1e-3, "{a}/{b}: {got} vs {data}");
        }
        // Linearisation check: ff (idsat at +3σ) sits close to tt + 3σ of the variation.
        let i = v.variation.params.iter().position(|p| p == "nmos.vto").unwrap();
        assert!(v.variation.sigma(i) > 1e-3 && v.variation.sigma(i) < 0.1, "{v:?}");
        let lib = ex.library(&fits, Some(&v.variation), "generic180").unwrap();
        assert!(lib.contains(".lib mc\n") && lib.contains(".param ics_z4=0\n") && lib.contains("vto={ics_nmos_vto}"), "{lib}");
        // The built-in engine reads the mc section as the nominal: tt's drain current.
        let mc = lib.split(".lib mc").nth(1).unwrap();
        let tt_vto = silicon::model_param(&lib, "nmos", "vto").unwrap();
        assert!(mc.contains(&format!("ics_nmos_vto={{{tt_vto:.6e}")), "{mc}");

        // Round trip: Monte Carlo of the idsat_n test structure with the variation (no mismatch)
        // reproduces the spread of the data.
        let e = ex.etests.iter().find(|e| e.name == "idsat_n").unwrap();
        let deck = cell_deck(&ex.project, &e.cell, vec![], &[], None).unwrap();
        let spec = crate::montecarlo::McSpec {
            runs: 600,
            seed: 7,
            avt: 0.0,
            abeta: 0.0,
            process: Some(v.variation.clone()),
            ..Default::default()
        };
        let ms = [crate::montecarlo::Measure { analysis: "op".into(), expr: e.expr.clone() }];
        let r = crate::montecarlo::monte_carlo(&deck, &[], &ms, &spec, None).unwrap();
        let data = v.params.iter().find(|p| p.0 == "idsat_n").unwrap().1;
        let mean = ex.stats.mean[ex.stats.names.iter().position(|n| n == "idsat_n").unwrap()];
        assert!((r.stats[0].sigma / data - 1.0).abs() < 0.1, "MC σ {} vs data {data}", r.stats[0].sigma);
        assert!((r.stats[0].mean / mean - 1.0).abs() < 0.01, "MC mean {} vs data {mean}", r.stats[0].mean);
    }
}
