//! Corner and parametric sweeps (ADE Explorer / Assembler style): every combination of model
//! corners, temperatures and variable values is one simulation point, and specs are checked on
//! the results of each.

use serde::{Deserialize, Serialize};

use crate::netlist::netlist_for;
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
}

impl MonteCarlo {
    pub fn is_off(&self) -> bool {
        self.runs == 0 && self.vars.is_empty()
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct McVar {
    pub target: String,
    /// Nominal value of a design variable (instance parameters use the instance's value).
    pub nominal: String,
    /// Standard deviation (Gauss) or half range (Uniform): `5%` of the nominal, or a SPICE number.
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
    pub fn expand(&self, base: &SimSetup, sch: &Schematic) -> Result<Vec<SweepPoint>, String> {
        let points = self.points(base)?;
        let mc = &self.monte_carlo;
        if mc.runs == 0 {
            return Ok(points);
        }
        let total = points.len().saturating_mul(mc.runs as usize);
        if total > MAX_POINTS {
            return Err(format!("{total} points exceed the limit of {MAX_POINTS}"));
        }
        let samples = mc_samples(mc, sch)?;
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

/// The sampled assignments of every Monte Carlo run (the same for every sweep point, so corners
/// compare on equal samples).
pub fn mc_samples(mc: &MonteCarlo, sch: &Schematic) -> Result<Vec<Vec<(String, String)>>, String> {
    struct Target {
        name: String,
        nominal: f64,
        sigma: f64,
        group: usize,
    }
    let mut targets: Vec<Target> = vec![];
    let mut groups: Vec<Dist> = vec![];
    for v in mc.vars.iter().filter(|v| !v.target.trim().is_empty()) {
        let t = v.target.trim();
        let sigma_of = |nominal: f64| -> Result<f64, String> {
            let s = v.sigma.trim();
            match s.strip_suffix('%') {
                Some(pct) => parse_spice(pct).map(|p| nominal.abs() * p / 100.0),
                None => parse_spice(s),
            }
            .ok_or_else(|| format!("{t}: invalid sigma '{s}'"))
        };
        groups.push(v.dist);
        let shared = groups.len() - 1;
        match t.split_once('.') {
            Some((pat, param)) => {
                let param = param.to_ascii_lowercase();
                let mut hit = false;
                for inst in sch.instances.iter().filter(|i| glob(pat, &i.name)) {
                    let raw = inst.param(&param);
                    let nominal =
                        parse_spice(raw).ok_or_else(|| format!("{t}: {}.{param} = '{raw}' is not a number", inst.name))?;
                    let group = if v.mismatch && hit {
                        groups.push(v.dist);
                        groups.len() - 1
                    } else {
                        shared
                    };
                    targets.push(Target { name: format!("{}.{param}", inst.name), nominal, sigma: sigma_of(nominal)?, group });
                    hit = true;
                }
                if !hit {
                    return Err(format!("{t}: no instance matches '{pat}'"));
                }
            }
            None => {
                let nominal = parse_spice(&v.nominal).ok_or_else(|| format!("{t}: invalid nominal '{}'", v.nominal))?;
                targets.push(Target { name: t.to_string(), nominal, sigma: sigma_of(nominal)?, group: shared });
            }
        }
    }
    let mut rng = Rng(mc.seed ^ 0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::with_capacity(mc.runs as usize);
    for _ in 0..mc.runs {
        let draws: Vec<f64> = groups
            .iter()
            .map(|d| match d {
                Dist::Gauss => rng.gauss(),
                Dist::Uniform => rng.uniform() * 2.0 - 1.0,
            })
            .collect();
        out.push(targets.iter().map(|t| (t.name.clone(), format!("{:.6e}", t.nominal + t.sigma * draws[t.group]))).collect());
    }
    Ok(out)
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
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1).
    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
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
    for (name, value) in &pt.vars {
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
        };
        let s = mc_samples(&mc, &sch).unwrap();
        assert_eq!(s.len(), 4000);
        let val = |run: &Vec<(String, String)>, k: usize| parse_spice(&run[k].1).unwrap();
        let r1: Vec<f64> = s.iter().map(|run| val(run, 0)).collect();
        let st = stats(&r1);
        assert!((st.mean - 1000.0).abs() < 10.0 && (st.std - 100.0).abs() < 6.0, "{st:?}");
        assert!(s.iter().all(|run| (val(run, 1) / val(run, 0) - 2.0).abs() < 1e-5), "process variation is shared");
        let mis = MonteCarlo { vars: vec![McVar { mismatch: true, ..mc.vars[0].clone() }], ..mc.clone() };
        let s = mc_samples(&mis, &sch).unwrap();
        assert!(s.iter().any(|run| (val(run, 1) / val(run, 0) - 2.0).abs() > 0.01), "mismatch draws independently");
        assert_eq!(mc_samples(&mc, &sch).unwrap(), mc_samples(&mc, &sch).unwrap(), "deterministic per seed");
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
        assert!(mc_samples(&uni, &sch).unwrap().iter().all(|run| (val(run, 0) - 1.0).abs() <= 0.1));
        let bad = MonteCarlo {
            runs: 1,
            vars: vec![McVar { target: "Q*.r".into(), sigma: "1".into(), ..Default::default() }],
            ..Default::default()
        };
        assert!(mc_samples(&bad, &sch).is_err());
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
            },
            ..Default::default()
        };
        let pts = s.expand(&SimSetup::default(), &sch).unwrap();
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
