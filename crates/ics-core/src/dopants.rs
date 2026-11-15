//! Dopant physics for the process emulator (`process`): ion implant ranges, intrinsic diffusion,
//! thermal oxidation (Deal-Grove) and carrier mobility. These are the textbook first-order
//! models, enough to place wells, junctions and sheet resistances, not a replacement for TCAD:
//!
//! - implants are Gaussian (projected range Rp and straggle ΔRp in silicon from LSS / Gibbons
//!   tables, lateral straggle 0.8 ΔRp); masking layers stop ions like silicon;
//! - diffusion is intrinsic, `D = D0 exp(-Ea / kT)`, without transient enhanced, oxidation
//!   enhanced or concentration-dependent diffusion and without segregation at interfaces;
//! - oxidation follows Deal-Grove for (100) silicon, dry or wet, and consumes 0.44 of the
//!   oxide thickness in silicon; it is vertical (no bird's beak);
//! - mobility is Caughey-Thomas at 300 K.

/// Boltzmann constant (eV/K).
const KB: f64 = 8.617_333e-5;
/// Elementary charge (C).
pub const Q: f64 = 1.602_176_634e-19;

/// A dopant species.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    Boron,
    Phosphorus,
    Arsenic,
    Antimony,
}

impl Species {
    pub const ALL: [Species; 4] = [Species::Boron, Species::Phosphorus, Species::Arsenic, Species::Antimony];

    /// The species of a name; `bf2` is boron (implanted as BF₂, see [`implant_range`]).
    pub fn parse(name: &str) -> Option<Species> {
        match name.trim().to_ascii_lowercase().as_str() {
            "b" | "boron" | "bf2" => Some(Species::Boron),
            "p" | "phosphorus" => Some(Species::Phosphorus),
            "as" | "arsenic" => Some(Species::Arsenic),
            "sb" | "antimony" => Some(Species::Antimony),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Species::Boron => "boron",
            Species::Phosphorus => "phosphorus",
            Species::Arsenic => "arsenic",
            Species::Antimony => "antimony",
        }
    }

    /// Donors make silicon n-type, acceptors p-type.
    pub fn is_donor(self) -> bool {
        !matches!(self, Species::Boron)
    }

    /// Intrinsic diffusivity (cm²/s) at `celsius`.
    pub fn diffusivity(self, celsius: f64) -> f64 {
        let (d0, ea) = match self {
            Species::Boron => (0.76, 3.46),
            Species::Phosphorus => (3.85, 3.66),
            Species::Arsenic => (0.066, 3.44),
            Species::Antimony => (0.214, 3.65),
        };
        d0 * (-ea / (KB * (celsius + 273.15))).exp()
    }
}

/// Projected range and straggle (µm) in silicon: (keV, Rp, ΔRp).
const RANGES_B: [(f64, f64, f64); 7] = [
    (10.0, 0.0333, 0.0171),
    (20.0, 0.0662, 0.0283),
    (30.0, 0.0987, 0.0371),
    (50.0, 0.1608, 0.0504),
    (100.0, 0.3070, 0.0710),
    (200.0, 0.5290, 0.0921),
    (400.0, 0.8600, 0.1130),
];
const RANGES_P: [(f64, f64, f64); 7] = [
    (10.0, 0.0139, 0.0069),
    (20.0, 0.0253, 0.0119),
    (30.0, 0.0368, 0.0163),
    (50.0, 0.0607, 0.0256),
    (100.0, 0.1238, 0.0456),
    (200.0, 0.2539, 0.0811),
    (400.0, 0.5000, 0.1300),
];
const RANGES_AS: [(f64, f64, f64); 6] = [
    (10.0, 0.0097, 0.0036),
    (20.0, 0.0159, 0.0059),
    (30.0, 0.0228, 0.0083),
    (50.0, 0.0322, 0.0118),
    (100.0, 0.0582, 0.0207),
    (200.0, 0.1105, 0.0378),
];
const RANGES_SB: [(f64, f64, f64); 5] =
    [(10.0, 0.0088, 0.0026), (30.0, 0.0199, 0.0058), (50.0, 0.0283, 0.0083), (100.0, 0.0459, 0.0139), (200.0, 0.0818, 0.0250)];

/// Log-log interpolation in a range table, extrapolated with the end slopes.
fn interp(t: &[(f64, f64, f64)], e: f64) -> (f64, f64) {
    let k = t.iter().position(|r| r.0 >= e).unwrap_or(t.len() - 1).clamp(1, t.len() - 1);
    let (a, b) = (t[k - 1], t[k]);
    let f = (e.ln() - a.0.ln()) / (b.0.ln() - a.0.ln());
    let lerp = |x: f64, y: f64| (x.ln() + f * (y.ln() - x.ln())).exp();
    (lerp(a.1, b.1), lerp(a.2, b.2))
}

