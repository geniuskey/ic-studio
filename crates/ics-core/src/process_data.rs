//! Process data: wafer acceptance test (WAT / PCM / e-test) results against the process
//! specification, with statistical process control and a check of the model corners.
//!
//! - **Spec** (TOML): every parameter with its unit, target, spec limits and, optionally, the
//!   value of each model corner (`corners = { ff = 0.40, tt = 0.45, ss = 0.50 }`).
//! - **Data** (CSV): one row per measured site in time order; `lot`, `wafer` and `site`
//!   columns identify it, every other column is a parameter.
//!
//! For each parameter the summary gives mean, σ, Cp / Cpk against the spec limits and the
//! out-of-spec count; the control chart of the wafer means (individuals chart, limits from the
//! moving range of a baseline) is checked with the Western Electric rules; and the corners are
//! set against the measured spread: a corner spread narrower than ±3σ of silicon means the
//! corner models do not cover the process.

use serde::{Deserialize, Serialize};

use crate::silicon::Measured;

/// The process specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProcessSpec {
    #[serde(default)]
    pub name: String,
    /// Wafers at the start of the data that set the control limits (0 = all).
    #[serde(default)]
    pub baseline_wafers: usize,
    #[serde(rename = "param", default)]
    pub params: Vec<ParamSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub name: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target: Option<f64>,
    /// Lower and upper spec limits.
    #[serde(default)]
    pub lsl: Option<f64>,
    #[serde(default)]
    pub usl: Option<f64>,
    /// The parameter's value in each model corner.
    #[serde(default)]
    pub corners: std::collections::BTreeMap<String, f64>,
}

impl ProcessSpec {
    pub fn from_toml(text: &str) -> Result<ProcessSpec, String> {
        let s: ProcessSpec = toml::from_str(text).map_err(|e| e.to_string())?;
        for p in &s.params {
            if let (Some(l), Some(u)) = (p.lsl, p.usl)
                && l >= u
            {
                return Err(format!("{}: lsl must be below usl", p.name));
            }
        }
        Ok(s)
    }

    pub fn param(&self, name: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// One measured site.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub lot: String,
    pub wafer: String,
    pub site: String,
}

/// WAT data: the sites and the parameter columns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WatData {
    pub sites: Vec<Site>,
    /// Parameter names (columns other than lot / wafer / site).
    pub params: Vec<String>,
    /// `values[p][s]`: parameter `p` at site `s` (NaN where missing).
    pub values: Vec<Vec<f64>>,
}

impl WatData {
    /// Parse CSV with `lot`, `wafer` and `site` columns (any may be missing) and parameter
    /// columns. Text in the lot / wafer / site columns is kept as is.
    pub fn parse(text: &str) -> Result<WatData, String> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        let head = lines.next().ok_or("the data file is empty")?;
        let sep = [',', ';', '\t'].into_iter().find(|c| head.contains(*c)).unwrap_or(',');
        let split = |l: &str| l.split(sep).map(|c| c.trim().trim_matches('"').to_string()).collect::<Vec<_>>();
        let cols = split(head);
        let find = |n: &str| cols.iter().position(|c| c.eq_ignore_ascii_case(n));
        let (lc, wc, sc) = (find("lot"), find("wafer"), find("site"));
        let pcols: Vec<usize> = (0..cols.len()).filter(|i| Some(*i) != lc && Some(*i) != wc && Some(*i) != sc).collect();
        let mut d = WatData {
            params: pcols.iter().map(|&i| cols[i].clone()).collect(),
            values: vec![vec![]; pcols.len()],
            ..Default::default()
        };
        // Reuse the numeric parser of measured curves for the parameter cells.
        let num = |v: &str, row: usize| -> Result<f64, String> {
            if v.is_empty() {
                return Ok(f64::NAN);
            }
            crate::units::parse_spice(v).ok_or_else(|| format!("data row {row}: '{v}' is not a number"))
        };
        for (k, l) in lines.enumerate() {
            let c = split(l);
            let get = |i: Option<usize>| i.and_then(|i| c.get(i)).cloned().unwrap_or_default();
            d.sites.push(Site { lot: get(lc), wafer: get(wc), site: get(sc) });
            for (j, &i) in pcols.iter().enumerate() {
                d.values[j].push(num(c.get(i).map_or("", String::as_str), k + 1)?);
            }
        }
        if d.sites.is_empty() {
            return Err("the data file has no rows".into());
        }
        Ok(d)
    }

    /// The same data as named columns (for plotting with [`Measured`]).
    pub fn as_measured(&self) -> Measured {
        Measured {
            columns: self.params.clone(),
            rows: (0..self.sites.len()).map(|s| self.values.iter().map(|v| v[s]).collect()).collect(),
        }
    }

    /// Wafers in order of first appearance, as (lot, wafer, site indices).
    pub fn wafers(&self) -> Vec<(String, String, Vec<usize>)> {
        let mut out: Vec<(String, String, Vec<usize>)> = vec![];
        for (i, s) in self.sites.iter().enumerate() {
            match out.iter_mut().find(|w| w.0 == s.lot && w.1 == s.wafer) {
                Some(w) => w.2.push(i),
                None => out.push((s.lot.clone(), s.wafer.clone(), vec![i])),
            }
        }
        out
    }
}

