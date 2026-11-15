//! Corner models from silicon: fit model parameters ("knobs") so that simulated test structures
//! reproduce the WAT data's means (the nominal, `tt`) and each corner's targets
//! ([`ics_core::process_data::corner_targets`]), and write the result as a corner library
//! (`.lib tt` ... `.endl` sections of `.model` cards).

use ics_core::optimize::Bounds;
use ics_core::process_data::{CornerTargets, ProcessSpec, WatData, corner_targets};
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
        Ok(Self { project: p, etests, knobs: ex.knobs.clone(), bounds, targets, opts })
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

    /// The fitted corners as a SPICE corner library: one `.lib NAME` section per corner with
    /// the `.model` cards of every model the knobs touch.
    pub fn library(&self, fits: &[CornerFit], title: &str) -> Result<String, String> {
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
        let lib = ex.library(&fits, "generic180").unwrap();
        assert!(lib.contains(".lib tt\n.model nmos nmos") && lib.contains(".endl sf"), "{lib}");
        assert_eq!(lib.matches(".model pmos").count(), 5);
    }
}
