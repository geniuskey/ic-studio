//! Device mismatch from silicon: Pelgrom coefficients fitted to matched-pair measurements.
//!
//! The data (CSV) has one row per measured pair: `device` (`nmos`, `pmos` or a model name; a
//! name starting with `p` is p-channel), `w` and `l` (µm), `dvt` (V, threshold difference of
//! the two transistors) and optionally `dbeta` (relative current factor difference, 0.01 =
//! 1 %). For every device and size the standard deviation of the differences is fitted with
//! Pelgrom's law
//!
//! `σ²(ΔVt) = Avt² / (W L) + σ0²`
//!
//! (and likewise for Δβ/β) by least squares weighted with the uncertainty of each sample
//! variance; `σ0` is a size-independent floor, dropped when the data does not support one.
//! Avt is the pair coefficient: the σ of one device about its nominal is Avt / √(2 W L).

use serde::{Deserialize, Serialize};

/// Matched-pair measurements.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PairData {
    pub rows: Vec<PairRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairRow {
    pub device: String,
    /// µm
    pub w: f64,
    pub l: f64,
    /// V
    pub dvt: f64,
    /// Relative (NaN where not measured).
    pub dbeta: f64,
}

impl PairData {
    pub fn parse(text: &str) -> Result<PairData, String> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        let head = lines.next().ok_or("the pair data is empty")?;
        let sep = [',', ';', '\t'].into_iter().find(|c| head.contains(*c)).unwrap_or(',');
        let cols: Vec<String> = head.split(sep).map(|c| c.trim().trim_matches('"').to_ascii_lowercase()).collect();
        let col = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
        let device = col(&["device", "type", "model"]).ok_or("no 'device' column")?;
        let (w, l) = (col(&["w", "w_um"]).ok_or("no 'w' column")?, col(&["l", "l_um"]).ok_or("no 'l' column")?);
        let dvt = col(&["dvt", "dvth", "delta_vt"]).ok_or("no 'dvt' column")?;
        let dbeta = col(&["dbeta", "dbeta_rel", "delta_beta"]);
        let mut rows = vec![];
        for (k, line) in lines.enumerate() {
            let f: Vec<&str> = line.split(sep).map(|c| c.trim().trim_matches('"')).collect();
            let num = |i: usize, what: &str| -> Result<f64, String> {
                let s = f.get(i).copied().unwrap_or("");
                crate::units::parse_spice(s).ok_or_else(|| format!("row {}: {what} '{s}' is not a number", k + 2))
            };
            let r = PairRow {
                device: f.get(device).copied().unwrap_or("").to_string(),
                w: num(w, "w")?,
                l: num(l, "l")?,
                dvt: num(dvt, "dvt")?,
                dbeta: match dbeta {
                    Some(i) if !f.get(i).is_none_or(|s| s.is_empty()) => num(i, "dbeta")?,
                    _ => f64::NAN,
                },
            };
            if r.device.is_empty() || !(r.w > 0.0 && r.l > 0.0) {
                return Err(format!("row {}: needs a device and positive w, l", k + 2));
            }
            rows.push(r);
        }
        if rows.is_empty() {
            return Err("the pair data has no rows".into());
        }
        Ok(PairData { rows })
    }

    /// The device names in order of appearance.
    pub fn devices(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for r in &self.rows {
            if !out.iter().any(|d| d.eq_ignore_ascii_case(&r.device)) {
                out.push(r.device.clone());
            }
        }
        out
    }
}

/// One size of one device.
#[derive(Debug, Clone, PartialEq)]
pub struct SizeStats {
    pub w: f64,
    pub l: f64,
    pub n: usize,
    /// Mean difference (systematic offset).
    pub mean: f64,
    pub sigma: f64,
    /// σ of the fitted law at this size.
    pub fitted: f64,
}

/// A Pelgrom fit of one quantity.
#[derive(Debug, Clone, PartialEq)]
pub struct Pelgrom {
    /// Coefficient in the data's unit · µm (V·µm for Vt, 1·µm for β).
    pub a: f64,
    /// Its standard error.
    pub a_err: f64,
    /// Size-independent floor σ0 (0 when not supported by the data).
    pub floor: f64,
    pub sizes: Vec<SizeStats>,
}

