//! Spec-driven circuit optimizer: variables with ranges, a violation / margin objective over the
//! sweep specs, and a deterministic ask/tell pattern search. Simulation lives in `ics_sim::optimize`.
//!
//! Objective (lower is better, see [`objective`]):
//! - a failing point scores its spec violation [`score`] (> 0);
//! - a passing point scores in `[-1, 0]`, so every passing point beats every failing one:
//!   [`Goal::Margin`] gives `-min(margin, 1)` (larger worst margin is better),
//!   [`Goal::MinArea`] gives `-1 / (1 + area / area_ref)` (smaller area is better).

use serde::{Deserialize, Serialize};

use crate::schematic::{DeviceKind, Schematic};
use crate::sweep::Spec;
use crate::units::parse_spice;

/// Evaluation budget when [`OptSetup::max_evals`] is 0.
pub const DEFAULT_EVALS: usize = 60;
/// Score of one spec whose measurement failed (or is not finite).
pub const FAIL_PENALTY: f64 = 1e3;
/// Pattern-search step (in the unit cube) at which the search stops.
const MIN_DELTA: f64 = 1.0 / 1024.0;
const START_DELTA: f64 = 0.25;

/// An optimised variable: `INST.param` (e.g. `M1.w`) or a design variable substituted as `{name}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct OptVar {
    pub target: String,
    /// Lower bound, SPICE number.
    pub min: String,
    /// Upper bound, SPICE number.
    pub max: String,
    /// Search on a logarithmic scale (both bounds must be > 0).
    pub log: bool,
    /// Grid the value snaps to (multiples of it, e.g. `10n`); empty = continuous.
    pub step: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct OptSetup {
    pub vars: Vec<OptVar>,
    /// Simulation budget; 0 means [`DEFAULT_EVALS`].
    pub max_evals: usize,
    /// Regenerate, route and extract the layout of every candidate and simulate with parasitics.
    pub post_layout: bool,
    pub goal: Goal,
}

/// What to improve once every spec passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    /// Maximise the worst normalised spec margin.
    #[default]
    Margin,
    /// Minimise the MOS gate area (sum of W*L*m*nf), or the sum of the varied `.w` targets.
    MinArea,
}

impl OptSetup {
    pub fn is_empty(&self) -> bool {
        self == &OptSetup::default()
    }

    pub fn budget(&self) -> usize {
        if self.max_evals == 0 { DEFAULT_EVALS } else { self.max_evals }
    }
}

/// A variable's parsed bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: f64,
    pub max: f64,
    pub log: bool,
    /// Grid, 0 = continuous.
    pub step: f64,
}

impl Bounds {
    pub fn of(v: &OptVar) -> Result<Self, String> {
        let t = v.target.trim();
        let num = |label: &str, s: &str| parse_spice(s).ok_or_else(|| format!("{t}: invalid {label} '{s}'"));
        let (min, max) = (num("min", &v.min)?, num("max", &v.max)?);
        if !(min.is_finite() && max.is_finite()) || min > max {
            return Err(format!("{t}: min must not exceed max"));
        }
        if v.log && min <= 0.0 {
            return Err(format!("{t}: a log-scaled variable needs min > 0"));
        }
        let step = if v.step.trim().is_empty() { 0.0 } else { num("step", &v.step)?.abs() };
        Ok(Self { min, max, log: v.log, step })
    }

    /// Real value of unit coordinate `u` (clamped), snapped to the grid.
    pub fn to_real(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        let x = if self.log && self.max > self.min {
            (self.min.ln() + u * (self.max.ln() - self.min.ln())).exp()
        } else {
            self.min + u * (self.max - self.min)
        };
        self.snap(x)
    }