/// Projected range Rp and straggle ΔRp (µm) of `ion` (a [`Species`] name, or `bf2`) at
/// `kev` in silicon. BF₂ carries 11/49 of its energy on the boron.
pub fn implant_range(ion: &str, kev: f64) -> Option<(f64, f64)> {
    let s = Species::parse(ion)?;
    let e = if ion.trim().eq_ignore_ascii_case("bf2") { kev * 11.0 / 49.0 } else { kev };
    if !(e.is_finite() && e > 0.0) {
        return None;
    }
    Some(match s {
        Species::Boron => interp(&RANGES_B, e),
        Species::Phosphorus => interp(&RANGES_P, e),
        Species::Arsenic => interp(&RANGES_AS, e),
        Species::Antimony => interp(&RANGES_SB, e),
    })
}

/// Deal-Grove oxide thickness (µm) after `minutes` at `celsius` on (100) silicon, starting from
/// `x0` µm of oxide; `wet` is steam, else dry O₂.
pub fn oxide_growth(celsius: f64, minutes: f64, wet: bool, x0: f64) -> f64 {
    let kt = KB * (celsius + 273.15);
    // B (µm²/h) and B/A (µm/h).
    let (c1, e1, c2, e2) = if wet { (386.0, 0.78, 9.70e7, 2.05) } else { (772.0, 1.23, 3.71e6, 2.00) };
    let b = c1 * (-e1 / kt).exp();
    let a = b / (c2 * (-e2 / kt).exp());
    let tau = (x0 * x0 + a * x0) / b;
    let t = minutes / 60.0 + tau;
    (-a + (a * a + 4.0 * b * t).sqrt()) / 2.0
}

/// Carrier mobility (cm²/V·s) at total impurity concentration `n` (cm⁻³), Caughey-Thomas.
pub fn mobility(electrons: bool, n: f64) -> f64 {
    let (min, max, nref, alpha) = if electrons { (68.5, 1414.0, 9.2e16, 0.711) } else { (44.9, 470.5, 2.23e17, 0.719) };
    min + (max - min) / (1.0 + (n.max(0.0) / nref).powf(alpha))
}

/// Solve the tridiagonal system of one line of implicit diffusion with zero flux at both ends:
/// `(1 + r·k_i) c_i - r c_{i-1} - r c_{i+1} = d_i`, `k_i` the number of neighbours.
fn implicit_line(c: &mut [f64], r: f64, cp: &mut Vec<f64>) {
    let n = c.len();
    if n < 2 {
        return;
    }
    cp.clear();
    cp.resize(n, 0.0);
    // Thomas algorithm; sub- and super-diagonal are -r.
    let diag = |i: usize| 1.0 + r * if i == 0 || i == n - 1 { 1.0 } else { 2.0 };
    let mut b = diag(0);
    cp[0] = -r / b;
    c[0] /= b;
    for i in 1..n {
        b = diag(i) + r * cp[i - 1];
        cp[i] = -r / b;
        c[i] = (c[i] + r * c[i - 1]) / b;
    }
    for i in (0..n - 1).rev() {
        c[i] -= cp[i] * c[i + 1];
    }
}

/// Diffuse `c` (row-major `nx` × `nz`) for `dt_total` = D·t (cm²) with cell size `h` (cm) over the
/// cells where `inside` is true, with no flux out of them. Locally one-dimensional backward
/// Euler (x, then z): positive and conservative, and each step adds exactly 2 D dt to the
/// variance. A large step has exponential rather than Gaussian tails, though, and junctions
/// lie in the tails (5 to 7 σ), so it takes up to 100 steps: the sum of n steps follows the
/// Gaussian out to about √(n/2) σ.
pub fn diffuse(c: &mut [f64], inside: &[bool], nx: usize, nz: usize, dt_total: f64, h: f64) {
    if dt_total <= 0.0 {
        return;
    }
    let steps = ((4.0 * dt_total / (h * h)).ceil() as usize).clamp(1, 100);
    let r = dt_total / steps as f64 / (h * h);
    // Only the rows that hold inside cells.
    let rows: Vec<usize> = (0..nz).filter(|iz| inside[iz * nx..(iz + 1) * nx].iter().any(|i| *i)).collect();
    let (Some(&z_lo), Some(&z_hi)) = (rows.first(), rows.last()) else { return };
    let mut line = Vec::new();
    let mut cp = Vec::new();
    for _ in 0..steps {
        // x lines: runs of inside cells per row.
        for &iz in &rows {
            let row = iz * nx;
            let mut ix = 0;
            while ix < nx {
                if !inside[row + ix] {
                    ix += 1;
                    continue;
                }
                let s = ix;
                while ix < nx && inside[row + ix] {
                    ix += 1;
                }
                implicit_line(&mut c[row + s..row + ix], r, &mut cp);
            }
        }
        // z lines.
        for ix in 0..nx {
            let mut iz = z_lo;
            while iz <= z_hi {
                if !inside[iz * nx + ix] {
                    iz += 1;
                    continue;
                }
                let s = iz;
                while iz <= z_hi && inside[iz * nx + ix] {
                    iz += 1;
                }
                line.clear();
                line.extend((s..iz).map(|k| c[k * nx + ix]));
                implicit_line(&mut line, r, &mut cp);
                for (k, v) in (s..iz).zip(&line) {
                    c[k * nx + ix] = *v;
                }
            }
        }
    }
}

