//! Run a cell's saved corner / sweep / Monte Carlo setup (`sim.sweep`) headless, on a pool of
//! threads, and predict the yield of its Monte Carlo runs ([`ics_core::predict`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ics_core::predict::{self, ConditionYield};
use ics_core::sweep::{McPlan, SweepPoint, mc_plan, point_deck};
use ics_core::{CellRef, Project};

use crate::SimOptions;
use crate::optimize::measure;

/// The value of each spec at one point.
pub type SpecValues = Vec<Result<f64, String>>;

/// Every point of a sweep with the value of each spec.
#[derive(Debug, Clone)]
pub struct SweepRun {
    pub points: Vec<(SweepPoint, SpecValues)>,
    /// The Monte Carlo inputs and draws (`None` without Monte Carlo).
    pub plan: Option<McPlan>,
    /// Netlister and parasitic notes.
    pub notes: Vec<String>,
}

impl SweepRun {
    /// The predicted yield of every condition, from `samples` surrogate samples.
    pub fn predict(&self, specs: &[ics_core::sweep::Spec], samples: usize, seed: u64) -> Vec<ConditionYield> {
        let Some(plan) = &self.plan else { return vec![] };
        let pts: Vec<_> = self.points.iter().map(|(p, v)| (p, v.iter().map(|r| r.as_ref().ok().copied()).collect())).collect();
        predict::predict_sweep(plan, specs, &pts, samples, seed)
    }
}

