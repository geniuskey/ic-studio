//! Light sources and photocurrent: blackbody spectra scaled to an illuminance (lux) with the
//! CIE 1924 photopic luminosity function, and the photocurrent of a pixel from its quantum
//! efficiency.

const H: f64 = 6.626_070_15e-34;
const C: f64 = 2.997_924_58e8;
const KB: f64 = 1.380_649e-23;
pub const Q: f64 = 1.602_176_634e-19;

/// CIE 1924 photopic luminous efficiency V(λ), 380 - 780 nm in 10 nm steps.
const V: [f64; 41] = [
    0.000039, 0.00012, 0.000396, 0.00121, 0.004, 0.0116, 0.023, 0.038, 0.06, 0.09098, 0.13902, 0.20802, 0.323, 0.503, 0.71,
    0.862, 0.954, 0.99495, 0.995, 0.952, 0.87, 0.757, 0.631, 0.503, 0.381, 0.265, 0.175, 0.107, 0.061, 0.032, 0.017, 0.00821,
    0.004102, 0.002091, 0.001047, 0.00052, 0.000249, 0.00012, 0.00006, 0.00003, 0.000015,
];

/// V(λ) at `lambda` µm (linear interpolation, 0 outside 380 - 780 nm).
pub fn photopic(lambda: f64) -> f64 {
    let x = (lambda * 1000.0 - 380.0) / 10.0;
    if !(0.0..=40.0).contains(&x) {
        return 0.0;
    }
    let i = (x.floor() as usize).min(39);
    let f = x - i as f64;
    V[i] + f * (V[i + 1] - V[i])
}

/// Planck spectral radiance shape at `lambda` µm for temperature `t` K (arbitrary scale).
pub fn blackbody(lambda: f64, t: f64) -> f64 {
    let l = lambda * 1e-6;
    1.0 / (l.powi(5) * ((H * C / (l * KB * t)).exp() - 1.0))
}

/// A spectral irradiance at the pixel: W/m² per µm at each wavelength of a grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    pub wavelengths: Vec<f64>,
    pub irradiance: Vec<f64>,
}

/// Trapezoid integral of `f` over increasing `x`.
pub fn integrate(x: &[f64], f: &[f64]) -> f64 {
    x.windows(2).zip(f.windows(2)).map(|(x, f)| 0.5 * (f[0] + f[1]) * (x[1] - x[0])).sum()
}

impl Spectrum {
    /// A blackbody source at `cct` K giving `lux` at the pixel, on `wavelengths` (µm). With
    /// `ir_cut` (µm), light above it is removed (a camera's IR-cut filter) after scaling.
    pub fn blackbody(wavelengths: &[f64], cct: f64, lux: f64, ir_cut: Option<f64>) -> Spectrum {
        // Photometry over the visible range on a fine grid, independent of `wavelengths`.
        let fine: Vec<f64> = (0..=400).map(|i| 0.38 + i as f64 * 0.001).collect();
        let shape: Vec<f64> = fine.iter().map(|&l| blackbody(l, cct)).collect();
        let lum: Vec<f64> = fine.iter().zip(&shape).map(|(&l, s)| 683.0 * photopic(l) * s).collect();
        let scale = lux / integrate(&fine, &lum);
        let irradiance =
            wavelengths.iter().map(|&l| if ir_cut.is_some_and(|c| l > c) { 0.0 } else { blackbody(l, cct) * scale }).collect();
        Spectrum { wavelengths: wavelengths.to_vec(), irradiance }
    }

    /// Illuminance (lux) of the spectrum over 380 - 780 nm.
    pub fn lux(&self) -> f64 {
        let lum: Vec<f64> = self.wavelengths.iter().zip(&self.irradiance).map(|(&l, e)| 683.0 * photopic(l) * e).collect();
        integrate(&self.wavelengths, &lum)
    }

    /// Photocurrent (A) of a pixel of `area` µm² with quantum efficiency `qe` on the same
    /// wavelength grid: q ∫ QE(λ) E(λ) λ / (hc) dλ · area.
    pub fn photocurrent(&self, qe: &[f64], area_um2: f64) -> f64 {
        let flux: Vec<f64> =
            self.wavelengths.iter().zip(&self.irradiance).zip(qe).map(|((&l, e), q)| q * e * (l * 1e-6) / (H * C)).collect();
        Q * integrate(&self.wavelengths, &flux) * area_um2 * 1e-12
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photopic_function() {
        assert!((photopic(0.555) - 1.0).abs() < 0.01);
        assert_eq!(photopic(0.30), 0.0);
        let x: Vec<f64> = (0..=400).map(|i| 0.38 + i as f64 * 0.001).collect();
        let v: Vec<f64> = x.iter().map(|&l| photopic(l)).collect();
        // ∫ V dλ is about 106.9 nm.
        assert!((integrate(&x, &v) - 0.1069).abs() < 0.0015, "{}", integrate(&x, &v));
    }

    #[test]
    fn illuminance_and_photocurrent() {
        let wl: Vec<f64> = (0..=62).map(|i| 0.38 + i as f64 * 0.01).collect();
        let s = Spectrum::blackbody(&wl, 5500.0, 100.0, None);
        assert!((s.lux() - 100.0).abs() < 1.5, "{}", s.lux());
        // 555 nm monochromatic: 1 lux is 1/683 W/m²; QE 1 over 1 µm² gives q·E·λ/(hc).
        let mono = Spectrum { wavelengths: vec![0.550, 0.555, 0.560], irradiance: vec![0.0, 200.0 / 683.0, 0.0] };
        let i = mono.photocurrent(&[1.0, 1.0, 1.0], 1.0);
        let expect = Q * (1.0 / 683.0) * 0.555e-6 / (H * C) * 1e-12;
        assert!((i / expect - 1.0).abs() < 1e-9, "{i} {expect}");
        // An IR-cut filter lowers the current of a pixel that sees the near infrared.
        let cut = Spectrum::blackbody(&wl, 3000.0, 100.0, Some(0.65));
        let full = Spectrum::blackbody(&wl, 3000.0, 100.0, None);
        let qe = vec![0.5; wl.len()];
        assert!(cut.photocurrent(&qe, 1.0) < 0.8 * full.photocurrent(&qe, 1.0));
    }
}
