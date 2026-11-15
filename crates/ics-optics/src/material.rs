//! Optical constants of pixel materials: complex refractive index `n + ik` against wavelength.
//!
//! Built in, by name:
//!
//! | name | source |
//! |---|---|
//! | `air` | 1 |
//! | `si` | M. A. Green 2008, crystalline silicon at 300 K (tabulated) |
//! | `sio2` | I. H. Malitson 1965, fused silica (Sellmeier) |
//! | `si3n4` | K. Luke et al. 2015 (Sellmeier) |
//! | `hfo2` | M. F. Al-Kuhaili 2004 (Cauchy) |
//! | `ta2o5` | L. Gao et al. 2012 (tabulated) |
//! | `w`, `al` | A. D. Rakic et al. 1998, Lorentz-Drude (tabulated) |
//! | `resin` | generic microlens / planarisation resin, n = 1.55 + 0.005/λ² |
//! | `cf_red`, `cf_green`, `cf_blue` | generic colour filter dyes (see [`color_filter`]) |
//!
//! Tabulated data come from the refractiveindex.info database (public domain, CC0) for
//! 0.30 - 1.20 µm. Projects can add materials with a constant index or their own table.

use std::collections::BTreeMap;

use num_complex::Complex64;
use serde::Deserialize;

use crate::nk_data;

/// A material's refractive index model. Wavelengths in µm.
#[derive(Debug, Clone)]
pub enum Material {
    Constant {
        n: f64,
        k: f64,
    },
    /// Rows of (wavelength, n, k), increasing in wavelength; linear interpolation, held at the ends.
    Table(Vec<(f64, f64, f64)>),
    /// `n² = 1 + Σ B λ² / (λ² - C²)`, pairs (B, C).
    Sellmeier(Vec<(f64, f64)>),
    /// `n = A + B/λ² + C/λ⁴`.
    Cauchy {
        a: f64,
        b: f64,
        c: f64,
    },
    /// A colour filter dye: index `n` and the transmittance of a `ref_thickness` µm film.
    Dye {
        n: f64,
        ref_thickness: f64,
        transmittance: fn(f64) -> f64,
    },
}

impl Material {
    /// Complex index `n + ik` at `lambda` µm.
    pub fn index(&self, lambda: f64) -> Complex64 {
        match self {
            Material::Constant { n, k } => Complex64::new(*n, *k),
            Material::Table(rows) => {
                let (n, k) = interpolate(rows, lambda);
                Complex64::new(n, k)
            }
            Material::Sellmeier(terms) => {
                let l2 = lambda * lambda;
                let n2: f64 = 1.0 + terms.iter().map(|(b, c)| b * l2 / (l2 - c * c)).sum::<f64>();
                Complex64::new(n2.max(1.0).sqrt(), 0.0)
            }
            Material::Cauchy { a, b, c } => Complex64::new(a + b / lambda.powi(2) + c / lambda.powi(4), 0.0),
            Material::Dye { n, ref_thickness, transmittance } => {
                // T = exp(-4π k d / λ) for the reference film.
                let t = transmittance(lambda).clamp(1e-6, 1.0);
                Complex64::new(*n, -t.ln() * lambda / (4.0 * std::f64::consts::PI * ref_thickness))
            }
        }
    }
}

fn interpolate(rows: &[(f64, f64, f64)], l: f64) -> (f64, f64) {
    let first = rows[0];
    let last = rows[rows.len() - 1];
    if l <= first.0 {
        return (first.1, first.2);
    }
    if l >= last.0 {
        return (last.1, last.2);
    }
    let i = rows.partition_point(|r| r.0 < l).max(1);
    let (a, b) = (rows[i - 1], rows[i]);
    let f = (l - a.0) / (b.0 - a.0);
    (a.1 + f * (b.1 - a.1), a.2 + f * (b.2 - a.2))
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Transmittance curves of generic colour filter dyes (0.6 µm films, λ in µm), shaped like
/// typical pigment filters: passbands with a few percent leakage and transparency in the near
/// infrared (cameras add an IR-cut filter). Replace with measured data for a real process.
pub mod color_filter {
    use super::logistic;

    const LOW: f64 = 0.03;
    const HIGH: f64 = 0.92;

    pub fn red(l: f64) -> f64 {
        LOW + (HIGH - LOW) * logistic((l - 0.59) / 0.010)
    }

    pub fn green(l: f64) -> f64 {
        let band = logistic((l - 0.490) / 0.012) * logistic((0.590 - l) / 0.012);
        let nir = logistic((l - 0.740) / 0.020);
        LOW + (HIGH - LOW) * (band + nir).min(1.0)
    }

    pub fn blue(l: f64) -> f64 {
        let band = logistic((l - 0.400) / 0.010) * logistic((0.505 - l) / 0.012);
        let nir = logistic((l - 0.780) / 0.020);
        LOW + (HIGH - LOW) * (band + nir).min(1.0)
    }
}

/// A material given in a pixel stack file: a constant index or a table.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum MaterialSpec {
    Constant {
        n: f64,
        #[serde(default)]
        k: f64,
    },
    /// Rows `[wavelength µm, n, k]`.
    Table { table: Vec<[f64; 3]> },
}

