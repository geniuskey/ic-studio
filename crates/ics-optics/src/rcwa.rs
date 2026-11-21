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
    /// Layers whose fields the solution keeps, for the power absorbed inside them (e.g. the
    /// silicon of a back-side illuminated pixel, patterned by its trenches). Contiguous; depths
    /// in the solution count from the top of the first one.
    pub monitor: std::ops::Range<usize>,
}

/// The power-loss part of a monitored layer's permittivity, Im(n²) ≥ 0 (physical sign).
#[derive(Debug, Clone)]
enum Loss {
    Uniform(f64),
    Grid { nx: usize, ny: usize, im: Vec<f64> },
}

/// The eigenmodes of a monitored layer, shared by the polarisations.
struct LayerModes {
    thickness: f64,
    /// Mode eigenvalues (field exp(-λ k0 z)).
    lam: Vec<C>,
    w: M,
    v: M,
    /// εzz⁻¹ in the order basis, for Ez = εzz⁻¹ (Kx uy - Ky ux).
    ez: M,
    loss: Loss,
    /// Patterned layers: Li's εxx and εyy and the sampled permittivity (engineering sign).
    /// The in-plane field is rebuilt as D / ε point by point, which, unlike the Fourier sum
    /// of E, has no overshoot at the jumps of the normal component across the pattern's edges.
    pattern: Option<(M, M, usize, usize, Vec<C>)>,
}

/// The field of a monitored layer for one polarisation: forward mode amplitudes at its top and
/// backward amplitudes at its bottom (stable for thick, absorbing layers).
struct LayerField {
    modes: std::sync::Arc<LayerModes>,
    /// Depth of the layer's top below the first monitored layer's top (µm).
    depth: f64,
    cp: Vec<C>,
    cm: Vec<C>,
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
    /// Transmittance into a lossless substrate (power flux of the transmitted orders).
    pub transmitted: f64,
    /// Harmonics per direction; orders are listed p-major (kx follows p, ky follows q).
    harmonics: (usize, usize),
    normalised_k: (Vec<C>, Vec<C>),
    layers: Vec<LayerField>,
    /// Depth of the substrate's surface below the first monitored layer's top (µm): the
    /// monitored layers' total thickness when they are the last layers, else None.
    sub_depth: Option<f64>,
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

