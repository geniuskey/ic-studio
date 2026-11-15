//! Rigorous coupled-wave analysis (RCWA) of a structure periodic in x and y: z-invariant layers
//! with a permittivity pattern each, between a semi-infinite incidence medium and a
//! semi-infinite substrate. Scattering-matrix formulation after R. C. Rumpf, "Improved
//! formulation of scattering matrices for semi-analytical methods that is consistent with
//! convention", PIER B 35, 241 (2011). In patterned layers the in-plane permittivities follow
//! L. Li's factorisation rules for crossed gratings (J. Opt. Soc. Am. A 14, 2758 (1997)): the
//! inverse rule across the field's discontinuities and Laurent's rule along them, which makes
//! metal patterns converge; εzz uses Laurent's rule.
//!
//! Units: µm. Inputs are physical complex indices `n + ik`; internally the engineering sign
//! convention of the formulation (loss as negative imaginary permittivity) is used. The result
//! gives the reflectance and the power absorbed in boxes of the substrate, computed from the
//! substrate's plane-wave expansion in closed form.

use faer::Mat;
use faer::linalg::solvers::DenseSolveCore;
use num_complex::Complex64 as C;

type M = Mat<C>;

/// A layer's permittivity: uniform, or sampled on an `nx × ny` grid over one period (row-major
/// in y, cell centres at ((i + ½) Λx / nx, (j + ½) Λy / ny)).
#[derive(Debug, Clone)]
pub enum Eps {
    Uniform(C),
    Grid { nx: usize, ny: usize, samples: Vec<C> },
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub thickness: f64,
    pub eps: Eps,
}

/// One RCWA problem at one wavelength.
#[derive(Debug, Clone)]
pub struct Problem {
    /// Periods (µm).
    pub period: (f64, f64),
    /// Harmonics per direction (odd).
    pub harmonics: (usize, usize),
    pub n_in: C,
    pub layers: Vec<Layer>,
    pub n_sub: C,
    pub wavelength: f64,
    /// Polar and azimuthal angle of incidence (radians).
    pub theta: f64,
    pub phi: f64,
}

/// The solved structure for one source polarisation.
pub struct Solution {
    /// Reflectance.
    pub r: f64,
    /// Transmitted amplitudes (x, y, z) per order, and the orders' wave vectors (1/µm) in the
    /// substrate (field ∝ exp(-i (kx x + ky y + kz z)), z into the substrate).
    t: Vec<[C; 3]>,
    kx: Vec<f64>,
    ky: Vec<f64>,
    kz: Vec<C>,
    k0: f64,
    eps_sub_loss: f64,
    incident_flux: f64,
    area: f64,
}

fn diag_scale_rows(d: &[C], a: &M) -> M {
    Mat::from_fn(a.nrows(), a.ncols(), |i, j| d[i] * a[(i, j)])
}

fn diag_scale_cols(a: &M, d: &[C]) -> M {
    Mat::from_fn(a.nrows(), a.ncols(), |i, j| a[(i, j)] * d[j])
}

fn scaled(a: &M, k: f64) -> M {
    Mat::from_fn(a.nrows(), a.ncols(), |i, j| a[(i, j)] * k)
}

fn inv(a: &M) -> M {
    a.partial_piv_lu().inverse()
}

fn identity(n: usize) -> M {
    Mat::from_fn(n, n, |i, j| if i == j { C::new(1.0, 0.0) } else { C::new(0.0, 0.0) })
}

/// Block matrix [[a, b], [c, d]].
fn blocks(a: &M, b: &M, c: &M, d: &M) -> M {
    let n = a.nrows();
    Mat::from_fn(2 * n, 2 * n, |i, j| match (i < n, j < n) {
        (true, true) => a[(i, j)],
        (true, false) => b[(i, j - n)],
        (false, true) => c[(i - n, j)],
        (false, false) => d[(i - n, j - n)],
    })
}

fn diag(d: &[C]) -> M {
    Mat::from_fn(d.len(), d.len(), |i, j| if i == j { d[i] } else { C::new(0.0, 0.0) })
}

#[derive(Clone)]
struct SMatrix {
    s11: M,
    s12: M,
    s21: M,
    s22: M,
}