    /// Unit coordinate of real value `x` (clamped).
    pub fn to_unit(&self, x: f64) -> f64 {
        if self.max <= self.min || !x.is_finite() {
            return 0.5;
        }
        let x = x.clamp(self.min, self.max);
        let u = if self.log {
            (x.ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
        } else {
            (x - self.min) / (self.max - self.min)
        };
        u.clamp(0.0, 1.0)
    }

    /// `x` clamped to the bounds and rounded to the nearest grid multiple inside them.
    pub fn snap(&self, x: f64) -> f64 {
        let x = x.clamp(self.min, self.max);
        if self.step <= 0.0 {
            return x;
        }
        let (lo, hi) = ((self.min / self.step - 1e-9).ceil(), (self.max / self.step + 1e-9).floor());
        if lo > hi {
            return self.min;
        }
        let k = (x / self.step).round().clamp(lo, hi);
        // Round off the float noise of k * step (e.g. 3 * 0.1).
        let v = k * self.step;
        if v == 0.0 {
            return v;
        }
        let mag = 10f64.powi(v.abs().log10().floor() as i32 - 12);
        if mag.is_finite() && mag > 0.0 { (v / mag).round() * mag } else { v }
    }
}

fn limits(s: &Spec) -> (Option<f64>, Option<f64>) {
    (parse_spice(&s.min), parse_spice(&s.max))
}

/// Whether the spec has a limit (and so counts in [`score`] / [`margin`]).
pub fn has_limit(s: &Spec) -> bool {
    let (lo, hi) = limits(s);
    lo.is_some() || hi.is_some()
}

/// Scale a limit's violation or margin is divided by: `|limit|`, or 1 when the limit is 0.
fn norm(limit: f64) -> f64 {
    if limit == 0.0 { 1.0 } else { limit.abs() }
}

/// Spec-violation score of measured values: the sum over specs with limits of
/// `(limit - value) / |limit|` below a minimum and `(value - limit) / |limit|` above a maximum
/// (absolute when the limit is 0). A failed or non-finite measurement adds [`FAIL_PENALTY`].
/// 0 means every spec passes; specs without limits are ignored.
pub fn score(specs: &[Spec], values: &[Result<f64, String>]) -> f64 {
    let mut total = 0.0;
    for (k, s) in specs.iter().enumerate() {
        let (lo, hi) = limits(s);
        if lo.is_none() && hi.is_none() {
            continue;
        }
        match values.get(k) {
            Some(Ok(v)) if v.is_finite() => {
                if let Some(l) = lo.filter(|l| v < l) {
                    total += (l - v) / norm(l);
                }
                if let Some(h) = hi.filter(|h| v > h) {
                    total += (v - h) / norm(h);
                }
            }
            _ => total += FAIL_PENALTY,
        }
    }
    total
}

/// The worst normalised margin over specs with limits: per spec the smaller of
/// `(value - min) / |min|` and `(max - value) / |max|` (absolute for a 0 limit); negative when a
/// spec fails, `-FAIL_PENALTY` for a failed measurement, infinite without specs with limits.
pub fn margin(specs: &[Spec], values: &[Result<f64, String>]) -> f64 {
    let mut worst = f64::INFINITY;
    for (k, s) in specs.iter().enumerate() {
        let (lo, hi) = limits(s);
        if lo.is_none() && hi.is_none() {
            continue;
        }
        let m = match values.get(k) {
            Some(Ok(v)) if v.is_finite() => {
                let a = lo.map_or(f64::INFINITY, |l| (v - l) / norm(l));
                let b = hi.map_or(f64::INFINITY, |h| (h - v) / norm(h));
                a.min(b)
            }
            _ => -FAIL_PENALTY,
        };
        worst = worst.min(m);
    }
    worst
}

/// The combined objective (lower is better; see the module docs). `area_ref` is the area of
/// the starting point (only used for [`Goal::MinArea`]).
pub fn objective(goal: Goal, specs: &[Spec], values: &[Result<f64, String>], area: f64, area_ref: f64) -> f64 {
    let s = score(specs, values);
    if s > 0.0 {
        return s;
    }
    match goal {
        Goal::Margin => -margin(specs, values).clamp(0.0, 1.0),
        Goal::MinArea => {
            let r = if area_ref > 0.0 && area.is_finite() { (area / area_ref).max(0.0) } else { 1.0 };
            -1.0 / (1.0 + r)
        }
    }
}

/// Area measure of [`Goal::MinArea`]: the sum over MOS instances of W*L*m*nf (m and nf default
/// to 1); without MOS devices, the sum of the varied `.w` targets' `values`, else 0.
pub fn area(sch: &Schematic, setup: &OptSetup, values: &[f64]) -> f64 {
    let num = |s: &str, d: f64| parse_spice(s).filter(|x| x.is_finite()).unwrap_or(d);
    let mos: Vec<f64> = sch
        .instances
        .iter()
        .filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos))
        .map(|i| num(i.param("w"), 0.0) * num(i.param("l"), 0.0) * num(i.param("m"), 1.0) * num(i.param("nf"), 1.0))
        .collect();
    if !mos.is_empty() {
        return mos.iter().sum();
    }
    setup
        .vars
        .iter()
        .zip(values)
        .filter(|(v, _)| v.target.rsplit_once('.').is_some_and(|(_, p)| p.eq_ignore_ascii_case("w")))
        .map(|(_, x)| *x)
        .sum()
}