        // Layers. Monitored layers keep their modes, the S-matrix of everything above them, and
        // the S-matrices from the first of them down for the stack below each.
        let mon = self.monitor.start.min(self.layers.len())..self.monitor.end.min(self.layers.len());
        let mut global = s_ref;
        let mut modes: Vec<(std::sync::Arc<LayerModes>, SMatrix)> = vec![];
        let mut below: Vec<SMatrix> = vec![];
        for (li, layer) in self.layers.iter().enumerate() {
            let keep = mon.contains(&li);
            let (s, kept) = match &layer.eps {
                Eps::Uniform(e) => {
                    let phys = *e;
                    let e = e.conj();
                    let kz = kz_of(e);
                    let lam: Vec<C> = kz.iter().chain(&kz).map(|k| C::new(0.0, 1.0) * k).collect();
                    let v = diag_scale_cols(&q_hom(e), &lam.iter().map(|l| 1.0 / l).collect::<Vec<_>>());
                    let s = layer_s(&identity(2 * n), &v, &lam, &v0, k0, layer.thickness);
                    let kept = keep.then(|| LayerModes {
                        thickness: layer.thickness,
                        lam: lam.clone(),
                        w: identity(2 * n),
                        v,
                        ez: diag(&vec![C::new(1.0, 0.0) / e; n]),
                        loss: Loss::Uniform(phys.im.max(0.0)),
                        pattern: None,
                    });
                    (s, kept)
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
                    let s = layer_s(&w, &v, &lam, &v0, k0, layer.thickness);
                    let kept = keep.then(|| LayerModes {
                        thickness: layer.thickness,
                        lam,
                        w,
                        v,
                        ez: ei,
                        loss: Loss::Grid { nx: *nx, ny: *ny, im: samples.iter().map(|e| e.im.max(0.0)).collect() },
                        pattern: Some((exx, eyy, *nx, *ny, conj)),
                    });
                    (s, kept)
                }
            };
            if let Some(m) = kept {
                modes.push((std::sync::Arc::new(m), global.clone()));
            }
            if !mon.is_empty() && li >= mon.start {
                below.push(s.clone());
            }
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
        // For each monitored layer: (modes, S above it, S of it and below, S below it).
        let mut suffix = vec![s_trn.clone()];
        for s in below.iter().rev() {
            let next = s.star(suffix.last().unwrap());
            suffix.push(next);
        }
        suffix.reverse(); // suffix[k] = layers mon.start + k .. end, then the substrate
        let mut depth = 0.0;
        let monitored: Vec<_> = modes
            .into_iter()
            .enumerate()
            .map(|(k, (m, above))| {
                let d = depth;
                depth += m.thickness;
                let above_next = above.star(&below[k]);
                (m, d, above, suffix[k].clone(), above_next, suffix[k + 1].clone())
            })
            .collect();
        let sub_depth = (mon.end == self.layers.len() || mon.is_empty()).then_some(depth);

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
                let transmitted =
                    (0..n).map(|m| kz_trn[m].re * (t[m][0].norm_sqr() + t[m][1].norm_sqr() + t[m][2].norm_sqr())).sum::<f64>()
                        / kinc[2];
                // Gap amplitudes between a stack above (A) and below (B): down a, up b.
                let gap = |a_s: &SMatrix, b_s: &SMatrix| -> (M, M) {
                    let i2 = identity(2 * n);
                    let a = inv(&(&i2 - &a_s.s22 * &b_s.s11)) * (&a_s.s21 * &esrc);
                    let b = &b_s.s11 * &a;
                    (a, b)
                };
                let layers = monitored
                    .iter()
                    .map(|(m, d, above, here, above_next, under)| {
                        // Mode amplitudes from the gap fields: W (c+ + c-) = a + b and
                        // V (c- - c+) = V0 (b - a).
                        let coeffs = |a: &M, b: &M| -> (Vec<C>, Vec<C>) {
                            let alpha = inv(&m.w) * (a + b);
                            let beta = inv(&m.v) * (&v0 * (b - a));
                            let cp = (0..2 * n).map(|i| (alpha[(i, 0)] - beta[(i, 0)]) * 0.5).collect();
                            let cm = (0..2 * n).map(|i| (alpha[(i, 0)] + beta[(i, 0)]) * 0.5).collect();
                            (cp, cm)
                        };
                        let (a, b) = gap(above, here);
                        let (cp, _) = coeffs(&a, &b);
                        let (a, b) = gap(above_next, under);
                        let (_, cm) = coeffs(&a, &b);
                        LayerField { modes: m.clone(), depth: *d, cp, cm }
                    })
                    .collect();
                Solution {
                    transmitted,
                    harmonics: (hx, hy),
                    normalised_k: (kx.clone(), ky.clone()),
                    layers,
                    sub_depth,
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
    /// Add `weight` times the power absorbed in each voxel of `g` (fractions of the incident
    /// power). The grid spans the period; its depth counts from the top of the first monitored
    /// layer (or from the substrate's surface without monitored layers). Voxels in the
    /// monitored layers take the local loss of their material; below them, an absorbing
    /// substrate's.
    pub fn add_generation(&self, g: &crate::carriers::Grid, out: &mut [f64], weight: f64) {
        let (hx, hy) = self.harmonics;
        let n = hx * hy;
        let i = C::new(0.0, 1.0);
        let zero = C::new(0.0, 0.0);
        // Orders are p-major: kx follows p, ky follows q, so the plane-wave sum separates.
        let xph: Vec<Vec<C>> =
            (0..hx).map(|p| (0..g.nx).map(|a| (-i * self.kx[p * hy] * (a as f64 + 0.5) * g.dx).exp()).collect()).collect();
        let yph: Vec<Vec<C>> =
            (0..hy).map(|q| (0..g.ny).map(|b| (-i * self.ky[q] * (b as f64 + 0.5) * g.dx).exp()).collect()).collect();
        let vol = g.dx * g.dx * g.dz;
        let scale = weight * self.k0 * vol / (self.incident_flux * self.area);
        let (kxn, kyn) = &self.normalised_k;
        let mut amps = vec![[zero; 3]; n];
        let mut field = vec![[zero; 3]; g.nx * g.ny];
        let mut part = vec![zero; hx * g.ny];
        let mut slice = vec![0.0; g.nx * g.ny];
        for k in 0..g.nz {
            let z = (k as f64 + 0.5) * g.dz;
            let loss: &Loss;
            let sub_loss;
            let mut pattern = None;
            let mut exact = None;
            if let Some(lf) = self.layers.iter().find(|l| z >= l.depth && z < l.depth + l.modes.thickness) {
                let m = &lf.modes;
                let zl = z - lf.depth;
                let (st, ut) = self.layer_fields(lf, zl);
                if m.pattern.is_some() {
                    // The slice's absorption, exactly, from the drop of the power flux across it.
                    let za = (k as f64 * g.dz - lf.depth).max(0.0);
                    let zb = ((k + 1) as f64 * g.dz - lf.depth).min(m.thickness);
                    exact = Some(weight * (self.layer_flux(lf, za) - self.layer_flux(lf, zb)));
                }
                let curl = Mat::from_fn(n, 1, |r, _| kxn[r] * ut[(n + r, 0)] - kyn[r] * ut[(r, 0)]);
                let sz = &m.ez * &curl;
                match &m.pattern {
                    Some((exx, eyy, ..)) => {
                        let ex = Mat::from_fn(n, 1, |r, _| st[(r, 0)]);
                        let ey = Mat::from_fn(n, 1, |r, _| st[(n + r, 0)]);
                        let (dx, dy) = (exx * &ex, eyy * &ey);
                        for r in 0..n {
                            amps[r] = [dx[(r, 0)], dy[(r, 0)], sz[(r, 0)]];
                        }
                    }
                    None => {
                        for r in 0..n {
                            amps[r] = [st[(r, 0)], st[(n + r, 0)], sz[(r, 0)]];
                        }
                    }
                }
                pattern = m.pattern.as_ref().map(|(_, _, nx, ny, e)| (*nx, *ny, e));
                loss = &m.loss;
            } else if let Some(sd) = self.sub_depth
                && z >= sd
                && self.eps_sub_loss > 0.0
            {
                for r in 0..n {
                    let ph = (-i * self.kz[r] * (z - sd)).exp();
                    amps[r] = [self.t[r][0] * ph, self.t[r][1] * ph, self.t[r][2] * ph];
                }
                sub_loss = Loss::Uniform(self.eps_sub_loss);
                loss = &sub_loss;
            } else {
                continue;
            }
            field.iter_mut().for_each(|f| *f = [zero; 3]);
            for c in 0..3 {
                for p in 0..hx {
                    for b in 0..g.ny {
                        part[p * g.ny + b] = (0..hy).map(|q| yph[q][b] * amps[p * hy + q][c]).sum();
                    }
                }
                for b in 0..g.ny {
                    for a in 0..g.nx {
                        field[b * g.nx + a][c] = (0..hx).map(|p| xph[p][a] * part[p * g.ny + b]).sum();
                    }
                }
            }
            for b in 0..g.ny {
                for a in 0..g.nx {
                    let im = match loss {
                        Loss::Uniform(v) => *v,
                        Loss::Grid { nx, ny, im } => {
                            let (sa, sb) =
                                (((2 * a + 1) * nx / (2 * g.nx)).min(nx - 1), ((2 * b + 1) * ny / (2 * g.ny)).min(ny - 1));
                            im[sb * nx + sa]
                        }
                    };
                    if im == 0.0 {
                        continue;
                    }
                    let mut e = field[b * g.nx + a];
                    if let Some((nx, ny, eps)) = pattern {
                        // In-plane components came as D: E = D / ε here.
                        let (sa, sb) = (((2 * a + 1) * nx / (2 * g.nx)).min(nx - 1), ((2 * b + 1) * ny / (2 * g.ny)).min(ny - 1));
                        let ep = eps[sb * nx + sa];
                        e[0] /= ep;
                        e[1] /= ep;
                    }
                    slice[b * g.nx + a] = scale * im * (e[0].norm_sqr() + e[1].norm_sqr() + e[2].norm_sqr());
                }
            }
            // In a patterned layer the rebuilt field only distributes the slice's absorption.
            let total: f64 = slice.iter().sum();
            let f = match exact {
                Some(x) if total > 0.0 => x.max(0.0) / total,
                _ => 1.0,
            };
            for (v, s) in slice.iter_mut().enumerate() {
                out[k * g.nx * g.ny + v] += *s * f;
                *s = 0.0;
            }
        }
    }

    /// Transverse fields (sx, sy; ux, uy per order) at depth `zl` in a monitored layer.
    fn layer_fields(&self, lf: &LayerField, zl: f64) -> (M, M) {
        let m = &lf.modes;
        let n2 = m.lam.len();
        let sum = Mat::from_fn(n2, 1, |r, _| {
            (-m.lam[r] * self.k0 * zl).exp() * lf.cp[r] + (-m.lam[r] * self.k0 * (m.thickness - zl)).exp() * lf.cm[r]
        });
        let dif = Mat::from_fn(n2, 1, |r, _| {
            (-m.lam[r] * self.k0 * (m.thickness - zl)).exp() * lf.cm[r] - (-m.lam[r] * self.k0 * zl).exp() * lf.cp[r]
        });
        (&m.w * &sum, &m.v * &dif)
    }

    /// Downward power flux at depth `zl` in a monitored layer, as a fraction of the incident
    /// power (Parseval over the orders: Im(E × H̃*)z in the normalised fields).
    fn layer_flux(&self, lf: &LayerField, zl: f64) -> f64 {
        let (st, ut) = self.layer_fields(lf, zl);
        let n = st.nrows() / 2;
        let s: f64 = (0..n).map(|r| (st[(r, 0)] * ut[(n + r, 0)].conj() - st[(n + r, 0)] * ut[(r, 0)].conj()).im).sum();
        s.abs() / self.incident_flux
    }

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
                monitor: 0..0,
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
            monitor: 0..0,
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

    /// Power absorbed in the voxels of a grid over the period, `nz` voxels down to `depth`.
    fn grid_absorption(s: &Solution, period: f64, depth: f64, nxy: usize, nz: usize) -> Vec<f64> {
        let g = crate::carriers::Grid { nx: nxy, ny: nxy, nz, dx: period / nxy as f64, dz: depth / nz as f64 };
        let mut out = vec![0.0; g.len()];
        s.add_generation(&g, &mut out, 1.0);
        out
    }

    #[test]
    fn finite_absorbing_layer_matches_thin_film_optics() {
        // Back-side illuminated silicon: oxide, 1.5 µm of silicon, then an oxide substrate. The
        // power absorbed in the monitored silicon (summed over voxels) and transmitted below it
        // match thin-film optics, including the standing wave from the silicon's far side.
        let (si, ox) = (c(3.94, 0.02), c(1.46, 0.0));
        for theta in [0.0f64, 0.4] {
            let films = [(c(2.1, 0.0), 0.06), (si, 1.5)];
            let pr = Problem {
                period: (0.8, 0.8),
                harmonics: (3, 3),
                n_in: ox,
                layers: films.iter().map(|&(n, t)| uniform(n, t)).collect(),
                n_sub: ox,
                wavelength: 0.6,
                theta,
                phi: 0.2,
                monitor: 1..2,
            };
            for (k, pol) in [tmm::Pol::S, tmm::Pol::P].into_iter().enumerate() {
                let x = tmm::rt(pol, ox, &films, ox, theta, 0.6);
                let sol = &pr.solve()[k];
                let a: f64 = grid_absorption(sol, 0.8, 1.5, 4, 1500).iter().sum();
                assert!((sol.r - x.r).abs() < 1e-6, "{pol:?} θ {theta}: R {} vs {}", sol.r, x.r);
                assert!((sol.transmitted - x.t).abs() < 1e-6, "{pol:?} θ {theta}: T {} vs {}", sol.transmitted, x.t);
                assert!((a - (1.0 - x.r - x.t)).abs() < 2e-4, "{pol:?} θ {theta}: A {a} vs {}", 1.0 - x.r - x.t);
            }
        }
    }

    #[test]
    fn patterned_absorbing_layer_conserves_energy() {
        // Silicon cut by an oxide trench grid (as deep trench isolation does), over silicon:
        // reflected + absorbed in the trenched layer + absorbed below = 1.
        let (nx, ny) = (32, 32);
        let (si, ox) = (c(3.94, 0.05), c(1.46, 0.0));
        let samples: Vec<C> = (0..nx * ny)
            .map(|k| {
                let (i, j) = (k % nx, k / nx);
                if i < 3 || j < 3 { ox * ox } else { si * si }
            })
            .collect();
        let pr = Problem {
            period: (1.0, 1.0),
            harmonics: (9, 9),
            n_in: ox,
            layers: vec![uniform(c(2.0, 0.0), 0.07), Layer { thickness: 0.8, eps: Eps::Grid { nx, ny, samples } }],
            n_sub: si,
            wavelength: 0.55,
            theta: 0.15,
            phi: 0.0,
            monitor: 1..2,
        };
        for s in pr.solve() {
            let in_layer: f64 = grid_absorption(&s, 1.0, 0.8, 32, 400).iter().sum();
            let below = s.absorbed((0.0, 1.0), (0.0, 1.0), (0.0, f64::INFINITY));
            assert!((s.r + in_layer + below - 1.0).abs() < 5e-3, "R {} + A {in_layer} + {below}", s.r);
            // The substrate's share sits below the layer in the voxel grid too.
            let deep: f64 = grid_absorption(&s, 1.0, 0.8 + 40.0, 24, 4080).iter().sum();
            assert!((deep - in_layer - below).abs() < 5e-3, "{deep} vs {}", in_layer + below);
        }
    }
}