impl SMatrix {
    /// Redheffer star product `self ⊗ b`.
    fn star(&self, b: &SMatrix) -> SMatrix {
        let n = self.s11.nrows();
        let i = identity(n);
        let d = &self.s12 * inv(&(&i - &b.s11 * &self.s22));
        let f = &b.s21 * inv(&(&i - &self.s22 * &b.s11));
        SMatrix {
            s11: &self.s11 + &d * &b.s11 * &self.s21,
            s12: &d * &b.s12,
            s21: &f * &self.s21,
            s22: &b.s22 + &f * &self.s22 * &b.s12,
        }
    }
}

/// Fourier coefficients ε̂(p, q) = mean of ε exp(-i 2π (p x / Λx + q y / Λy)) for |p| ≤ px,
/// |q| ≤ py, indexed [(p + px) * (2 py + 1) + q + py].
fn fourier(nx: usize, ny: usize, s: &[C], px: i64, py: i64) -> Vec<C> {
    let tau = 2.0 * std::f64::consts::PI;
    let wp = (2 * px + 1) as usize;
    // Along x for every row j.
    let mut a = vec![C::new(0.0, 0.0); wp * ny];
    for j in 0..ny {
        for (ip, p) in (-px..=px).enumerate() {
            let mut acc = C::new(0.0, 0.0);
            for i in 0..nx {
                let ph = -tau * p as f64 * (i as f64 + 0.5) / nx as f64;
                acc += s[j * nx + i] * C::new(ph.cos(), ph.sin());
            }
            a[ip * ny + j] = acc;
        }
    }
    let wq = (2 * py + 1) as usize;
    let mut out = vec![C::new(0.0, 0.0); wp * wq];
    for ip in 0..wp {
        for (iq, q) in (-py..=py).enumerate() {
            let mut acc = C::new(0.0, 0.0);
            for j in 0..ny {
                let ph = -tau * q as f64 * (j as f64 + 0.5) / ny as f64;
                acc += a[ip * ny + j] * C::new(ph.cos(), ph.sin());
            }
            out[ip * wq + iq] = acc / (nx * ny) as f64;
        }
    }
    out
}

/// 1-D Fourier coefficients c(d) = mean of f exp(-i 2π d k / n) for |d| ≤ dmax, indexed d + dmax.
fn fourier_1d(f: &[C], dmax: i64) -> Vec<C> {
    let n = f.len();
    let tau = 2.0 * std::f64::consts::PI;
    (-dmax..=dmax)
        .map(|d| {
            let mut acc = C::new(0.0, 0.0);
            for (k, v) in f.iter().enumerate() {
                let ph = -tau * d as f64 * (k as f64 + 0.5) / n as f64;
                acc += v * C::new(ph.cos(), ph.sin());
            }
            acc / n as f64
        })
        .collect()
}

/// Li's mixed rule for a sampled pattern: the inverse rule along `inv_axis` (0 = x, 1 = y),
/// Laurent's rule along the other; returned on the (p, q) order list.
fn li_matrix(nx: usize, ny: usize, s: &[C], px: i64, py: i64, pq: &[(i64, i64)], inv_axis: usize) -> M {
    let n = pq.len();
    let (h_inv, n_inv, n_lau) = if inv_axis == 0 { (px, nx, ny) } else { (py, ny, nx) };
    let hi = (2 * h_inv + 1) as usize;
    // For every line along the inverse axis: the inverse of the Toeplitz matrix of 1/ε.
    let lines: Vec<M> = (0..n_lau)
        .map(|l| {
            let f: Vec<C> =
                (0..n_inv).map(|k| C::new(1.0, 0.0) / if inv_axis == 0 { s[l * nx + k] } else { s[k * nx + l] }).collect();
            let c = fourier_1d(&f, 2 * h_inv);
            inv(&Mat::from_fn(hi, hi, |a, b| c[(a as i64 - b as i64 + 2 * h_inv) as usize]))
        })
        .collect();
    // Laurent's rule across the lines.
    let tau = 2.0 * std::f64::consts::PI;
    Mat::from_fn(n, n, |i, j| {
        let ((pi, qi), (pj, qj)) = (pq[i], pq[j]);
        let (a, b, d) = if inv_axis == 0 { (pi, pj, qi - qj) } else { (qi, qj, pi - pj) };
        let (ra, rb) = ((a + h_inv) as usize, (b + h_inv) as usize);
        let mut acc = C::new(0.0, 0.0);
        for (l, m) in lines.iter().enumerate() {
            let ph = -tau * d as f64 * (l as f64 + 0.5) / n_lau as f64;
            acc += m[(ra, rb)] * C::new(ph.cos(), ph.sin());
        }
        acc / n_lau as f64
    })
}