/// A SPICE number with an engineering suffix (`meg` for 1e6, `u` for 1e-6) and up to 6
/// significant digits, e.g. `12.5k`, `180n`.
pub fn spice_number(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return if v == 0.0 { "0".into() } else { format!("{v}") };
    }
    const SUFFIXES: [(f64, &str); 11] = [
        (1e12, "t"),
        (1e9, "g"),
        (1e6, "meg"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "u"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
        (1e-18, "a"),
    ];
    let a = v.abs();
    for (scale, p) in SUFFIXES {
        if a >= scale * 0.999_999_5 {
            let x = v / scale;
            let decimals = (5 - x.abs().log10().floor() as i32).clamp(0, 5) as usize;
            let s = format!("{x:.decimals$}");
            let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
            return format!("{s}{p}");
        }
    }
    format!("{v:.5e}")
}

/// A poll direction: axis and sign.
type Dir = (usize, f64);

/// Ask/tell optimizer: compass (pattern) search in the unit cube of the variables (log-scaled
/// for log variables). Starts at the given point (clamped) with step 1/4, polls +/- step along
/// each axis, moves to the first improvement (polling the successful direction first again) and
/// halves the step after a poll without one. Points are snapped to the variables' grids and
/// never evaluated twice. Stops at the evaluation budget or when the step falls below 1/1024.
/// Deterministic.
#[derive(Debug, Clone)]
pub struct Optimizer {
    bounds: Vec<Bounds>,
    budget: usize,
    delta: f64,
    centre: Vec<f64>,
    centre_f: Option<f64>,
    /// Unit points still to poll around the centre.
    queue: Vec<Dir>,
    /// Point last returned by `ask`: (unit coordinates, real values, poll direction).
    pending: Option<(Vec<f64>, Vec<f64>, Option<Dir>)>,
    /// Every evaluated point (real values) and its objective.
    history: Vec<(Vec<f64>, f64)>,
    best: Option<usize>,
    done: bool,
}

impl Optimizer {
    /// `start` is in real units; non-finite entries start at the box centre.
    pub fn new(setup: &OptSetup, start: Vec<f64>) -> Result<Self, String> {
        if setup.vars.is_empty() {
            return Err("no variables to optimise".into());
        }
        let bounds = setup.vars.iter().map(Bounds::of).collect::<Result<Vec<_>, _>>()?;
        let centre = bounds.iter().enumerate().map(|(k, b)| start.get(k).map_or(0.5, |x| b.to_unit(*x))).collect();
        Ok(Self {
            bounds,
            budget: setup.budget(),
            delta: START_DELTA,
            centre,
            centre_f: None,
            queue: vec![],
            pending: None,
            history: vec![],
            best: None,
            done: false,
        })
    }

    pub fn bounds(&self) -> &[Bounds] {
        &self.bounds
    }

    fn real(&self, u: &[f64]) -> Vec<f64> {
        self.bounds.iter().zip(u).map(|(b, u)| b.to_real(*u)).collect()
    }

    fn cached(&self, x: &[f64]) -> Option<f64> {
        self.history.iter().find(|(p, _)| p.as_slice() == x).map(|(_, f)| *f)
    }

    fn poll(&mut self, first: Option<Dir>) {
        self.queue.clear();
        for d in (0..self.bounds.len()).rev() {
            for s in [-1.0, 1.0] {
                if first != Some((d, s)) {
                    self.queue.push((d, s));
                }
            }
        }
        if let Some(f) = first {
            self.queue.push(f);
        }
    }