/// Materials by lower-case name: the built-in set plus a stack's own.
#[derive(Debug, Clone)]
pub struct Materials(BTreeMap<String, Material>);

impl Default for Materials {
    fn default() -> Self {
        let mut m = BTreeMap::new();
        let mut add = |name: &str, mat: Material| {
            m.insert(name.to_string(), mat);
        };
        add("air", Material::Constant { n: 1.0, k: 0.0 });
        add("si", Material::Table(nk_data::SI.to_vec()));
        add("sio2", Material::Sellmeier(vec![(0.6961663, 0.0684043), (0.4079426, 0.1162414), (0.8974794, 9.896161)]));
        add("si3n4", Material::Sellmeier(vec![(3.0249, 0.1353406), (40314.0, 1239.842)]));
        add("hfo2", Material::Cauchy { a: 1.875, b: 6.28e-3, c: 5.80e-4 });
        add("ta2o5", Material::Table(nk_data::TA2O5.to_vec()));
        add("w", Material::Table(nk_data::W.to_vec()));
        add("al", Material::Table(nk_data::AL.to_vec()));
        add("resin", Material::Cauchy { a: 1.55, b: 0.005, c: 0.0 });
        add("cf_red", Material::Dye { n: 1.62, ref_thickness: 0.6, transmittance: color_filter::red });
        add("cf_green", Material::Dye { n: 1.62, ref_thickness: 0.6, transmittance: color_filter::green });
        add("cf_blue", Material::Dye { n: 1.62, ref_thickness: 0.6, transmittance: color_filter::blue });
        Materials(m)
    }
}

impl Materials {
    pub fn get(&self, name: &str) -> Result<&Material, String> {
        self.0.get(&name.to_ascii_lowercase()).ok_or_else(|| {
            format!("unknown material '{name}' (known: {})", self.0.keys().cloned().collect::<Vec<_>>().join(", "))
        })
    }

    pub fn add(&mut self, name: &str, spec: &MaterialSpec) -> Result<(), String> {
        let m = match spec {
            MaterialSpec::Constant { n, k } => Material::Constant { n: *n, k: *k },
            MaterialSpec::Table { table } => {
                if table.is_empty() || table.windows(2).any(|w| w[1][0] <= w[0][0]) {
                    return Err(format!("material '{name}': the table needs rows of increasing wavelength"));
                }
                Material::Table(table.iter().map(|r| (r[0], r[1], r[2])).collect())
            }
        };
        self.0.insert(name.to_ascii_lowercase(), m);
        Ok(())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_values() {
        let m = Materials::default();
        let at = |name: &str, l: f64| m.get(name).unwrap().index(l);
        // Silicon (Green 2008): n 4.08 / k 0.0405 at 550 nm... check against the table rows.
        let si = at("si", 0.55);
        assert!((si.re - 4.08).abs() < 0.05 && (si.im - 0.028).abs() < 0.02, "{si}");
        // Absorption depth 1/α grows from ~0.1 µm in the blue to ~10 µm in the near infrared.
        let depth = |l: f64| l / (4.0 * std::f64::consts::PI * at("si", l).im);
        assert!(depth(0.45) < 0.5 && depth(0.85) > 10.0, "{} {}", depth(0.45), depth(0.85));
        // Fused silica 1.4585 at 589 nm; Si3N4 ~2.02; HfO2 ~1.9.
        assert!((at("sio2", 0.5893).re - 1.4585).abs() < 2e-4);
        assert!((at("si3n4", 0.6).re - 2.02).abs() < 0.03);
        assert!((at("hfo2", 0.6).re - 1.894).abs() < 0.02);
        // Metals absorb strongly.
        assert!(at("w", 0.55).im > 2.0 && at("al", 0.55).im > 5.0);
    }

    #[test]
    fn colour_filters_pass_their_band() {
        let t = |f: fn(f64) -> f64, l: f64| f(l);
        use color_filter::*;
        assert!(t(red, 0.65) > 0.85 && t(red, 0.45) < 0.05 && t(red, 0.53) < 0.05);
        assert!(t(green, 0.54) > 0.85 && t(green, 0.45) < 0.1 && t(green, 0.65) < 0.1);
        assert!(t(blue, 0.45) > 0.85 && t(blue, 0.55) < 0.1 && t(blue, 0.65) < 0.05);
        // The dye index reproduces the transmittance of its reference film.
        let m = Materials::default();
        let k = m.get("cf_green").unwrap().index(0.54).im;
        let tr = (-4.0 * std::f64::consts::PI * k * 0.6 / 0.54).exp();
        assert!((tr - green(0.54)).abs() < 1e-9);
    }

    #[test]
    fn custom_materials() {
        let mut m = Materials::default();
        m.add("mycf", &MaterialSpec::Table { table: vec![[0.4, 1.6, 0.1], [0.8, 1.5, 0.0]] }).unwrap();
        let v = m.get("MyCF").unwrap().index(0.6);
        assert!((v.re - 1.55).abs() < 1e-12 && (v.im - 0.05).abs() < 1e-12);
        assert!(m.add("bad", &MaterialSpec::Table { table: vec![[0.5, 1.0, 0.0], [0.4, 1.0, 0.0]] }).is_err());
        assert!(m.get("unobtainium").unwrap_err().contains("known: "));
    }
}
