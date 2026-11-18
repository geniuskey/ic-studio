//! Corner and parametric sweeps (ADE Explorer / Assembler style): every combination of model
//! corners, temperatures and variable values is one simulation point, and specs are checked on
//! the results of each.

use serde::{Deserialize, Serialize};

use crate::netlist::netlist_for;
use crate::process_data::ProcessVariation;
use crate::schematic::Schematic;
use crate::sim_setup::SimSetup;
use crate::tech::Tech;
use crate::units::parse_spice;

/// Upper bound on the number of points of one sweep.
pub const MAX_POINTS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SweepSetup {
    /// Model corners to run; empty runs the simulation setup's corner only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corners: Vec<String>,
    /// Temperatures in °C as a value list ([`parse_values`]); empty runs the setup's temperature.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub temps: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<SweepVar>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<Spec>,
    #[serde(skip_serializing_if = "MonteCarlo::is_off")]
    pub monte_carlo: MonteCarlo,
}

/// Monte Carlo: `runs` random samples per sweep point of the varied parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MonteCarlo {
    /// Samples per point; 0 turns Monte Carlo off.
    pub runs: u32,
    pub seed: u64,
    pub vars: Vec<McVar>,
    /// Also draw the technology's measured process variation (`Tech::variation`): its model
    /// parameters, correlated, shared by every device (`model:NAME.PARAM` targets).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub process: bool,
}

impl MonteCarlo {
    pub fn is_off(&self) -> bool {
        self.runs == 0 && self.vars.is_empty() && !self.process
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Dist {
    #[default]
    Gauss,
    Uniform,
}

/// A varied parameter. `target` is `INST.param` with `*` / `?` wildcards in the instance name
/// (`M*.w`, `R1.r`) or a design variable name (replacing `{name}`, with `nominal` as its value).
/// MOS threshold and mobility variation go through the instance parameters `delvto` (nominal
/// 0) and `mulu0` (nominal 1), which BSIM models in both engines evaluate: `M*.delvto` with
/// sigma `5m/sqrt(wl)` and `mismatch` draws Pelgrom mismatch, 5 mV·µm over √(W·L) in µm².
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct McVar {
    pub target: String,
    /// Nominal value of a design variable (instance parameters use the instance's value).
    pub nominal: String,
    /// Standard deviation (Gauss) or half range (Uniform): `5%` of the nominal, a SPICE number,
    /// or `A/sqrt(wl)` for an area-scaled (Pelgrom) value: `A` divided by the square root of the
    /// instance's W·L in µm².
    pub sigma: String,
    pub dist: Dist,
    /// Draw each matching instance independently (mismatch); otherwise one draw per run is shared
    /// by all of them (process variation).
    pub mismatch: bool,
}

/// A swept variable: `INST.param` sets an instance parameter (e.g. `R1.r`, `M1.w`); any other
/// name is a design variable, substituted wherever the deck writes `{name}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SweepVar {
    pub name: String,
    /// Value list ([`parse_values`]).
    pub values: String,
}

/// A measured quantity with optional limits: `expr` is a waveform calculator expression evaluated
/// on the `analysis` dataset (`dc`, `ac`, `tran`, `op`) of each point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Spec {
    pub name: String,
    pub analysis: String,
    pub expr: String,
    /// Lower limit as a SPICE number (empty = none).
    pub min: String,
    /// Upper limit as a SPICE number (empty = none).
    pub max: String,
}

impl Spec {
    /// Whether `v` meets the limits; `None` when the spec has none.
    pub fn check(&self, v: f64) -> Option<bool> {
        let (lo, hi) = (parse_spice(&self.min), parse_spice(&self.max));
        if lo.is_none() && hi.is_none() {
            return None;
        }
        Some(v.is_finite() && lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h))
    }
}

/// One combination of the sweep.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepPoint {
    pub corner: String,
    pub temp: f64,
    pub vars: Vec<(String, String)>,
    /// Monte Carlo run number (1-based); its sampled values follow the swept ones in `vars`.
    pub run: Option<u32>,
    /// Number of leading `vars` that are swept.
    pub swept: usize,
}

