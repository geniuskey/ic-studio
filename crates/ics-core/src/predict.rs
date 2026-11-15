//! Yield prediction from Monte Carlo results. Each spec gets a response-surface surrogate over
//! the standardised random inputs of the run ([`crate::sweep::McPlan`]): quadratic when there
//! are enough runs, else linear, fitted by least squares with a small ridge term. Its leave-one-out
//! error (PRESS, from the hat matrix) tells how far it can be trusted and is added back as noise
//! when the surrogate is sampled, so the predicted yield does not claim more than the model knows.
//!
//! Sampling the surrogate costs nothing next to a simulation: 10⁵ samples resolve yields that a
//! few hundred simulated runs cannot, and the main effect of every input gives the sensitivity
//! (share of the spec's variance) that says which variation limits the yield.

use crate::sweep::{Dist, McPlan, Rng, Spec, SweepPoint};
use crate::units::parse_spice;

/// A polynomial model of one spec over the standardised inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct Surrogate {
    pub quadratic: bool,
    inputs: usize,
    /// Coefficients in the spec's unit: constant, linear terms, then x_i x_j for i <= j.
    coef: Vec<f64>,
    /// Runs the model was fitted on.
    pub samples: usize,
    /// Coefficient of determination on the fitted runs.
    pub r2: f64,
    /// Leave-one-out (predictive) coefficient of determination.
    pub q2: f64,
    /// Leave-one-out RMS error (spec unit).
    pub rmse: f64,
}

fn features(x: &[f64], quadratic: bool, out: &mut Vec<f64>) {
    out.clear();
    out.push(1.0);
    out.extend_from_slice(x);
    if quadratic {
        for i in 0..x.len() {
            for j in i..x.len() {
                out.push(x[i] * x[j]);
            }
        }
    }
}

/// Lower-triangular Cholesky factor of a symmetric positive definite `p` × `p` matrix.
pub fn cholesky(a: &[f64], p: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; p * p];
    for i in 0..p {
        for j in 0..=i {
            let s = a[i * p + j] - (0..j).map(|k| l[i * p + k] * l[j * p + k]).sum::<f64>();
            if i == j {
                if s <= 0.0 || s.is_nan() {
                    return None;
                }
                l[i * p + i] = s.sqrt();
            } else {
                l[i * p + j] = s / l[j * p + j];
            }
        }
    }
    Some(l)
}