/// A Western Electric rule violation on the control chart of wafer means.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    /// Rule 1 to 4.
    pub rule: u8,
    /// Index of the wafer (in [`WatData::wafers`] order) where the rule fires.
    pub wafer: usize,
    pub message: String,
}

/// Summary of one parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSummary {
    pub name: String,
    pub n: usize,
    pub mean: f64,
    pub std: f64,
    pub min: f64,
    pub max: f64,
    pub cp: Option<f64>,
    pub cpk: Option<f64>,
    /// Sites outside the spec limits.
    pub out_of_spec: usize,
    /// Wafer means in order, and the control chart's centre and limits.
    pub wafer_means: Vec<f64>,
    pub center: f64,
    pub ucl: f64,
    pub lcl: f64,
    pub violations: Vec<Violation>,
    /// Corners as (name, value, z-score against the data).
    pub corners: Vec<(String, f64, f64)>,
    /// Whether the corners span the data's ±3σ (`None` without at least two corners).
    pub corners_cover: Option<bool>,
}

fn mean_std(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    if v.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let m = v.iter().sum::<f64>() / n;
    let s = if v.len() > 1 { (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt() } else { 0.0 };
    (m, s)
}

/// Western Electric rules on `x` with centre `c` and sigma `s`.
fn western_electric(x: &[f64], c: f64, s: f64) -> Vec<Violation> {
    let mut out = vec![];
    if s.is_nan() || s <= 0.0 {
        return out;
    }
    let z: Vec<f64> = x.iter().map(|v| (v - c) / s).collect();
    for i in 0..z.len() {
        if z[i].abs() > 3.0 {
            out.push(Violation { rule: 1, wafer: i, message: format!("beyond the 3σ control limits ({:+.1}σ)", z[i]) });
        }
        // Rule 2: two of three beyond 2σ on the same side.
        if i >= 2 {
            for sign in [1.0, -1.0] {
                if (i - 2..=i).filter(|&k| z[k] * sign > 2.0).count() >= 2 && z[i] * sign > 2.0 {
                    out.push(Violation { rule: 2, wafer: i, message: "two of three beyond 2σ".into() });
                }
            }
        }
        // Rule 3: four of five beyond 1σ on the same side.
        if i >= 4 {
            for sign in [1.0, -1.0] {
                if (i - 4..=i).filter(|&k| z[k] * sign > 1.0).count() >= 4 && z[i] * sign > 1.0 {
                    out.push(Violation { rule: 3, wafer: i, message: "four of five beyond 1σ".into() });
                }
            }
        }
        // Rule 4: eight in a row on the same side of the centre (a shift).
        if i >= 7 && ((i - 7..=i).all(|k| z[k] > 0.0) || (i - 7..=i).all(|k| z[k] < 0.0)) {
            out.push(Violation { rule: 4, wafer: i, message: "eight in a row on one side of the centre".into() });
        }
    }
    // Keep the first wafer of each run of a rule.
    out.dedup_by(|b, a| a.rule == b.rule && b.wafer <= a.wafer + 1);
    out
}

/// Summarise every parameter of `data` against `spec`.
pub fn summarize(data: &WatData, spec: &ProcessSpec) -> Vec<ParamSummary> {
    let wafers = data.wafers();
    data.params
        .iter()
        .enumerate()
        .map(|(j, name)| {
            let vals: Vec<f64> = data.values[j].iter().copied().filter(|v| v.is_finite()).collect();
            let (mean, std) = mean_std(&vals);
            let ps = spec.param(name);
            let (lsl, usl) = (ps.and_then(|p| p.lsl), ps.and_then(|p| p.usl));
            let cp = match (lsl, usl) {
                (Some(l), Some(u)) if std > 0.0 => Some((u - l) / (6.0 * std)),
                _ => None,
            };
            let cpk = if std > 0.0 {
                [lsl.map(|l| (mean - l) / (3.0 * std)), usl.map(|u| (u - mean) / (3.0 * std))]
                    .into_iter()
                    .flatten()
                    .reduce(f64::min)
            } else {
                None
            };
            let out_of_spec = vals.iter().filter(|v| lsl.is_some_and(|l| **v < l) || usl.is_some_and(|u| **v > u)).count();
            let wafer_means: Vec<f64> = wafers
                .iter()
                .map(|w| mean_std(&w.2.iter().map(|&i| data.values[j][i]).filter(|v| v.is_finite()).collect::<Vec<_>>()).0)
                .collect();
            // Individuals chart: centre and sigma (moving range / 1.128) from the baseline.
            let base_n = if spec.baseline_wafers == 0 { wafer_means.len() } else { spec.baseline_wafers.min(wafer_means.len()) };
            let base: Vec<f64> = wafer_means[..base_n].iter().copied().filter(|v| v.is_finite()).collect();
            let center = mean_std(&base).0;
            let mr: Vec<f64> = base.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            let sigma = if mr.is_empty() { 0.0 } else { mr.iter().sum::<f64>() / mr.len() as f64 / 1.128 };
            let violations = western_electric(&wafer_means, center, sigma);
            let corners: Vec<(String, f64, f64)> = ps
                .map(|p| p.corners.iter().map(|(n, v)| (n.clone(), *v, if std > 0.0 { (v - mean) / std } else { 0.0 })).collect())
                .unwrap_or_default();
            let corners_cover = (corners.len() >= 2 && std > 0.0).then(|| {
                let lo = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
                let hi = corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max);
                lo <= mean - 3.0 * std && hi >= mean + 3.0 * std
            });
            ParamSummary {
                name: name.clone(),
                n: vals.len(),
                mean,
                std,
                min: vals.iter().copied().fold(f64::INFINITY, f64::min),
                max: vals.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                cp,
                cpk,
                out_of_spec,
                wafer_means,
                center,
                ucl: center + 3.0 * sigma,
                lcl: center - 3.0 * sigma,
                violations,
                corners,
                corners_cover,
            }
        })
        .collect()
}