    /// The next point to evaluate (real units), or `None` when the search is over.
    pub fn ask(&mut self) -> Option<Vec<f64>> {
        if let Some((_, x, _)) = &self.pending {
            return Some(x.clone());
        }
        if self.done || self.history.len() >= self.budget {
            self.done = true;
            return None;
        }
        if self.centre_f.is_none() {
            let x = self.real(&self.centre);
            self.pending = Some((self.centre.clone(), x.clone(), None));
            return Some(x);
        }
        loop {
            let Some((d, s)) = self.queue.pop() else {
                self.delta /= 2.0;
                if self.delta < MIN_DELTA {
                    self.done = true;
                    return None;
                }
                self.poll(None);
                continue;
            };
            let mut u = self.centre.clone();
            u[d] = (u[d] + s * self.delta).clamp(0.0, 1.0);
            let x = self.real(&u);
            // The centre holds the lowest objective so far, so a cached point never improves on it.
            if self.cached(&x).is_some() {
                continue;
            }
            self.pending = Some((u, x.clone(), Some((d, s))));
            return Some(x);
        }
    }

    /// Report the objective (lower is better) of the point last returned by [`Optimizer::ask`].
    pub fn tell(&mut self, objective: f64) {
        let Some((u, x, dir)) = self.pending.take() else { return };
        let f = if objective.is_nan() { f64::INFINITY } else { objective };
        self.history.push((x, f));
        let k = self.history.len() - 1;
        if self.best.is_none_or(|b| f < self.history[b].1) {
            self.best = Some(k);
        }
        match self.centre_f {
            None => {
                self.centre_f = Some(f);
                self.poll(None);
            }
            Some(c) if f < c => {
                self.centre = u;
                self.centre_f = Some(f);
                self.poll(dir);
            }
            Some(_) => {}
        }
    }

    /// The best point evaluated so far (real units) and its objective.
    pub fn best(&self) -> Option<(&[f64], f64)> {
        self.best.map(|b| (self.history[b].0.as_slice(), self.history[b].1))
    }

    /// Evaluations told so far.
    pub fn evals(&self) -> usize {
        self.history.len()
    }