impl Pelgrom {
    /// σ at a size (µm).
    pub fn sigma(&self, w: f64, l: f64) -> f64 {
        (self.a * self.a / (w * l) + self.floor * self.floor).sqrt()
    }
}

/// The fit of one device.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceFit {
    pub device: String,
    pub pmos: bool,
    pub vt: Pelgrom,
    pub beta: Option<Pelgrom>,
}

/// Fit Pelgrom's law to the pairs of every device.
pub fn fit(data: &PairData) -> Result<Vec<DeviceFit>, String> {
    let mut out = vec![];
    for dev in data.devices() {
        let rows: Vec<&PairRow> = data.rows.iter().filter(|r| r.device.eq_ignore_ascii_case(&dev)).collect();
        let vt = fit_quantity(&rows, |r| r.dvt).map_err(|e| format!("{dev} dvt: {e}"))?;
        let beta = if rows.iter().filter(|r| r.dbeta.is_finite()).count() >= 6 {
            Some(fit_quantity(&rows, |r| r.dbeta).map_err(|e| format!("{dev} dbeta: {e}"))?)
        } else {
            None
        };
        let lower = dev.to_ascii_lowercase();
        out.push(DeviceFit { pmos: lower.starts_with('p'), device: dev, vt, beta });
    }
    Ok(out)
}

fn fit_quantity(rows: &[&PairRow], get: impl Fn(&PairRow) -> f64) -> Result<Pelgrom, String> {
    // Group by size.
    let mut sizes: Vec<(f64, f64, Vec<f64>)> = vec![];
    for r in rows {
        let v = get(r);
        if !v.is_finite() {
            continue;
        }
        match sizes.iter_mut().find(|s| (s.0 - r.w).abs() < 1e-9 && (s.1 - r.l).abs() < 1e-9) {
            Some(s) => s.2.push(v),
            None => sizes.push((r.w, r.l, vec![v])),
        }
    }
    sizes.retain(|s| s.2.len() >= 3);
    if sizes.len() < 2 {
        return Err("needs at least 2 sizes with 3 or more pairs each".into());
    }
    let mut stats: Vec<SizeStats> = sizes
        .iter()
        .map(|(w, l, v)| {
            let n = v.len();
            let mean = v.iter().sum::<f64>() / n as f64;
            let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
            SizeStats { w: *w, l: *l, n, mean, sigma: var.sqrt(), fitted: 0.0 }
        })
        .collect();
    // σ² = a2 · x + f2 with x = 1 / (W L); weights 1 / var(s²) = (n - 1) / (2 σ⁴), with σ from
    // the through-origin fit to keep the weights independent of the noise in each s².
    let x: Vec<f64> = stats.iter().map(|s| 1.0 / (s.w * s.l)).collect();
    let y: Vec<f64> = stats.iter().map(|s| s.sigma * s.sigma).collect();
    let a2_0 = x.iter().zip(&y).map(|(x, y)| x * y).sum::<f64>() / x.iter().map(|x| x * x).sum::<f64>();
    let w: Vec<f64> = stats.iter().zip(&x).map(|(s, x)| (s.n as f64 - 1.0) / (2.0 * (a2_0 * x).max(1e-300).powi(2))).collect();
    let (sw, swx, swxx, swy, swxy) = (0..x.len()).fold((0.0, 0.0, 0.0, 0.0, 0.0), |a, i| {
        (a.0 + w[i], a.1 + w[i] * x[i], a.2 + w[i] * x[i] * x[i], a.3 + w[i] * y[i], a.4 + w[i] * x[i] * y[i])
    });
    let det = sw * swxx - swx * swx;
    let (mut a2, mut f2, mut var_a2) = if det > 0.0 {
        ((sw * swxy - swx * swy) / det, (swxx * swy - swx * swxy) / det, sw / det)
    } else {
        (f64::NAN, f64::NAN, f64::NAN)
    };
    if !(f2.is_finite() && a2.is_finite() && f2 >= 0.0 && a2 > 0.0) {
        // No floor: weighted least squares through the origin.
        a2 = swxy / swxx;
        f2 = 0.0;
        var_a2 = 1.0 / swxx;
    }
    if a2.is_nan() || a2 <= 0.0 {
        return Err("the spread does not fall with the area".into());
    }
    let a = a2.sqrt();
    let fit = Pelgrom { a, a_err: var_a2.sqrt() / (2.0 * a), floor: f2.sqrt(), sizes: vec![] };
    for s in &mut stats {
        s.fitted = fit.sigma(s.w, s.l);
    }
    Ok(Pelgrom { sizes: stats, ..fit })
}