/// Longitudinal wave number for `kz² = z` (engineering convention, field exp(-i kz z)): the
/// root that propagates forward (Re ≥ 0) or decays (Im ≤ 0). Chosen explicitly, not by the
/// sign of a zero imaginary part.
fn kz_root(z: C) -> C {
    let k = z.sqrt();
    if k.im > 0.0 { -k } else { k }
}

/// Eigenvalue λ of a layer mode (field exp(-λ z)) from λ²: decaying (Re > 0) or, for
/// propagating modes, forward (Im > 0).
fn mode_root(l2: C) -> C {
    // A mode at cutoff (λ = 0) would divide by zero in V = Q W λ⁻¹.
    let l = if l2.norm() < 1e-18 { C::new(1e-9, 0.0) } else { l2.sqrt() };
    let tol = 1e-12 * l.norm().max(1.0);
    if l.re < -tol || (l.re.abs() <= tol && l.im < 0.0) { -l } else { l }
}

impl Problem {
    pub fn solve(&self) -> Vec<Solution> {
        self.solve_pols(&[(1.0, 0.0), (0.0, 1.0)])
    }

    /// Solve for source polarisations given as (TE, TM) amplitude pairs.
    pub fn solve_pols(&self, pols: &[(f64, f64)]) -> Vec<Solution> {
        let (hx, hy) = self.harmonics;
        let (px, py) = ((hx as i64 - 1) / 2, (hy as i64 - 1) / 2);
        let n = hx * hy;
        let k0 = 2.0 * std::f64::consts::PI / self.wavelength;
        let n_in = self.n_in.re;
        let kinc = [n_in * self.theta.sin() * self.phi.cos(), n_in * self.theta.sin() * self.phi.sin(), n_in * self.theta.cos()];
        let (lx, ly) = self.period;
        // Orders m = (p, q), normalised wave vectors.
        let mut pq = Vec::with_capacity(n);
        for p in -px..=px {
            for q in -py..=py {
                pq.push((p, q));
            }
        }
        let kx: Vec<C> = pq.iter().map(|&(p, _)| C::new(kinc[0] - p as f64 * self.wavelength / lx, 0.0)).collect();
        let ky: Vec<C> = pq.iter().map(|&(_, q)| C::new(kinc[1] - q as f64 * self.wavelength / ly, 0.0)).collect();
        // At a Rayleigh anomaly an order grazes the vacuum gap or the incidence medium (kz = 0),
        // which makes the scattering matrices singular: solve a hair off that wavelength.
        let grazing = |e: f64| kx.iter().zip(&ky).any(|(a, b)| (e - a.re * a.re - b.re * b.re).abs() < 1e-9);
        if grazing(1.0) || grazing(n_in * n_in) {
            let nudged = Problem { wavelength: self.wavelength * (1.0 + 1e-7), ..self.clone() };
            return nudged.solve_pols(pols);
        }
        let kx2: Vec<C> = kx.iter().map(|k| k * k).collect();
        let ky2: Vec<C> = ky.iter().map(|k| k * k).collect();
        let kxy: Vec<C> = kx.iter().zip(&ky).map(|(a, b)| a * b).collect();

        // Homogeneous medium of permittivity e (engineering sign): Q, and the kz of each order.
        let q_hom = |e: C| -> M {
            blocks(
                &diag(&kxy),
                &diag(&kx2.iter().map(|k2| e - k2).collect::<Vec<_>>()),
                &diag(&ky2.iter().map(|k2| k2 - e).collect::<Vec<_>>()),
                &diag(&kxy.iter().map(|k| -k).collect::<Vec<_>>()),
            )
        };
        let kz_of = |e: C| -> Vec<C> {
            kx2.iter()
                .zip(&ky2)
                .map(|(a, b)| {
                    let z = kz_root(e - a - b);
                    if z.norm() < 1e-9 { C::new(1e-9, 0.0) } else { z }
                })
                .collect()
        };
        // Gap medium (vacuum): W0 = I, V0 = Q0 λ0⁻¹.
        let kz0 = kz_of(C::new(1.0, 0.0));
        let lam0: Vec<C> = kz0.iter().chain(&kz0).map(|k| C::new(0.0, 1.0) * k).collect();
        let v0 = diag_scale_cols(&q_hom(C::new(1.0, 0.0)), &lam0.iter().map(|l| 1.0 / l).collect::<Vec<_>>());
        let v0_inv = inv(&v0);

        // Reflection side.
        let e_in = (self.n_in * self.n_in).conj();
        let kz_ref: Vec<C> = kz_of(e_in).iter().map(|k| -k).collect();
        let lam_ref: Vec<C> = kz_ref.iter().chain(&kz_ref).map(|k| C::new(0.0, -1.0) * k).collect();
        let v_ref = diag_scale_cols(&q_hom(e_in), &lam_ref.iter().map(|l| 1.0 / l).collect::<Vec<_>>());
        let a = identity(2 * n) + &v0_inv * &v_ref;
        let b = identity(2 * n) - &v0_inv * &v_ref;
        let a_inv = inv(&a);
        let s_ref = SMatrix {
            s11: -(&a_inv * &b),
            s12: scaled(&a_inv, 2.0),
            s21: scaled(&(&a - &b * &a_inv * &b), 0.5),
            s22: &b * &a_inv,
        };

        // Layers.
        let mut global = s_ref;
        for layer in &self.layers {
            let s = match &layer.eps {
                Eps::Uniform(e) => {
                    let e = e.conj();
                    let kz = kz_of(e);
                    let lam: Vec<C> = kz.iter().chain(&kz).map(|k| C::new(0.0, 1.0) * k).collect();
                    let v = diag_scale_cols(&q_hom(e), &lam.iter().map(|l| 1.0 / l).collect::<Vec<_>>());
                    layer_s(&identity(2 * n), &v, &lam, &v0, k0, layer.thickness)
                }
                Eps::Grid { nx, ny, samples } => {
                    let conj: Vec<C> = samples.iter().map(|e| e.conj()).collect();
                    let f = fourier(*nx, *ny, &conj, 2 * px, 2 * py);
                    let wq = (4 * py + 1) as usize;
                    let erc = Mat::from_fn(n, n, |i, j| {
                        let (dp, dq) = (pq[i].0 - pq[j].0 + 2 * px, pq[i].1 - pq[j].1 + 2 * py);
                        f[dp as usize * wq + dq as usize]
                    });
                    let ei = inv(&erc);
                    let p11 = diag_scale_cols(&diag_scale_rows(&kx, &ei), &ky);
                    let p12 = identity(n) - diag_scale_cols(&diag_scale_rows(&kx, &ei), &kx);
                    let p21 = diag_scale_cols(&diag_scale_rows(&ky, &ei), &ky) - identity(n);
                    let p22 = -diag_scale_cols(&diag_scale_rows(&ky, &ei), &kx);
                    let p = blocks(&p11, &p12, &p21, &p22);
                    // εxx: Ex jumps across x boundaries (inverse rule in x); εyy likewise in y.
                    let exx = li_matrix(*nx, *ny, &conj, px, py, &pq, 0);
                    let eyy = li_matrix(*nx, *ny, &conj, px, py, &pq, 1);
                    let q = blocks(
                        &diag(&kxy),
                        &(&eyy - diag(&kx2)),
                        &(diag(&ky2) - &exx),
                        &diag(&kxy.iter().map(|k| -k).collect::<Vec<_>>()),
                    );
                    let omega2 = &p * &q;
                    let evd = omega2.eigen().expect("eigendecomposition");
                    let w: M = evd.U().to_owned();
                    let lam: Vec<C> = (0..2 * n).map(|i| mode_root(evd.S()[i])).collect();
                    let v = diag_scale_cols(&(&q * &w), &lam.iter().map(|l| 1.0 / l).collect::<Vec<_>>());
                    layer_s(&w, &v, &lam, &v0, k0, layer.thickness)
                }
            };
            global = global.star(&s);
        }

        // Transmission side.
        let e_sub = (self.n_sub * self.n_sub).conj();
        let kz_trn = kz_of(e_sub);
        let lam_trn: Vec<C> = kz_trn.iter().chain(&kz_trn).map(|k| C::new(0.0, 1.0) * k).collect();
        let v_trn = diag_scale_cols(&q_hom(e_sub), &lam_trn.iter().map(|l| 1.0 / l).collect::<Vec<_>>());
        let a = identity(2 * n) + &v0_inv * &v_trn;
        let b = identity(2 * n) - &v0_inv * &v_trn;
        let a_inv = inv(&a);
        let s_trn = SMatrix {
            s11: &b * &a_inv,
            s12: scaled(&(&a - &b * &a_inv * &b), 0.5),
            s21: scaled(&a_inv, 2.0),
            s22: -(&a_inv * &b),
        };
        let global = global.star(&s_trn);

        // Sources.
        let centre = pq.iter().position(|&(p, q)| p == 0 && q == 0).unwrap();
        let kn = [kinc[0] / n_in, kinc[1] / n_in, kinc[2] / n_in];
        let ate = if self.theta.abs() < 1e-12 {
            [0.0, 1.0, 0.0]
        } else {
            // a_TE = k × z / |k × z|
            let c = [kn[1], -kn[0], 0.0];
            let l = (c[0] * c[0] + c[1] * c[1]).sqrt();
            [c[0] / l, c[1] / l, 0.0]
        };
        // a_TM = a_TE × k
        let atm = [ate[1] * kn[2] - ate[2] * kn[1], ate[2] * kn[0] - ate[0] * kn[2], ate[0] * kn[1] - ate[1] * kn[0]];
        let eps_sub_loss = (self.n_sub * self.n_sub).im;
        pols.iter()
            .map(|&(te, tm)| {
                let pv = [te * ate[0] + tm * atm[0], te * ate[1] + tm * atm[1]];
                let norm = (te * te + tm * tm).sqrt();
                let mut esrc = Mat::<C>::zeros(2 * n, 1);
                esrc[(centre, 0)] = C::new(pv[0] / norm, 0.0);
                esrc[(n + centre, 0)] = C::new(pv[1] / norm, 0.0);
                // W_ref = I: c_src = e_src.
                let cref = &global.s11 * &esrc;
                let ctrn = &global.s21 * &esrc;
                let r2: f64 = (0..n)
                    .map(|m| {
                        let (rx, ry) = (cref[(m, 0)], cref[(n + m, 0)]);
                        let rz = -(kx[m] * rx + ky[m] * ry) / kz_ref[m];
                        (-kz_ref[m]).re * (rx.norm_sqr() + ry.norm_sqr() + rz.norm_sqr())
                    })
                    .sum();
                let t: Vec<[C; 3]> = (0..n)
                    .map(|m| {
                        let (tx, ty) = (ctrn[(m, 0)], ctrn[(n + m, 0)]);
                        let tz = -(kx[m] * tx + ky[m] * ty) / kz_trn[m];
                        [tx, ty, tz]
                    })
                    .collect();
                Solution {
                    r: r2 / kinc[2],
                    t,
                    kx: kx.iter().map(|k| k.re * k0).collect(),
                    ky: ky.iter().map(|k| k.re * k0).collect(),
                    kz: kz_trn.iter().map(|k| k * k0).collect(),
                    k0,
                    eps_sub_loss,
                    incident_flux: kinc[2],
                    area: lx * ly,
                }
            })
            .collect()
    }
}