    /// Every evaluated point and its objective, in order.
    pub fn history(&self) -> &[(Vec<f64>, f64)] {
        &self.history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(min: &str, max: &str) -> Spec {
        Spec { name: "s".into(), analysis: "ac".into(), expr: "x".into(), min: min.into(), max: max.into() }
    }

    #[test]
    fn score_and_margin() {
        let specs = [spec("10", ""), spec("", "0"), spec("9k", "11k"), spec("", "")];
        let ok = [Ok(12.0), Ok(-0.5), Ok(10e3), Ok(123.0)];
        assert_eq!(score(&specs, &ok), 0.0);
        assert!((margin(&specs, &ok) - 1.0 / 11.0).abs() < 1e-12, "{}", margin(&specs, &ok));
        let bad = [Ok(5.0), Ok(0.25), Ok(12e3), Err("x".into())];
        assert!((score(&specs, &bad) - (0.5 + 0.25 + 1.0 / 11.0)).abs() < 1e-12);
        assert!((margin(&specs, &bad) - -0.5).abs() < 1e-12);
        let failed = [Err("no".to_string()), Ok(0.0), Ok(10e3), Ok(0.0)];
        assert_eq!(score(&specs, &failed), FAIL_PENALTY);
        assert_eq!(margin(&specs, &failed), -FAIL_PENALTY);
        assert_eq!(score(&specs, &[Ok(f64::NAN), Ok(0.0), Ok(1e4)]), FAIL_PENALTY);
        assert_eq!(margin(&[spec("", "")], &[Ok(1.0)]), f64::INFINITY);
    }

    #[test]
    fn objective_orders_pass_before_fail() {
        let specs = [spec("1", "")];
        let pass_thin = objective(Goal::Margin, &specs, &[Ok(1.0)], 0.0, 0.0);
        let pass_wide = objective(Goal::Margin, &specs, &[Ok(5.0)], 0.0, 0.0);
        let fail = objective(Goal::Margin, &specs, &[Ok(0.999)], 0.0, 0.0);
        assert!(pass_wide < pass_thin && pass_thin <= 0.0 && fail > 0.0);
        assert_eq!(pass_wide, -1.0);
        let small = objective(Goal::MinArea, &specs, &[Ok(1.0)], 1.0, 2.0);
        let large = objective(Goal::MinArea, &specs, &[Ok(9.0)], 4.0, 2.0);
        assert!(small < large && large < 0.0 && small > -1.0);
    }

    #[test]
    fn bounds_and_grid() {
        let b = Bounds::of(&OptVar { target: "M1.w".into(), min: "1u".into(), max: "10u".into(), log: true, step: "10n".into() })
            .unwrap();
        assert!((b.to_real(0.0) - 1e-6).abs() < 1e-15 && (b.to_real(1.0) - 1e-5).abs() < 1e-15);
        let mid = b.to_real(0.5);
        assert!((mid - 3.16e-6).abs() < 1e-12, "{mid}");
        assert!((b.to_unit(mid) - 0.5).abs() < 1e-3);
        assert!(Bounds::of(&OptVar { min: "0".into(), max: "1".into(), log: true, ..Default::default() }).is_err());
        assert!(Bounds::of(&OptVar { min: "2".into(), max: "1".into(), ..Default::default() }).is_err());
        assert_eq!(spice_number(12_500.0), "12.5k");
        assert_eq!(spice_number(1.8e-7), "180n");
        assert_eq!(spice_number(2.2e6), "2.2meg");
        assert_eq!(spice_number(3.0e-6), "3u");
        assert_eq!(spice_number(1.0), "1");
        assert_eq!(parse_spice(&spice_number(1234.56789)), Some(1234.57));
    }

    fn analytic(setup: &OptSetup, start: Vec<f64>) -> Optimizer {
        let mut o = Optimizer::new(setup, start).unwrap();
        while let Some(x) = o.ask() {
            o.tell((x[0] - 3.0).powi(2) + (x[1].log10() - 2.0).powi(2));
        }
        o
    }

    fn two_vars(step: &str) -> OptSetup {
        OptSetup {
            vars: vec![
                OptVar { target: "x".into(), min: "0".into(), max: "10".into(), log: false, step: step.into() },
                OptVar { target: "y".into(), min: "1".into(), max: "1e4".into(), log: true, step: String::new() },
            ],
            max_evals: 80,
            ..Default::default()
        }
    }

    #[test]
    fn converges_on_analytic_function() {
        let setup = two_vars("");
        let o = analytic(&setup, vec![f64::NAN, 3000.0]);
        let (x, f) = o.best().unwrap();
        assert!(o.evals() <= 80);
        assert!((x[0] - 3.0).abs() < 0.05 && (x[1].log10() - 2.0).abs() < 0.02 && f < 3e-3, "{x:?} {f} in {} evals", o.evals());
        let again = analytic(&setup, vec![f64::NAN, 3000.0]);
        assert_eq!(o.history(), again.history(), "deterministic");
        let first = &o.history()[0].0;
        assert!(first[0] == 5.0 && (first[1] - 3000.0).abs() < 1e-6, "starts at the box centre / the given value: {first:?}");
    }

    #[test]
    fn grid_is_respected() {
        let o = analytic(&two_vars("0.7"), vec![5.0, 100.0]);
        for (x, _) in o.history() {
            let k = x[0] / 0.7;
            assert!((k - k.round()).abs() < 1e-9 && (0.0..=10.0).contains(&x[0]), "{x:?}");
        }
        assert!((o.best().unwrap().0[0] - 2.8).abs() < 1e-9, "{:?}", o.best());
        let h = o.history();
        assert!(h.iter().enumerate().all(|(i, a)| h[..i].iter().all(|b| a.0 != b.0)), "no point evaluated twice");
        assert!(o.evals() < 80, "converges before the budget on a grid");
    }

    #[test]
    fn budget_and_errors() {
        let mut setup = two_vars("");
        setup.max_evals = 5;
        assert_eq!(analytic(&setup, vec![]).evals(), 5);
        assert!(Optimizer::new(&OptSetup::default(), vec![]).is_err());
        assert!(OptSetup::default().is_empty() && !setup.is_empty());
        assert_eq!(OptSetup::default().budget(), DEFAULT_EVALS);
    }
}
