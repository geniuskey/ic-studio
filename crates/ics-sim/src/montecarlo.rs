//! Monte Carlo on the built-in engine: every run draws a global (process) shift of each
//! polarity's threshold and transconductance plus a per-device mismatch scaled by area
//! (Pelgrom: `σ(ΔVth) = avt / sqrt(W L)`, `σ(Δβ/β) = abeta / sqrt(W L)`), simulates, and
//! evaluates waveform-calculator measurements on the results.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ics_core::Analysis;

use crate::calc::{self, CalcValue};
use crate::circuit::{Element, parse_deck};
use crate::engine::Engine;
use crate::results::SimResults;
use crate::{SimError, external_models};

#[derive(Debug, Clone, PartialEq)]
pub struct McSpec {
    pub runs: usize,
    pub seed: u64,
    /// Global threshold shift per polarity (V, one sigma).
    pub vto_sigma: f64,
    /// Global relative transconductance shift per polarity (one sigma).
    pub kp_sigma: f64,
    /// Threshold mismatch coefficient (V·m): 5 mV·µm is 5e-9.
    pub avt: f64,
    /// Current factor mismatch coefficient (m): 1 %·µm is 1e-8.
    pub abeta: f64,
}

impl Default for McSpec {
    fn default() -> Self {
        Self { runs: 100, seed: 1, vto_sigma: 0.02, kp_sigma: 0.03, avt: 5e-9, abeta: 1e-8 }
    }
}