/// Solve the symmetric positive definite system `a` x = `b` (`a` is `n` × `n`, overwritten).
pub fn solve_spd(a: &mut [f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let l = cholesky(a, n)?;
    let mut x = b.to_vec();
    forward(&l, n, &mut x);
    backward(&l, n, &mut x);
    Some(x)
}

/// Solve L z = b in place.
pub(crate) fn forward(l: &[f64], p: usize, b: &mut [f64]) {
    for i in 0..p {
        b[i] = (b[i] - (0..i).map(|k| l[i * p + k] * b[k]).sum::<f64>()) / l[i * p + i];
    }
}

/// Solve Lᵀ z = b in place.
pub(crate) fn backward(l: &[f64], p: usize, b: &mut [f64]) {
    for i in (0..p).rev() {
        b[i] = (b[i] - (i + 1..p).map(|k| l[k * p + i] * b[k]).sum::<f64>()) / l[i * p + i];
    }
}

impl Surrogate {
    /// Fit `y` over the input draws `x` (one row per run). Quadratic when there are at least
    /// twice as many runs as quadratic terms, linear with at least two more runs than terms.
    pub fn fit(x: &[Vec<f64>], y: &[f64]) -> Result<Surrogate, String> {
        let m = y.len();
        let n = x.first().map_or(0, |r| r.len());
        if x.len() != m || x.iter().any(|r| r.len() != n) {
            return Err("inconsistent samples".into());
        }
        let (pq, pl) = (1 + n + n * (n + 1) / 2, 1 + n);
        let quadratic = n > 0 && m >= 2 * pq;
        if !quadratic && m < pl + 2 {
            return Err(format!("{m} run(s); a model of {n} input(s) needs at least {}", pl + 2));
        }
        let p = if quadratic { pq } else { pl };
        let mean = y.iter().sum::<f64>() / m as f64;
        let sst = y.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
        let scale = if sst > 0.0 { (sst / m as f64).sqrt() } else { 1.0 };
        let mut a = vec![0.0; p * p];
        let mut b = vec![0.0; p];
        let mut f = Vec::with_capacity(p);
        for (xi, yi) in x.iter().zip(y) {
            features(xi, quadratic, &mut f);
            let yz = (yi - mean) / scale;
            for r in 0..p {
                b[r] += f[r] * yz;
                for c in 0..=r {
                    a[r * p + c] += f[r] * f[c];
                }
            }
        }
        for r in 0..p {
            for c in 0..r {
                a[c * p + r] = a[r * p + c];
            }
        }
        // A small ridge keeps nearly collinear terms (few runs, many inputs) well posed.
        for r in 1..p {
            a[r * p + r] += 1e-6 * m as f64;
        }
        let l = cholesky(&a, p).ok_or("the fit is singular")?;
        let mut coef = b;
        forward(&l, p, &mut coef);
        backward(&l, p, &mut coef);
        let (mut sse, mut press) = (0.0, 0.0);
        let mut z = vec![0.0; p];
        for (xi, yi) in x.iter().zip(y) {
            features(xi, quadratic, &mut f);
            let e = (yi - mean) / scale - f.iter().zip(&coef).map(|(a, b)| a * b).sum::<f64>();
            z.copy_from_slice(&f);
            forward(&l, p, &mut z);
            let h = z.iter().map(|v| v * v).sum::<f64>().min(1.0 - 1e-9);
            sse += e * e;
            press += (e / (1.0 - h)).powi(2);
        }
        let sst_z = sst / (scale * scale);
        let (r2, q2) = if sst > 0.0 { (1.0 - sse / sst_z, 1.0 - press / sst_z) } else { (1.0, 1.0) };
        for c in &mut coef {
            *c *= scale;
        }
        coef[0] += mean;
        Ok(Surrogate { quadratic, inputs: n, coef, samples: m, r2, q2, rmse: (press / m as f64).sqrt() * scale })
    }

    /// The model's value at the draws `x`.
    pub fn eval(&self, x: &[f64]) -> f64 {
        let n = self.inputs;
        let mut y = self.coef[0] + (0..n).map(|i| self.coef[1 + i] * x[i]).sum::<f64>();
        if self.quadratic {
            let mut k = 1 + n;
            for i in 0..n {
                for j in i..n {
                    y += self.coef[k] * x[i] * x[j];
                    k += 1;
                }
            }
        }
        y
    }

    /// Change of the spec per unit draw of input `i` at the nominal point.
    pub fn slope(&self, i: usize) -> f64 {
        self.coef[1 + i]
    }

    /// Variance of the main effect of input `i` (its terms alone: b xᵢ + c xᵢ²).
    pub fn main_effect_variance(&self, i: usize, d: Dist) -> f64 {
        let b = self.coef[1 + i];
        let c = if self.quadratic {
            let n = self.inputs;
            // Index of x_i x_i: the terms of rows 0..i come first.
            let k = 1 + n + (0..i).map(|r| n - r).sum::<usize>();
            self.coef[k]
        } else {
            0.0
        };
        match d {
            // Var(x) = 1, Var(x²) = 2, Cov(x, x²) = 0.
            Dist::Gauss => b * b + 2.0 * c * c,
            // U(-1, 1): Var(x) = 1/3, Var(x²) = 4/45, Cov(x, x²) = 0.
            Dist::Uniform => b * b / 3.0 + 4.0 * c * c / 45.0,
        }
    }
}

/// Lower and upper limit of a spec.
pub type Limits = (Option<f64>, Option<f64>);

fn passes(v: f64, (lo, hi): Limits) -> bool {
    v.is_finite() && lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h)
}