/// Measured mismatch for Monte Carlo: Pelgrom pair coefficients per polarity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MismatchModel {
    #[serde(default)]
    pub source: String,
    pub devices: Vec<DeviceMismatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceMismatch {
    pub device: String,
    pub pmos: bool,
    /// Pelgrom pair coefficient of Vt (V·m).
    pub avt: f64,
    /// Pelgrom pair coefficient of Δβ/β (m); 0 when not measured.
    #[serde(default)]
    pub abeta: f64,
}

impl MismatchModel {
    pub fn from_fits(fits: &[DeviceFit], source: &str) -> MismatchModel {
        MismatchModel {
            source: source.to_string(),
            devices: fits
                .iter()
                .map(|f| DeviceMismatch {
                    device: f.device.clone(),
                    pmos: f.pmos,
                    avt: f.vt.a * 1e-6,
                    abeta: f.beta.as_ref().map_or(0.0, |b| b.a * 1e-6),
                })
                .collect(),
        }
    }

    /// Per-device coefficients (avt, abeta) for one polarity: the pair coefficients over √2,
    /// as Monte Carlo draws every device about its own nominal.
    pub fn per_device(&self, pmos: bool) -> Option<(f64, f64)> {
        self.devices.iter().find(|d| d.pmos == pmos).map(|d| (d.avt / 2f64.sqrt(), d.abeta / 2f64.sqrt()))
    }
}

/// Synthetic matched-pair data of the generic 180 nm technology (not measurements).
pub const GENERIC180_PAIRS: &str = include_str!("../../../process/generic180_mismatch.csv");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pelgrom_from_the_example_pairs() {
        let d = PairData::parse(GENERIC180_PAIRS).unwrap();
        let fits = fit(&d).unwrap();
        assert_eq!(fits.len(), 2);
        let n = fits.iter().find(|f| !f.pmos).unwrap();
        let p = fits.iter().find(|f| f.pmos).unwrap();
        // Generated with Avt 4.0 / 3.5 mV·µm and Aβ 1.0 / 0.9 %·µm.
        assert!((n.vt.a * 1e3 - 4.0).abs() < 3.0 * n.vt.a_err * 1e3 + 0.1, "{:?}", n.vt);
        assert!((p.vt.a * 1e3 - 3.5).abs() < 3.0 * p.vt.a_err * 1e3 + 0.1, "{:?}", p.vt);
        assert!(n.vt.a_err > 0.0 && n.vt.a_err < 0.0004);
        let nb = n.beta.as_ref().unwrap();
        assert!((nb.a * 100.0 - 1.0).abs() < 0.15, "{nb:?}");
        // The fitted law follows the measured σ at every size.
        for s in &n.vt.sizes {
            assert!((s.fitted / s.sigma - 1.0).abs() < 0.35, "{s:?}");
        }
        let m = MismatchModel::from_fits(&fits, "test");
        let (avt, abeta) = m.per_device(false).unwrap();
        assert!((avt - n.vt.a * 1e-6 / 2f64.sqrt()).abs() < 1e-15 && abeta > 0.0);
        assert!(PairData::parse("device,w,l\nnmos,1,1\n").is_err());
        assert!(fit(&PairData::parse("device,w,l,dvt\nnmos,1,1,0.001\nnmos,1,1,0.002\n").unwrap()).is_err());
    }
}