impl SweepPoint {
    /// The label without the Monte Carlo run: corner, temperature and swept values.
    pub fn condition(&self) -> String {
        SweepPoint { run: None, ..self.clone() }.label()
    }

    pub fn label(&self) -> String {
        let mut parts = vec![];
        if !self.corner.is_empty() {
            parts.push(self.corner.clone());
        }
        parts.push(format!("{}°C", self.temp));
        parts.extend(self.vars.iter().take(self.swept).map(|(n, v)| format!("{n}={v}")));
        if let Some(k) = self.run {
            parts.push(format!("mc#{k}"));
        }
        parts.join(" ")
    }
}

/// Values separated by spaces or commas; `start:step:stop` expands to a linear range.
pub fn parse_values(s: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    for tok in s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()) {
        let parts: Vec<&str> = tok.split(':').collect();
        match parts.as_slice() {
            [v] => {
                parse_spice(v).ok_or_else(|| format!("invalid number '{v}'"))?;
                out.push(v.to_string());
            }
            [a, st, b] => {
                let num = |v: &str| parse_spice(v).ok_or_else(|| format!("invalid number '{v}' in '{tok}'"));
                let (a, st, b) = (num(a)?, num(st)?, num(b)?);
                if st == 0.0 || (b - a) * st < 0.0 {
                    return Err(format!("range '{tok}': the step must be non-zero and go from start towards stop"));
                }
                let n = ((b - a) / st + 1e-9).floor() as usize + 1;
                if n > MAX_POINTS {
                    return Err(format!("range '{tok}' has more than {MAX_POINTS} values"));
                }
                out.extend((0..n).map(|k| fmt_value(a + k as f64 * st)));
            }
            _ => return Err(format!("invalid value '{tok}' (use a number or start:step:stop)")),
        }
    }
    Ok(out)
}

fn fmt_value(v: f64) -> String {
    let s = format!("{v:.9}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

impl SweepSetup {
    pub fn is_empty(&self) -> bool {
        self == &SweepSetup::default()
    }

    /// Every combination of corners, temperatures and variable values, corners outermost.
    pub fn points(&self, base: &SimSetup) -> Result<Vec<SweepPoint>, String> {
        let corners = if self.corners.is_empty() { vec![base.corner.clone()] } else { self.corners.clone() };
        let temps: Vec<f64> = if self.temps.trim().is_empty() {
            vec![base.temp]
        } else {
            parse_values(&self.temps).map_err(|e| format!("temperatures: {e}"))?.iter().filter_map(|t| parse_spice(t)).collect()
        };
        let mut axes: Vec<(String, Vec<String>)> = vec![];
        for v in self.vars.iter().filter(|v| !v.name.trim().is_empty()) {
            let vals = parse_values(&v.values).map_err(|e| format!("{}: {e}", v.name))?;
            if vals.is_empty() {
                return Err(format!("{}: no values", v.name));
            }
            axes.push((v.name.trim().to_string(), vals));
        }
        let total = axes.iter().fold(corners.len() * temps.len(), |n, (_, v)| n.saturating_mul(v.len()));
        if total > MAX_POINTS {
            return Err(format!("{total} points exceed the limit of {MAX_POINTS}"));
        }
        let mut out = Vec::with_capacity(total);
        for c in &corners {
            for &t in &temps {
                let mut idx = vec![0usize; axes.len()];
                'combos: loop {
                    let vars = axes.iter().zip(&idx).map(|((n, v), &k)| (n.clone(), v[k].clone())).collect();
                    out.push(SweepPoint { corner: c.clone(), temp: t, vars, run: None, swept: axes.len() });
                    for d in (0..axes.len()).rev() {
                        idx[d] += 1;
                        if idx[d] < axes[d].1.len() {
                            continue 'combos;
                        }
                        idx[d] = 0;
                    }
                    break;
                }
            }
        }
        Ok(out)
    }
}

impl SweepSetup {
    /// [`SweepSetup::points`] with every point repeated for each Monte Carlo run, its sampled
    /// values appended.
    pub fn expand(
        &self,
        base: &SimSetup,
        sch: &Schematic,
        variation: Option<&ProcessVariation>,
    ) -> Result<Vec<SweepPoint>, String> {
        let points = self.points(base)?;
        let mc = &self.monte_carlo;
        if mc.runs == 0 {
            return Ok(points);
        }
        let total = points.len().saturating_mul(mc.runs as usize);
        if total > MAX_POINTS {
            return Err(format!("{total} points exceed the limit of {MAX_POINTS}"));
        }
        let samples = mc_samples(mc, sch, variation)?;
        let mut out = Vec::with_capacity(total);
        for p in points {
            for (k, s) in samples.iter().enumerate() {
                let mut q = p.clone();
                q.vars.extend(s.iter().cloned());
                q.run = Some(k as u32 + 1);
                out.push(q);
            }
        }
        Ok(out)
    }
}

/// The random inputs of a Monte Carlo setup and the draws of every run: each input is one
/// independent draw (a shared process variation, or one instance's mismatch), standardised
/// (Gauss: N(0, 1); Uniform: U(-1, 1)).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct McPlan {
    /// Name of each input: the target (`M*.w`, `vb`), or `INST.param` of a mismatch draw.
    pub inputs: Vec<String>,
    pub dists: Vec<Dist>,
    /// Draws of every run (`runs` × `inputs`).
    pub draws: Vec<Vec<f64>>,
    targets: Vec<McTarget>,
}