/// S-matrix of a layer with eigenmodes (W, V, λ) and thickness `l` against the vacuum gap.
fn layer_s(w: &M, v: &M, lam: &[C], v0: &M, k0: f64, l: f64) -> SMatrix {
    let wi = inv(w);
    let vi = inv(v);
    let a = &wi + &vi * v0;
    let b = &wi - &vi * v0;
    let x: Vec<C> = lam.iter().map(|g| (-g * k0 * l).exp()).collect();
    let a_inv = inv(&a);
    let xb = diag_scale_rows(&x, &b);
    let xa = diag_scale_rows(&x, &a);
    let xba = &xb * &a_inv;
    let d = &a - &xba * &xb;
    let d_inv = inv(&d);
    let s11 = &d_inv * (&xba * &xa - &b);
    let s12 = &d_inv * diag_scale_rows(&x, &(&a - &b * &a_inv * &b));
    SMatrix { s11: s11.clone(), s12: s12.clone(), s21: s12, s22: s11 }
}

/// ∫ exp(-i β s) ds over [a, b].
fn span(beta: C, a: f64, b: f64) -> C {
    if beta.norm() < 1e-12 {
        return C::new(b - a, 0.0);
    }
    let i = C::new(0.0, 1.0);
    ((-i * beta * b).exp() - (-i * beta * a).exp()) / (-i * beta)
}