/// The prediction of one spec.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecPrediction {
    pub model: Surrogate,
    /// Mean and standard deviation of the sampled surrogate (with its noise).
    pub mean: f64,
    pub std: f64,
    /// Share of the spec's samples within its limits (`None` without limits).
    pub yield_: Option<f64>,
    /// Process capability: distance of the mean to the nearer limit in units of 3σ.
    pub cpk: Option<f64>,
    /// Share of the spec's variance due to each input's main effect.
    pub sensitivity: Vec<f64>,
}

/// Predicted yield of a Monte Carlo run.
#[derive(Debug, Clone, PartialEq)]
pub struct YieldPrediction {
    /// Per spec, its prediction or why there is none.
    pub specs: Vec<Result<SpecPrediction, String>>,
    /// Share of samples meeting every limited spec (`None` when a limited spec has no model or
    /// no spec has limits).
    pub joint: Option<f64>,
    /// Surrogate samples drawn.
    pub samples: usize,
}

/// Fit a surrogate of every spec over the run draws `x` and sample them `samples` times with
/// the input distributions `dists`. `values[k][r]` is spec `k` in run `r` (`None` where the
/// run gave no value; those runs are left out of that spec's fit).
pub fn predict(
    dists: &[Dist],
    x: &[Vec<f64>],
    values: &[Vec<Option<f64>>],
    limits: &[Limits],
    samples: usize,
    seed: u64,
) -> YieldPrediction {
    let models: Vec<Result<Surrogate, String>> = values
        .iter()
        .map(|v| {
            let (xs, ys): (Vec<Vec<f64>>, Vec<f64>) =
                x.iter().zip(v).filter_map(|(x, y)| y.filter(|y| y.is_finite()).map(|y| (x.clone(), y))).unzip();
            Surrogate::fit(&xs, &ys)
        })
        .collect();
    let k = values.len();
    let limited: Vec<bool> = (0..k).map(|i| limits.get(i).is_some_and(|l| l.0.is_some() || l.1.is_some())).collect();
    let mut rng = Rng(seed ^ 0x005e_ed0f_9a11);
    let (mut sum, mut sum2, mut pass) = (vec![0.0; k], vec![0.0; k], vec![0usize; k]);
    let mut joint = 0usize;
    let mut xs = vec![0.0; dists.len()];
    for _ in 0..samples {
        for (v, d) in xs.iter_mut().zip(dists) {
            *v = rng.draw(*d);
        }
        let mut all = true;
        for i in 0..k {
            let Ok(m) = &models[i] else { continue };
            let y = m.eval(&xs) + m.rmse * rng.draw(Dist::Gauss);
            sum[i] += y;
            sum2[i] += y * y;
            if limited[i] {
                let ok = passes(y, limits[i]);
                pass[i] += usize::from(ok);
                all &= ok;
            }
        }
        joint += usize::from(all);
    }
    let n = samples.max(1) as f64;
    let specs = models
        .into_iter()
        .enumerate()
        .map(|(i, m)| {
            let model = m?;
            let mean = sum[i] / n;
            let var = (sum2[i] / n - mean * mean).max(0.0);
            let std = var.sqrt();
            let (lo, hi) = limits.get(i).copied().unwrap_or((None, None));
            let cpk = if std > 0.0 {
                [lo.map(|l| (mean - l) / (3.0 * std)), hi.map(|h| (h - mean) / (3.0 * std))]
                    .into_iter()
                    .flatten()
                    .reduce(f64::min)
            } else {
                None
            };
            let sensitivity = (0..dists.len())
                .map(|j| if var > 0.0 { (model.main_effect_variance(j, dists[j]) / var).min(1.0) } else { 0.0 })
                .collect();
            Ok(SpecPrediction { model, mean, std, yield_: limited[i].then(|| pass[i] as f64 / n), cpk, sensitivity })
        })
        .collect::<Vec<Result<SpecPrediction, String>>>();
    let any_limited = limited.iter().any(|l| *l);
    let complete = specs.iter().zip(&limited).all(|(s, l)| !l || s.is_ok());
    YieldPrediction { specs, joint: (any_limited && complete).then(|| joint as f64 / n), samples }
}