/// Pearson correlation of two parameters over the sites where both are measured.
pub fn correlation(data: &WatData, a: usize, b: usize) -> f64 {
    let pairs: Vec<(f64, f64)> = data.values[a]
        .iter()
        .zip(&data.values[b])
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|(x, y)| (*x, *y))
        .collect();
    let n = pairs.len() as f64;
    if pairs.len() < 3 {
        return f64::NAN;
    }
    let (mx, my) = (pairs.iter().map(|p| p.0).sum::<f64>() / n, pairs.iter().map(|p| p.1).sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in &pairs {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx).powi(2);
        syy += (y - my).powi(2);
    }
    sxy / (sxx * syy).sqrt()
}

/// The WAT specification of the generic 180 nm technology.
pub const GENERIC180_SPEC: &str = include_str!("../../../process/generic180_wat.toml");

#[cfg(test)]
mod tests {
    use super::*;

    fn data(rows: &[(&str, &str, f64)]) -> WatData {
        let mut t = String::from("lot,wafer,site,vtn\n");
        for (l, w, v) in rows {
            t += &format!("{l},{w},1,{v}\n{l},{w},2,{}\n", v + 0.002);
        }
        WatData::parse(&t).unwrap()
    }

    #[test]
    fn parse_and_group() {
        let d = WatData::parse("# x\nlot;wafer;site;vtn;rs_poly\nA;1;1;0.45;7.1\nA;1;2;0.46;\nA;2;1;0.44;7.3\n").unwrap();
        assert_eq!(d.params, ["vtn", "rs_poly"]);
        assert!(d.values[1][1].is_nan());
        let w = d.wafers();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].2, [0, 1]);
        assert!(WatData::parse("lot,vtn\nA,x\n").is_err());
        assert_eq!(d.as_measured().columns, ["vtn", "rs_poly"]);
    }

    #[test]
    fn capability_spc_and_corners() {
        // 20 stable wafers, then a shift of +2σ for 8 wafers.
        let mut rows = vec![];
        let noise = [0.3, -0.5, 0.8, -0.2, 0.1, -0.9, 0.6, 0.0, -0.4, 0.7, -0.6, 0.2, 0.5, -0.1, -0.7, 0.9, -0.3, 0.4, -0.8, 0.1];
        let names: Vec<String> = (0..28).map(|k| format!("w{k}")).collect();
        for (k, n) in noise.iter().enumerate() {
            rows.push(("L1", names[k].as_str(), 0.45 + 0.01 * n));
        }
        for (k, n) in names.iter().enumerate().skip(20) {
            rows.push(("L2", n.as_str(), 0.45 + 0.012 + 0.002 * ((k % 3) as f64)));
        }
        let d = data(&rows);
        let spec = ProcessSpec::from_toml(
            "baseline_wafers = 20\n[[param]]\nname = \"vtn\"\nunit = \"V\"\nlsl = 0.40\nusl = 0.50\ncorners = { ff = 0.42, tt = 0.45, ss = 0.48 }\n",
        )
        .unwrap();
        let s = &summarize(&d, &spec)[0];
        assert_eq!(s.n, 56);
        assert!(s.cp.unwrap() > 1.0 && s.cpk.unwrap() < s.cp.unwrap(), "{s:?}");
        assert_eq!(s.out_of_spec, 0);
        assert_eq!(s.wafer_means.len(), 28);
        assert!(s.violations.iter().any(|v| v.rule == 4 && v.wafer >= 20), "the shift is caught: {:?}", s.violations);
        assert!(s.violations.iter().all(|v| v.wafer >= 20), "the baseline is in control: {:?}", s.violations);
        // Corners at ±0.03 cover ±3σ only when σ < 0.01.
        assert_eq!(s.corners.len(), 3);
        assert_eq!(s.corners_cover, Some(s.std * 3.0 <= 0.03 - (s.mean - 0.45).abs()));
        assert!(ProcessSpec::from_toml("[[param]]\nname = \"x\"\nlsl = 2\nusl = 1\n").is_err());
    }

    #[test]
    fn rule_one_and_correlation() {
        let mut rows: Vec<(&str, &str, f64)> = vec![];
        let names: Vec<String> = (0..12).map(|k| format!("w{k}")).collect();
        for (k, n) in names.iter().enumerate() {
            rows.push(("L", n.as_str(), if k == 9 { 0.60 } else { 0.45 + 0.002 * ((k % 4) as f64) }));
        }
        let d = data(&rows);
        let s = &summarize(&d, &ProcessSpec::default())[0];
        assert!(s.violations.iter().any(|v| v.rule == 1 && v.wafer == 9), "{:?}", s.violations);
        assert_eq!((s.cp, s.cpk), (None, None));
        let d2 = WatData::parse("vtn,idsat\n0.40,1.2\n0.45,1.0\n0.50,0.8\n0.47,0.95\n").unwrap();
        assert!(correlation(&d2, 0, 1) < -0.95);
    }

    #[test]
    fn generic_spec_parses() {
        let s = ProcessSpec::from_toml(GENERIC180_SPEC).unwrap();
        assert!(s.param("vtn").is_some_and(|p| p.corners.len() == 3));
    }
}