#[derive(Debug, Clone, PartialEq)]
struct McTarget {
    name: String,
    nominal: f64,
    /// (input, coefficient): the value is `nominal + Σ coefficient · draw[input]`.
    terms: Vec<(usize, f64)>,
}

impl McPlan {
    /// Vary `name` by `term`. Several variables on one parameter (a process shift and a
    /// mismatch of `M1.delvto`) add up instead of the last one replacing the others.
    fn add_term(&mut self, name: String, nominal: f64, term: (usize, f64)) {
        match self.targets.iter_mut().find(|t| t.name.eq_ignore_ascii_case(&name)) {
            Some(t) => t.terms.push(term),
            None => self.targets.push(McTarget { name, nominal, terms: vec![term] }),
        }
    }

    /// The parameter values of the draws `x` (one per input).
    pub fn assign(&self, x: &[f64]) -> Vec<(String, String)> {
        self.targets
            .iter()
            .map(|t| (t.name.clone(), format!("{:.6e}", t.nominal + t.terms.iter().map(|(i, c)| c * x[*i]).sum::<f64>())))
            .collect()
    }
}

/// The inputs and draws of a Monte Carlo setup on `sch`; `variation` is the technology's
/// measured process variation, drawn when the setup asks for it.
pub fn mc_plan(mc: &MonteCarlo, sch: &Schematic, variation: Option<&ProcessVariation>) -> Result<McPlan, String> {
    let mut plan = McPlan::default();
    for v in mc.vars.iter().filter(|v| !v.target.trim().is_empty()) {
        let t = v.target.trim();
        // `inst` gives the W·L of an area-scaled sigma.
        let sigma_of = |nominal: f64, inst: Option<&crate::schematic::Instance>| -> Result<f64, String> {
            let s = v.sigma.trim();
            let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
            if let Some(a) = compact.strip_suffix("/sqrt(wl)") {
                let a = parse_spice(a).ok_or_else(|| format!("{t}: invalid sigma '{s}'"))?;
                let inst = inst.ok_or_else(|| format!("{t}: '{s}' needs an instance with W and L"))?;
                let dim = |k: &str| {
                    parse_spice(inst.param(k))
                        .filter(|x| *x > 0.0)
                        .ok_or_else(|| format!("{t}: '{s}' needs {}.{k}, which is '{}'", inst.name, inst.param(k)))
                };
                // W and L in µm, whether the schematic writes metres (1u) or microns (1).
                let um = |x: f64| if x < 1e-3 { x * 1e6 } else { x };
                return Ok(a / (um(dim("w")?) * um(dim("l")?)).sqrt());
            }
            match s.strip_suffix('%') {
                Some(pct) => parse_spice(pct).map(|p| nominal.abs() * p / 100.0),
                None => parse_spice(s),
            }
            .ok_or_else(|| format!("{t}: invalid sigma '{s}'"))
        };
        plan.inputs.push(t.to_string());
        plan.dists.push(v.dist);
        let shared = plan.inputs.len() - 1;
        match t.split_once('.') {
            Some((pat, param)) => {
                let param = param.to_ascii_lowercase();
                let mut hit = false;
                for inst in sch.instances.iter().filter(|i| glob(pat, &i.name)) {
                    let raw = inst.param(&param);
                    // The MOS shift parameters are rarely written: they start at no shift.
                    let unset = match param.as_str() {
                        "delvto" if raw.trim().is_empty() => Some(0.0),
                        "mulu0" if raw.trim().is_empty() => Some(1.0),
                        _ => None,
                    };
                    let nominal = unset
                        .or_else(|| parse_spice(raw))
                        .ok_or_else(|| format!("{t}: {}.{param} = '{raw}' is not a number", inst.name))?;
                    let name = format!("{}.{param}", inst.name);
                    let input = if v.mismatch && hit {
                        plan.inputs.push(name.clone());
                        plan.dists.push(v.dist);
                        plan.inputs.len() - 1
                    } else {
                        if v.mismatch {
                            plan.inputs[shared] = name.clone();
                        }
                        shared
                    };
                    let term = (input, sigma_of(nominal, Some(inst))?);
                    plan.add_term(name, nominal, term);
                    hit = true;
                }
                if !hit {
                    return Err(format!("{t}: no instance matches '{pat}'"));
                }
            }
            None => {
                let nominal = parse_spice(&v.nominal).ok_or_else(|| format!("{t}: invalid nominal '{}'", v.nominal))?;
                let term = (shared, sigma_of(nominal, None)?);
                plan.add_term(t.to_string(), nominal, term);
            }
        }
    }
    if mc.process {
        let v = variation.ok_or("the technology has no measured process variation (Process Data > Use for Monte Carlo)")?;
        v.validate()?;
        let l = v.factor().expect("validated");
        let n = v.params.len();
        // One standard normal input per parameter; parameter i is its nominal plus row i of
        // the Cholesky factor times the inputs, so the draws have the measured covariance.
        let first = plan.inputs.len();
        for p in &v.params {
            plan.inputs.push(format!("{p} (process)"));
            plan.dists.push(Dist::Gauss);
        }
        for i in 0..n {
            let terms = (0..=i).filter(|&j| l[i * n + j] != 0.0).map(|j| (first + j, l[i * n + j])).collect();
            plan.targets.push(McTarget { name: format!("model:{}", v.params[i]), nominal: v.mean[i], terms });
        }
    }
    let mut rng = Rng(mc.seed ^ 0x9e37_79b9_7f4a_7c15);
    plan.draws = (0..mc.runs).map(|_| plan.dists.iter().map(|d| rng.draw(*d)).collect()).collect();
    Ok(plan)
}