/// Simulate every point of the cell's sweep setup (with its parasitics when the setup asks for
/// them); `progress(done, total)` is called as points finish.
pub fn run(
    project: &Project,
    cell: &CellRef,
    opts: &SimOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<SweepRun, String> {
    let c = project.cell(cell).ok_or_else(|| format!("no cell {cell}"))?;
    let circuit = project.circuit(cell)?;
    let sch = &*circuit;
    let setup = &c.sim;
    let analyses: Vec<_> = setup.enabled().cloned().collect();
    if analyses.is_empty() {
        return Err("no analyses enabled".into());
    }
    let variation = project.tech.variation.as_ref();
    let points = setup.sweep.expand(setup, sch, variation)?;
    let plan = (setup.sweep.monte_carlo.runs > 0).then(|| mc_plan(&setup.sweep.monte_carlo, sch, variation)).transpose()?;
    let mut notes = vec![];
    let pex = if setup.parasitics && c.layout.is_some() {
        match ics_core::pex::post_layout(project, cell) {
            Ok(pl) => Some(pl),
            Err(e) => {
                notes.push(format!("parasitics not included: {e}"));
                None
            }
        }
    } else {
        None
    };
    let mut decks = Vec::with_capacity(points.len());
    for (k, pt) in points.iter().enumerate() {
        let (deck, w) = point_deck(sch, &project.tech, &cell.to_string(), setup, pt)?;
        if k == 0 {
            notes.extend(w.into_iter().map(|w| format!("netlist: {w}")));
        }
        decks.push(match &pex {
            Some(pl) => pl.apply(&deck),
            None => deck,
        });
    }
    let opts = SimOptions { backend: setup.backend, ..opts.clone() };
    let specs = &setup.sweep.specs;
    let total = decks.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<SpecValues>>> = Mutex::new(vec![None; total]);
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).min(total).max(1);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= total {
                        break;
                    }
                    let v: SpecValues = match crate::simulate(&decks[i], &analyses, &opts) {
                        Ok(res) => specs.iter().map(|s| measure(s, &res)).collect(),
                        Err(e) => vec![Err(format!("simulation failed: {e}")); specs.len()],
                    };
                    out.lock().unwrap()[i] = Some(v);
                    progress(done.fetch_add(1, Ordering::SeqCst) + 1, total);
                }
            });
        }
    });
    let values = out.into_inner().unwrap();
    let points = points.into_iter().zip(values).map(|(p, v)| (p, v.unwrap_or_default())).collect();
    Ok(SweepRun { points, plan, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::sweep::{McVar, MonteCarlo, Spec};

    /// The RC low-pass's bandwidth under a 10 % spread of R1: the predicted yield of a bandwidth
    /// window agrees with the simulated runs.
    #[test]
    fn rc_bandwidth_yield() {
        let mut p = ics_core::examples::demo_project();
        let r = CellRef::new("work", "rc_lowpass");
        let c = p.cell_mut(&r).unwrap();
        c.sim.backend = ics_core::Backend::Builtin;
        for a in &mut c.sim.analyses {
            a.enabled = a.analysis.kind_name() == "ac";
        }
        let s = &mut c.sim.sweep;
        *s = Default::default();
        s.specs = vec![Spec {
            name: "bw".into(),
            analysis: "ac".into(),
            expr: "bandwidth(v(out), 3)".into(),
            min: "150k".into(),
            max: "".into(),
        }];
        s.monte_carlo = MonteCarlo {
            runs: 40,
            seed: 5,
            vars: vec![McVar { target: "R1.r".into(), sigma: "10%".into(), ..Default::default() }],
            ..Default::default()
        };
        let run = run(&p, &r, &SimOptions::default(), &|_, _| {}).unwrap();
        assert_eq!(run.points.len(), 40);
        let specs = p.cell(&r).unwrap().sim.sweep.specs.clone();
        let y = run.predict(&specs, 50_000, 1);
        assert_eq!(y.len(), 1);
        let pr = y[0].prediction.specs[0].as_ref().unwrap();
        assert!(pr.model.q2 > 0.99, "{:?}", pr.model);
        assert!(pr.sensitivity[0] > 0.95, "{:?}", pr.sensitivity);
        let sim = y[0].passed as f64 / y[0].runs as f64;
        let (lo, hi) = predict::wilson(y[0].passed, y[0].runs);
        let pred = y[0].prediction.joint.unwrap();
        assert!(y[0].passed > 5 && y[0].passed < 40, "{}", y[0].passed);
        assert!(pred > lo - 0.05 && pred < hi + 0.05, "simulated {sim}, predicted {pred}");
    }

    /// The inverter's switching point under the process variation measured on the example WAT
    /// data: the sweep's Monte Carlo draws the correlated model parameters, and the yield
    /// prediction ranks them.
    #[test]
    fn yield_under_the_measured_process_variation() {
        use ics_core::process_data::{GENERIC180_SPEC, ProcessSpec, WatData};
        let mut p = ics_core::examples::demo_project();
        let spec = ProcessSpec::from_toml(GENERIC180_SPEC).unwrap();
        let csv = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../process/generic180_wat.csv")).unwrap();
        let data = WatData::parse(&csv).unwrap();
        let ex = crate::corners::Extraction::new(&p, &spec, &data, &SimOptions::default()).unwrap();
        let fits = ex.run(40, &mut |_| {});
        let v = ex.variation(&fits[0], "generic180 WAT").unwrap().variation;
        let r = CellRef::new("work", "cmos_inverter");
        let c = p.cell_mut(&r).unwrap();
        c.sim.backend = ics_core::Backend::Builtin;
        let s = &mut c.sim.sweep;
        s.specs = vec![Spec {
            name: "vsw".into(),
            analysis: "dc".into(),
            expr: "cross(v(out), 0.9)".into(),
            min: "0.88".into(),
            max: "".into(),
        }];
        s.monte_carlo = MonteCarlo { runs: 40, seed: 3, process: true, ..Default::default() };
        // Without a measured variation in the technology the setup is an error.
        assert!(run(&p, &r, &SimOptions::default(), &|_, _| {}).unwrap_err().contains("measured process variation"));
        p.tech.variation = Some(v.clone());
        let run = run(&p, &r, &SimOptions::default(), &|_, _| {}).unwrap();
        assert_eq!(run.points.len(), 40);
        let vals: Vec<f64> = run.points.iter().filter_map(|(_, v)| v[0].as_ref().ok().copied()).collect();
        let st = ics_core::sweep::stats(&vals);
        // The same spread as the built-in Monte Carlo with this variation (≈ 891 mV, σ ≈ 10 mV).
        assert!((st.mean - 0.891).abs() < 0.01 && (st.std - 0.010).abs() < 0.004, "{st:?}");
        let plan = run.plan.as_ref().unwrap();
        assert_eq!(plan.inputs, ["nmos.vto (process)", "nmos.kp (process)", "pmos.vto (process)", "pmos.kp (process)"]);
        let specs = p.cell(&r).unwrap().sim.sweep.specs.clone();
        let y = run.predict(&specs, 20_000, 1);
        let pr = y[0].prediction.specs[0].as_ref().unwrap();
        assert!(pr.model.q2 > 0.9, "{:?}", pr.model);
        let pred = y[0].prediction.joint.unwrap();
        let (lo, hi) = predict::wilson(y[0].passed, y[0].runs);
        assert!(pred > lo - 0.05 && pred < hi + 0.05, "simulated {}/{}, predicted {pred}", y[0].passed, y[0].runs);
    }
}