/// A measurement: `expr` (waveform calculator, scalar result) on the dataset of `analysis`.
#[derive(Debug, Clone, PartialEq)]
pub struct Measure {
    pub analysis: String,
    pub expr: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stats {
    pub mean: f64,
    pub sigma: f64,
    pub min: f64,
    pub max: f64,
    /// Runs the measurement produced a value for.
    pub n: usize,
}

impl Stats {
    pub fn of(values: &[f64]) -> Self {
        let n = values.len();
        if n == 0 {
            return Self::default();
        }
        let mean = values.iter().sum::<f64>() / n as f64;
        let var = if n > 1 { values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64 } else { 0.0 };
        Self {
            mean,
            sigma: var.sqrt(),
            min: values.iter().copied().fold(f64::INFINITY, f64::min),
            max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            n,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct McResult {
    /// Per measure, the value of every run that produced one (run index, value).
    pub values: Vec<Vec<(usize, f64)>>,
    pub stats: Vec<Stats>,
    /// Runs whose simulation failed, with the error.
    pub failed: Vec<(usize, String)>,
    /// Runs carried out (fewer than asked when cancelled).
    pub runs: usize,
    pub cancelled: bool,
}

/// Progress and cancellation of a run from another thread.
#[derive(Debug, Default)]
pub struct McControl {
    /// Runs finished so far.
    pub done: AtomicUsize,
    pub cancel: AtomicBool,
}

/// SplitMix64 with Box-Muller normals: reproducible for a seed, no dependencies.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (2.0 * std::f64::consts::PI * self.uniform()).cos()
    }
}

pub fn monte_carlo(
    deck: &str,
    analyses: &[Analysis],
    measures: &[Measure],
    spec: &McSpec,
    ctl: Option<&McControl>,
) -> Result<McResult, SimError> {
    if let Some(l) = external_models(deck) {
        return Err(SimError::Engine(format!(
            "Monte Carlo varies the built-in engine's models; the deck loads external models ('{l}'), use the PDK's own statistical corners with ngspice"
        )));
    }
    let base = parse_deck(deck)?;
    let analyses: Vec<Analysis> = if analyses.is_empty() { base.analyses.clone() } else { analyses.to_vec() };
    if analyses.is_empty() {
        return Err(SimError::Engine("no analyses to run".into()));
    }
    let mut rng = Rng(spec.seed);
    let mut out = McResult { values: vec![vec![]; measures.len()], ..Default::default() };
    for run in 0..spec.runs {
        if let Some(ctl) = ctl {
            ctl.done.store(run, Ordering::Relaxed);
            if ctl.cancel.load(Ordering::Relaxed) {
                out.cancelled = true;
                break;
            }
        }
        out.runs = run + 1;
        // Global shifts per polarity: (nmos, pmos).
        let dvto = [rng.normal() * spec.vto_sigma, rng.normal() * spec.vto_sigma];
        let dkp = [rng.normal() * spec.kp_sigma, rng.normal() * spec.kp_sigma];
        let mut c = base.clone();
        for e in &mut c.elements {
            if let Element::M { model, w, l, m, .. } = e {
                let p = model.pmos as usize;
                let area = (*w * model.leff(*l) * *m).max(1e-18).sqrt();
                let dv = dvto[p] + rng.normal() * spec.avt / area;
                let db = dkp[p] + rng.normal() * spec.abeta / area;
                model.vto += if model.pmos { -dv } else { dv };
                model.kp *= (1.0 + db).max(0.05);
            }
        }
        let res = Engine::new(c).and_then(|mut eng| analyses.iter().map(|a| eng.run(a)).collect::<Result<Vec<_>, _>>());
        let datasets = match res {
            Ok(d) => d,
            Err(e) => {
                out.failed.push((run, e.to_string()));
                continue;
            }
        };
        let res = SimResults { backend: "built-in".into(), datasets, ..Default::default() };
        for (k, ms) in measures.iter().enumerate() {
            let v = match res.dataset(&ms.analysis).map(|d| calc::eval(&ms.expr, d)) {
                Some(Ok(CalcValue::Scalar(v))) => v,
                // An operating point is a one-point wave.
                Some(Ok(CalcValue::Wave(w))) if w.re.len() == 1 && w.im.is_none() => w.re[0],
                _ => continue,
            };
            if v.is_finite() {
                out.values[k].push((run, v));
            }
        }
    }
    if let Some(ctl) = ctl {
        ctl.done.store(out.runs, Ordering::Relaxed);
    }
    out.stats = out.values.iter().map(|v| Stats::of(&v.iter().map(|p| p.1).collect::<Vec<_>>())).collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normals_have_unit_variance() {
        let mut r = Rng(7);
        let v: Vec<f64> = (0..20000).map(|_| r.normal()).collect();
        let s = Stats::of(&v);
        assert!(s.mean.abs() < 0.03 && (s.sigma - 1.0).abs() < 0.03, "{s:?}");
    }

    /// A diode-connected NMOS fed by a current: vgs = vto + sqrt(2 I / beta). Mismatch only, so
    /// sigma(vgs) is about avt / sqrt(W L) plus the beta part.
    #[test]
    fn mismatch_scales_with_area() {
        let deck = |w: &str| format!("* t\nI1 0 g 10u\nM1 g g 0 0 nm w={w} l=1u\n.model nm nmos (vto=0.5 kp=100u)\n.op\n.end\n");
        let spec = McSpec { runs: 400, seed: 3, vto_sigma: 0.0, kp_sigma: 0.0, avt: 5e-9, abeta: 0.0 };
        let ms = [Measure { analysis: "op".into(), expr: "v(g)".into() }];
        let small = monte_carlo(&deck("1u"), &[], &ms, &spec, None).unwrap();
        let big = monte_carlo(&deck("4u"), &[], &ms, &spec, None).unwrap();
        let (s1, s4) = (small.stats[0].sigma, big.stats[0].sigma);
        assert!(small.failed.is_empty() && small.stats[0].n == 400);
        assert!((s1 / 5e-3 - 1.0).abs() < 0.12, "sigma at 1u x 1u: {s1}");
        assert!((s1 / s4 - 2.0).abs() < 0.3, "4x area halves sigma: {s1} vs {s4}");
        let again = monte_carlo(&deck("1u"), &[], &ms, &spec, None).unwrap();
        assert_eq!(again.values, small.values, "a seed reproduces the runs");
    }

    #[test]
    fn cancel_stops_early_and_reports_progress() {
        let deck = "* t\nI1 0 g 10u\nM1 g g 0 0 nm w=1u l=1u\n.model nm nmos (vto=0.5 kp=100u)\n.op\n.end\n";
        let ms = [Measure { analysis: "op".into(), expr: "v(g)".into() }];
        let spec = McSpec { runs: 50, ..Default::default() };
        let ctl = McControl::default();
        let r = monte_carlo(deck, &[], &ms, &spec, Some(&ctl)).unwrap();
        assert_eq!((r.runs, r.cancelled, ctl.done.load(Ordering::Relaxed)), (50, false, 50));
        ctl.cancel.store(true, Ordering::Relaxed);
        let r = monte_carlo(deck, &[], &ms, &spec, Some(&ctl)).unwrap();
        assert_eq!((r.runs, r.cancelled, r.stats[0].n), (0, true, 0));
    }

    #[test]
    fn rejects_external_models() {
        let e = monte_carlo("* t\n.lib \"x.lib\" tt\nR1 a 0 1\n.op\n.end\n", &[], &[], &McSpec::default(), None).unwrap_err();
        assert!(e.to_string().contains("statistical"), "{e}");
    }
}