/// The Monte Carlo runs of one sweep condition (corner, temperature, swept values) and their
/// predicted yield.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionYield {
    pub condition: String,
    /// Runs with a result.
    pub runs: usize,
    /// Simulated runs meeting every limited spec (a missing value fails).
    pub passed: usize,
    pub prediction: YieldPrediction,
}

/// Predict the yield of every condition of a Monte Carlo sweep: `points` are the finished points
/// with the value of every spec, `plan` the setup's inputs and draws.
pub fn predict_sweep(
    plan: &McPlan,
    specs: &[Spec],
    points: &[(&SweepPoint, Vec<Option<f64>>)],
    samples: usize,
    seed: u64,
) -> Vec<ConditionYield> {
    let limits: Vec<Limits> = specs.iter().map(|s| (parse_spice(&s.min), parse_spice(&s.max))).collect();
    let mut order: Vec<String> = vec![];
    for (p, _) in points {
        let c = p.condition();
        if p.run.is_some() && !order.contains(&c) {
            order.push(c);
        }
    }
    order
        .into_iter()
        .map(|cond| {
            let mut x = vec![];
            let mut values = vec![vec![]; specs.len()];
            let mut passed = 0;
            for (p, v) in points.iter().filter(|(p, _)| p.condition() == cond) {
                let Some(d) = p.run.and_then(|r| plan.draws.get(r as usize - 1)) else { continue };
                x.push(d.clone());
                for (k, col) in values.iter_mut().enumerate() {
                    col.push(v.get(k).copied().flatten());
                }
                let ok = limits
                    .iter()
                    .enumerate()
                    .all(|(k, l)| (l.0.is_none() && l.1.is_none()) || v.get(k).copied().flatten().is_some_and(|y| passes(y, *l)));
                passed += usize::from(ok);
            }
            let prediction = predict(&plan.dists, &x, &values, &limits, samples, seed);
            ConditionYield { condition: cond, runs: x.len(), passed, prediction }
        })
        .collect()
}