/// Convolve each row of `c` (row-major `nx` wide) with a Gaussian of `sigma` cells.
pub fn blur_rows(c: &mut [f64], nx: usize, sigma: f64) {
    if sigma < 0.3 {
        return;
    }
    let half = (3.0 * sigma).ceil() as isize;
    let w: Vec<f64> = (-half..=half).map(|k| (-(k * k) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let mut out = vec![0.0; nx];
    for row in c.chunks_mut(nx) {
        if row.iter().all(|v| *v == 0.0) {
            continue;
        }
        for (ix, o) in out.iter_mut().enumerate() {
            let (mut s, mut ws) = (0.0, 0.0);
            for (k, wk) in w.iter().enumerate() {
                let j = ix as isize + k as isize - half;
                if (0..nx as isize).contains(&j) {
                    s += wk * row[j as usize];
                    ws += wk;
                }
            }
            *o = s / ws;
        }
        row.copy_from_slice(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_diffusivities() {
        let (rp, drp) = implant_range("boron", 30.0).unwrap();
        assert!((rp - 0.0987).abs() < 1e-9 && (drp - 0.0371).abs() < 1e-9);
        let (rp40, _) = implant_range("arsenic", 40.0).unwrap();
        assert!(rp40 > 0.0228 && rp40 < 0.0322);
        // BF2 at 49 keV is boron at 11 keV.
        let (a, _) = implant_range("bf2", 49.0).unwrap();
        let (b, _) = implant_range("boron", 11.0).unwrap();
        assert!((a - b).abs() < 1e-12);
        assert!(implant_range("xenon", 30.0).is_none());
        // Boron at 1000 °C: about 1.5e-14 cm²/s; arsenic diffuses an order slower.
        let d = Species::Boron.diffusivity(1000.0);
        assert!(d > 1e-14 && d < 2.5e-14, "{d}");
        assert!(Species::Arsenic.diffusivity(1000.0) < d / 5.0);
    }

    #[test]
    fn deal_grove() {
        // Textbook values: dry 1000 °C 1 h ≈ 0.04 µm, wet 1000 °C 1 h ≈ 0.4 µm.
        let dry = oxide_growth(1000.0, 60.0, false, 0.0);
        let wet = oxide_growth(1000.0, 60.0, true, 0.0);
        assert!((0.03..0.06).contains(&dry), "{dry}");
        assert!((0.3..0.5).contains(&wet), "{wet}");
        // Two half hours give the same as one hour.
        let half = oxide_growth(1000.0, 30.0, true, 0.0);
        assert!((oxide_growth(1000.0, 30.0, true, half) - wet).abs() < 1e-9);
    }

    #[test]
    fn diffusion_conserves_dose_and_spreads_as_sqrt_2dt() {
        // A line source in the middle of a 1-D strip (nz = 1).
        let (nx, h) = (401, 1e-7_f64);
        let mut c = vec![0.0; nx];
        c[200] = 1.0;
        let inside = vec![true; nx];
        let dt = (20.0 * h).powi(2) / 2.0; // sigma = 20 cells
        diffuse(&mut c, &inside, nx, 1, dt, h);
        let sum: f64 = c.iter().sum();
        let var: f64 = c.iter().enumerate().map(|(i, v)| v * (i as f64 - 200.0).powi(2)).sum::<f64>() / sum;
        assert!((sum - 1.0).abs() < 1e-12);
        assert!((var.sqrt() / 20.0 - 1.0).abs() < 0.02, "{}", var.sqrt());
        assert!(c.iter().all(|v| *v >= 0.0));
        // No flux out of the region: a wall keeps everything on its side.
        let mut c = vec![0.0; nx];
        c[100] = 1.0;
        let inside: Vec<bool> = (0..nx).map(|i| i < 150).collect();
        diffuse(&mut c, &inside, nx, 1, dt * 50.0, h);
        assert!(c[150..].iter().all(|v| *v == 0.0) && (c.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn mobility_falls_with_doping() {
        assert!((mobility(true, 1e14) - 1414.0).abs() < 20.0);
        assert!(mobility(true, 1e20) < 100.0 && mobility(false, 1e20) < 60.0);
    }
}