impl Solution {
    /// Fraction of the incident power absorbed in the substrate box x ∈ [x0, x1], y ∈ [y0, y1],
    /// depth ∈ [d0, d1] (µm, depth from the substrate surface; `d1` may be infinite).
    pub fn absorbed(&self, x: (f64, f64), y: (f64, f64), d: (f64, f64)) -> f64 {
        let n = self.t.len();
        let mut sum = C::new(0.0, 0.0);
        for m in 0..n {
            for k in 0..n {
                let dot =
                    self.t[m][0] * self.t[k][0].conj() + self.t[m][1] * self.t[k][1].conj() + self.t[m][2] * self.t[k][2].conj();
                if dot.norm() < 1e-14 {
                    continue;
                }
                let ix = span(C::new(self.kx[m] - self.kx[k], 0.0), x.0, x.1);
                let iy = span(C::new(self.ky[m] - self.ky[k], 0.0), y.0, y.1);
                let beta = self.kz[m] - self.kz[k].conj();
                let iz = if d.1.is_infinite() {
                    // exp(-iβ d) decays (Im β < 0): ∫_d0^∞ = exp(-iβ d0) / (iβ).
                    let i = C::new(0.0, 1.0);
                    (-i * beta * d.0).exp() / (i * beta)
                } else {
                    span(beta, d.0, d.1)
                };
                sum += dot * ix * iy * iz;
            }
        }
        self.k0 * self.eps_sub_loss * sum.re / (self.incident_flux * self.area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmm;

    fn c(n: f64, k: f64) -> C {
        C::new(n, k)
    }

    fn uniform(n: C, t: f64) -> Layer {
        Layer { thickness: t, eps: Eps::Uniform(n * n) }
    }

    #[test]
    fn uniform_stack_matches_thin_film_optics() {
        let si = c(4.08, 0.03);
        let films = [(c(2.1, 0.0), 0.06), (c(1.9, 0.0), 0.01), (c(1.6, 0.02), 0.3)];
        for theta in [0.0f64, 0.35] {
            let pr = Problem {
                period: (1.0, 1.0),
                harmonics: (3, 3),
                n_in: c(1.46, 0.0),
                layers: films.iter().map(|&(n, t)| uniform(n, t)).collect(),
                n_sub: si,
                wavelength: 0.55,
                theta,
                phi: 0.3,
            };
            let sols = pr.solve();
            for (k, pol) in [tmm::Pol::S, tmm::Pol::P].into_iter().enumerate() {
                let x = tmm::rt(pol, c(1.46, 0.0), &films, si, theta, 0.55);
                let s = &sols[k];
                let a_sub = s.absorbed((0.0, 1.0), (0.0, 1.0), (0.0, f64::INFINITY));
                assert!((s.r - x.r).abs() < 1e-6, "{pol:?} θ {theta}: R {} vs {}", s.r, x.r);
                assert!((a_sub - x.t).abs() < 1e-6, "{pol:?} θ {theta}: T {} vs {}", a_sub, x.t);
            }
        }
    }

    #[test]
    fn grating_conserves_energy() {
        // A lossless crossed grating over silicon: reflected + absorbed in silicon = 1.
        let (nx, ny) = (24, 24);
        let samples: Vec<C> = (0..nx * ny)
            .map(|k| {
                let (i, j) = (k % nx, k / nx);
                if i < 10 && j < 14 { c(2.0, 0.0) * c(2.0, 0.0) } else { c(1.45, 0.0) * c(1.45, 0.0) }
            })
            .collect();
        let pr = Problem {
            period: (0.8, 0.8),
            harmonics: (7, 7),
            n_in: c(1.0, 0.0),
            layers: vec![Layer { thickness: 0.3, eps: Eps::Grid { nx, ny, samples } }, uniform(c(1.46, 0.0), 0.1)],
            n_sub: c(4.08, 0.03),
            wavelength: 0.55,
            theta: 0.2,
            phi: 0.0,
        };
        for s in pr.solve() {
            let a = s.absorbed((0.0, 0.8), (0.0, 0.8), (0.0, f64::INFINITY));
            assert!((s.r + a - 1.0).abs() < 1e-6, "R {} + A {}", s.r, a);
            // The four quadrants add up to the whole.
            let q: f64 = [(0.0, 0.4), (0.4, 0.8)]
                .iter()
                .flat_map(|&x| [(0.0, 0.4), (0.4, 0.8)].map(move |y| (x, y)))
                .map(|(x, y)| s.absorbed(x, y, (0.0, f64::INFINITY)))
                .sum();
            assert!((q - a).abs() < 1e-9, "{q} vs {a}");
        }
    }
}