/// The sampled assignments of every Monte Carlo run (the same for every sweep point, so corners
/// compare on equal samples).
pub fn mc_samples(
    mc: &MonteCarlo,
    sch: &Schematic,
    variation: Option<&ProcessVariation>,
) -> Result<Vec<Vec<(String, String)>>, String> {
    let plan = mc_plan(mc, sch, variation)?;
    Ok(plan.draws.iter().map(|x| plan.assign(x)).collect())
}

/// `*` and `?` wildcard match, case-insensitive.
fn glob(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.to_ascii_lowercase().chars().collect();
    let s: Vec<char> = s.to_ascii_lowercase().chars().collect();
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < s.len() {
        if i < p.len() && (p[i] == '?' || p[i] == s[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(st) = star {
            i = st + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

/// SplitMix64 with Box-Muller normals; deterministic for a seed.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1).
    pub(crate) fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }

    /// A standardised draw: N(0, 1) or U(-1, 1).
    pub(crate) fn draw(&mut self, d: Dist) -> f64 {
        match d {
            Dist::Gauss => self.gauss(),
            Dist::Uniform => self.uniform() * 2.0 - 1.0,
        }
    }
}

/// Summary statistics of spec values.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub std: f64,
    pub min: f64,
    pub max: f64,
}

/// Statistics of the finite values (sample standard deviation).
pub fn stats(values: &[f64]) -> Stats {
    let v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return Stats::default();
    }
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let var = if n > 1 { v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64 } else { 0.0 };
    let min = v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Stats { n, mean, std: var.sqrt(), min, max }
}

/// The deck of one sweep point: instance parameters set, corner and temperature applied and
/// design variables substituted. Returns the deck and the netlister's warnings.
pub fn point_deck(
    sch: &Schematic,
    tech: &Tech,
    title: &str,
    base: &SimSetup,
    pt: &SweepPoint,
) -> Result<(String, Vec<String>), String> {
    let mut sch = sch.clone();
    let mut design = vec![];
    let mut models = vec![];
    for (name, value) in &pt.vars {
        if let Some(mp) = name.strip_prefix("model:") {
            let (m, p) = mp.split_once('.').ok_or_else(|| format!("{name}: write model:NAME.PARAM"))?;
            models.push((m, p, value));
            continue;
        }
        match name.split_once('.') {
            Some((inst, param)) => {
                let i = sch
                    .instances
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(inst))
                    .ok_or_else(|| format!("{name}: no instance '{inst}' in the schematic"))?;
                i.params.insert(param.to_ascii_lowercase(), value.clone());
            }
            None => design.push((name, value)),
        }
    }
    let mut setup = base.clone();
    setup.corner = pt.corner.clone();
    setup.temp = pt.temp;
    let nl = netlist_for(&sch, tech, &setup.corner);
    let mut deck = nl.deck(title, &setup);
    for (name, value) in design {
        deck = deck.replace(&format!("{{{name}}}"), value);
    }
    for (m, p, value) in models {
        let v = parse_spice(value).ok_or_else(|| format!("model:{m}.{p}: invalid value '{value}'"))?;
        deck = crate::silicon::set_model_param(&deck, m, p, v)
            .map_err(|e| format!("model:{m}.{p}: {e} (the measured variation needs the models inline, not a .lib)"))?;
    }
    Ok((deck, nl.warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Orient, Pt};
    use crate::schematic::DeviceKind;

    #[test]
    fn values_and_ranges() {
        assert_eq!(parse_values("1k, 2k 5k").unwrap(), ["1k", "2k", "5k"]);
        assert_eq!(parse_values("0:0.5:2").unwrap(), ["0", "0.5", "1", "1.5", "2"]);
        assert_eq!(parse_values("-40:82.5:125").unwrap(), ["-40", "42.5", "125"]);
        assert!(parse_values("1:0:2").is_err());
        assert!(parse_values("2:1:1").is_err());
        assert!(parse_values("x").is_err());
    }

    #[test]
    fn points_cover_every_combination() {
        let base = SimSetup::default();
        let s = SweepSetup {
            corners: vec!["tt".into(), "ff".into()],
            temps: "-40 125".into(),
            vars: vec![SweepVar { name: "R1.r".into(), values: "1k 2k 3k".into() }],
            specs: vec![],
            ..Default::default()
        };
        let pts = s.points(&base).unwrap();
        assert_eq!(pts.len(), 12);
        assert_eq!(pts[0].label(), "tt -40°C R1.r=1k");
        assert_eq!(pts[11].label(), "ff 125°C R1.r=3k");
        let only_base = SweepSetup::default().points(&base).unwrap();
        assert_eq!(only_base, [SweepPoint { corner: String::new(), temp: 27.0, vars: vec![], run: None, swept: 0 }]);
    }

    #[test]
    fn two_axes() {
        let s = SweepSetup {
            vars: vec![
                SweepVar { name: "a".into(), values: "1 2".into() },
                SweepVar { name: "b".into(), values: "3 4 5".into() },
            ],
            ..Default::default()
        };
        let pts = s.points(&SimSetup::default()).unwrap();
        let got: Vec<String> = pts.iter().map(|p| p.vars.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(",")).collect();
        assert_eq!(got, ["1,3", "1,4", "1,5", "2,3", "2,4", "2,5"]);
    }

    #[test]
    fn deck_applies_point() {
        let mut sch = Schematic::default();
        let v = sch.add_instance(DeviceKind::VSource, Pt::new(0, 0), Orient::default());
        sch.instance_mut(v).unwrap().params.insert("dc".into(), "{vdd}".into());
        let r = sch.add_instance(DeviceKind::Resistor, Pt::new(4, 0), Orient::default());
        let rname = sch.instance(r).unwrap().name.clone();
        let pt = SweepPoint {
            corner: String::new(),
            temp: 85.0,
            vars: vec![("vdd".into(), "1.2".into()), (format!("{rname}.r"), "4.7k".into())],
            run: None,
            swept: 2,
        };
        let (deck, _) = point_deck(&sch, &Tech::generic(), "t", &SimSetup::default(), &pt).unwrap();
        assert!(deck.contains("DC 1.2"), "{deck}");
        assert!(deck.contains(" 4.7k"), "{deck}");
        assert!(deck.contains(".temp 85"), "{deck}");
        let bad = SweepPoint { vars: vec![("X9.r".into(), "1".into())], ..pt };
        assert!(point_deck(&sch, &Tech::generic(), "t", &SimSetup::default(), &bad).is_err());
    }

    fn two_resistors() -> Schematic {
        let mut sch = Schematic::default();
        for (k, r) in ["1k", "2k"].iter().enumerate() {
            let id = sch.add_instance(DeviceKind::Resistor, Pt::new(4 * k as i32, 0), Orient::default());
            let i = sch.instance_mut(id).unwrap();
            i.name = format!("R{}", k + 1);
            i.params.insert("r".into(), r.to_string());
        }
        sch
    }

    #[test]
    fn monte_carlo_samples() {
        let sch = two_resistors();
        let mc = MonteCarlo {
            runs: 4000,
            seed: 7,
            vars: vec![McVar { target: "R*.r".into(), sigma: "10%".into(), ..Default::default() }],
            ..Default::default()
        };
        let s = mc_samples(&mc, &sch, None).unwrap();
        assert_eq!(s.len(), 4000);
        let val = |run: &Vec<(String, String)>, k: usize| parse_spice(&run[k].1).unwrap();
        let r1: Vec<f64> = s.iter().map(|run| val(run, 0)).collect();
        let st = stats(&r1);
        assert!((st.mean - 1000.0).abs() < 10.0 && (st.std - 100.0).abs() < 6.0, "{st:?}");
        assert!(s.iter().all(|run| (val(run, 1) / val(run, 0) - 2.0).abs() < 1e-5), "process variation is shared");
        let mis = MonteCarlo { vars: vec![McVar { mismatch: true, ..mc.vars[0].clone() }], ..mc.clone() };
        let s = mc_samples(&mis, &sch, None).unwrap();
        assert!(s.iter().any(|run| (val(run, 1) / val(run, 0) - 2.0).abs() > 0.01), "mismatch draws independently");
        assert_eq!(mc_samples(&mc, &sch, None).unwrap(), mc_samples(&mc, &sch, None).unwrap(), "deterministic per seed");
        let uni = MonteCarlo {
            runs: 500,
            vars: vec![McVar {
                target: "vb".into(),
                nominal: "1".into(),
                sigma: "0.1".into(),
                dist: Dist::Uniform,
                mismatch: false,
            }],
            ..mc
        };
        assert!(mc_samples(&uni, &sch, None).unwrap().iter().all(|run| (val(run, 0) - 1.0).abs() <= 0.1));
        let bad = MonteCarlo {
            runs: 1,
            vars: vec![McVar { target: "Q*.r".into(), sigma: "1".into(), ..Default::default() }],
            ..Default::default()
        };
        assert!(mc_samples(&bad, &sch, None).is_err());
    }

    #[test]
    fn mos_mismatch_through_delvto() {
        // Two NMOS of 1 × 1 µm and 4 × 1 µm with no delvto written: Pelgrom sigma 5 mV·µm.
        let mut sch = Schematic::default();
        for (k, w) in ["1u", "4u"].iter().enumerate() {
            let id = sch.add_instance(DeviceKind::Nmos, Pt::new(6 * k as i32, 0), Orient::default());
            let i = sch.instance_mut(id).unwrap();
            i.name = format!("M{}", k + 1);
            i.params.insert("w".into(), w.to_string());
            i.params.insert("l".into(), "1u".into());
        }
        let mc = MonteCarlo {
            runs: 4000,
            seed: 11,
            vars: vec![McVar { target: "M*.delvto".into(), sigma: "5m / sqrt(WL)".into(), mismatch: true, ..Default::default() }],
            ..Default::default()
        };
        let s = mc_samples(&mc, &sch, None).unwrap();
        let col = |name: &str| -> Vec<f64> {
            s.iter().map(|run| parse_spice(&run.iter().find(|(n, _)| n == name).unwrap().1).unwrap()).collect()
        };
        let (a, b) = (stats(&col("M1.delvto")), stats(&col("M2.delvto")));
        assert!(a.mean.abs() < 3e-4 && (a.std / 5e-3 - 1.0).abs() < 0.06, "{a:?}");
        assert!((b.std / 2.5e-3 - 1.0).abs() < 0.06, "4x the area halves sigma: {b:?}");
        // A shared process shift on the same parameter adds to the mismatch (variances add).
        let both = MonteCarlo {
            vars: vec![mc.vars[0].clone(), McVar { target: "M*.delvto".into(), sigma: "10m".into(), ..Default::default() }],
            ..mc.clone()
        };
        let s2 = mc_samples(&both, &sch, None).unwrap();
        let m1: Vec<f64> =
            s2.iter().map(|run| parse_spice(&run.iter().find(|(n, _)| n == "M1.delvto").unwrap().1).unwrap()).collect();
        let st = stats(&m1);
        assert!((st.std / (5e-3f64.hypot(10e-3)) - 1.0).abs() < 0.06, "{st:?}");
        // The netlist carries the shift to the simulator.
        let pt = SweepPoint { corner: String::new(), temp: 27.0, vars: s[0].clone(), run: Some(1), swept: 0 };
        let (deck, _) = point_deck(&sch, &Tech::generic(), "t", &SimSetup::default(), &pt).unwrap();
        assert!(deck.lines().any(|l| l.starts_with("M1 ") && l.contains(" delvto=")), "{deck}");
        let no_wl = MonteCarlo {
            vars: vec![McVar { target: "M*.delvto".into(), sigma: "5m/sqrt(wl)".into(), ..Default::default() }],
            ..mc
        };
        let mut bare = sch.clone();
        bare.instances[0].params.remove("w");
        assert!(mc_samples(&no_wl, &bare, None).unwrap_err().contains("M1.w"));
    }

    #[test]
    fn expand_repeats_points_per_run() {
        let sch = two_resistors();
        let s = SweepSetup {
            temps: "0 50".into(),
            monte_carlo: MonteCarlo {
                runs: 3,
                seed: 1,
                vars: vec![McVar { target: "R1.r".into(), sigma: "1%".into(), ..Default::default() }],
                ..Default::default()
            },
            ..Default::default()
        };
        let pts = s.expand(&SimSetup::default(), &sch, None).unwrap();
        assert_eq!(pts.len(), 6);
        assert_eq!(pts[0].vars[0].1, pts[3].vars[0].1, "same samples at every point");
        assert_eq!(pts[2].label(), "0°C mc#3");
        assert!(glob("M*", "m12") && glob("R?", "R1") && !glob("R?", "R12"));
    }

    #[test]
    fn spec_limits() {
        let s = Spec { min: "1m".into(), max: "".into(), ..Default::default() };
        assert_eq!(s.check(2e-3), Some(true));
        assert_eq!(s.check(0.5e-3), Some(false));
        assert_eq!(s.check(f64::NAN), Some(false));
        assert_eq!(Spec::default().check(1.0), None);
    }
}