/// 95 % Wilson score interval of a yield of `pass` out of `n` runs.
pub fn wilson(pass: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let (z, nf) = (1.96f64, n as f64);
    let p = pass as f64 / nf;
    let d = 1.0 + z * z / nf;
    let c = (p + z * z / (2.0 * nf)) / d;
    let h = z * (p * (1.0 - p) / nf + z * z / (4.0 * nf * nf)).sqrt() / d;
    ((c - h).max(0.0), (c + h).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draws(n: usize, runs: usize, seed: u64) -> Vec<Vec<f64>> {
        let mut rng = Rng(seed);
        (0..runs).map(|_| (0..n).map(|_| rng.draw(Dist::Gauss)).collect()).collect()
    }

    #[test]
    fn fits_a_quadratic_response() {
        let x = draws(3, 100, 1);
        let f = |x: &[f64]| 2.0 + 0.5 * x[0] - 0.2 * x[1] + 0.1 * x[0] * x[0] + 0.05 * x[0] * x[2];
        let y: Vec<f64> = x.iter().map(|x| f(x)).collect();
        let s = Surrogate::fit(&x, &y).unwrap();
        assert!(s.quadratic && s.q2 > 0.999 && s.rmse < 1e-4, "{s:?}");
        let p = [0.3, -1.2, 0.7];
        assert!((s.eval(&p) - f(&p)).abs() < 1e-4);
        assert!((s.slope(0) - 0.5).abs() < 1e-4 && (s.slope(1) + 0.2).abs() < 1e-4);
        assert!((s.main_effect_variance(0, Dist::Gauss) - (0.25 + 2.0 * 0.01)).abs() < 1e-4);
        // Too few runs for the quadratic: linear, and an error below the minimum.
        assert!(!Surrogate::fit(&x[..12], &y[..12]).unwrap().quadratic);
        assert!(Surrogate::fit(&x[..4], &y[..4]).is_err());
    }

    #[test]
    fn predicts_yield_with_noise() {
        // y = x0 + 0.5 x1 + noise: σ = sqrt(1.25 + 0.04) ≈ 1.136; limit y <= 2 → Φ(2 / 1.136) ≈ 0.961.
        let x = draws(2, 400, 3);
        let mut rng = Rng(9);
        let y: Vec<Option<f64>> = x.iter().map(|x| Some(x[0] + 0.5 * x[1] + 0.2 * rng.draw(Dist::Gauss))).collect();
        let lim = [(None, Some(2.0))];
        let p = predict(&[Dist::Gauss, Dist::Gauss], &x, &[y], &lim, 200_000, 1);
        let s = p.specs[0].as_ref().unwrap();
        assert!((s.yield_.unwrap() - 0.961).abs() < 0.01, "{:?}", s.yield_);
        assert!((p.joint.unwrap() - s.yield_.unwrap()).abs() < 1e-12);
        assert!((s.std - 1.136).abs() < 0.05, "{}", s.std);
        assert!((s.cpk.unwrap() - 2.0 / (3.0 * 1.136)).abs() < 0.03);
        // x0 carries 1 / 1.29 of the variance, x1 0.25 / 1.29.
        assert!((s.sensitivity[0] - 0.775).abs() < 0.04 && (s.sensitivity[1] - 0.194).abs() < 0.03, "{:?}", s.sensitivity);
        // A spec without values has no model, so the joint yield is unknown.
        let p = predict(&[Dist::Gauss, Dist::Gauss], &x, &[vec![None; 400]], &lim, 100, 1);
        assert!(p.specs[0].is_err() && p.joint.is_none());
    }

    #[test]
    fn sweep_conditions() {
        use crate::schematic::Schematic;
        use crate::sweep::{McVar, MonteCarlo, SweepSetup};
        let setup = SweepSetup {
            temps: "0 50".into(),
            monte_carlo: MonteCarlo {
                runs: 60,
                seed: 2,
                vars: vec![McVar { target: "vb".into(), nominal: "1".into(), sigma: "0.1".into(), ..Default::default() }],
                ..Default::default()
            },
            ..Default::default()
        };
        let sch = Schematic::default();
        let plan = crate::sweep::mc_plan(&setup.monte_carlo, &sch, None).unwrap();
        let pts = setup.expand(&crate::SimSetup::default(), &sch, None).unwrap();
        // The spec is vb (+ 0.05 at 50 °C) with vb <= 1.1: yield Φ(1) ≈ 0.84 and Φ(0.5) ≈ 0.69.
        let vals: Vec<(&SweepPoint, Vec<Option<f64>>)> = pts
            .iter()
            .map(|p| {
                let vb: f64 = parse_spice(&p.vars[0].1).unwrap();
                (p, vec![Some(vb + if p.temp > 0.0 { 0.05 } else { 0.0 })])
            })
            .collect();
        let spec = Spec { name: "vb".into(), max: "1.1".into(), ..Default::default() };
        let c = predict_sweep(&plan, &[spec], &vals, 100_000, 1);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].condition.as_str(), c[0].runs), ("0°C", 60));
        assert!((c[0].prediction.joint.unwrap() - 0.841).abs() < 0.01, "{:?}", c[0].prediction.joint);
        assert!((c[1].prediction.joint.unwrap() - 0.691).abs() < 0.01, "{:?}", c[1].prediction.joint);
        assert!(c[1].passed < c[0].passed);
    }

    #[test]
    fn wilson_interval() {
        let (lo, hi) = wilson(50, 50);
        assert!(hi == 1.0 && (lo - 0.929).abs() < 0.002, "{lo}");
        let (lo, hi) = wilson(90, 100);
        assert!(lo < 0.9 && hi > 0.9 && hi - lo < 0.13);
    }
}
