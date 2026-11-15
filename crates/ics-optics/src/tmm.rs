//! Coherent thin-film optics (transfer matrices) for a planar stack between a semi-infinite
//! incidence medium and a semi-infinite substrate, at oblique incidence. Indices are `n + ik`
//! (fields `exp(i(kz - ωt))`), lengths and wavelength in µm.

use num_complex::Complex64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pol {
    S,
    P,
}

/// Reflected and transmitted power fractions; what remains is absorbed in the films.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rt {
    pub r: f64,
    pub t: f64,
}

/// `cos θ` in a medium of index `n` for the invariant `n0 sin θ0`, on the forward branch
/// (decaying or forward-propagating).
fn cos_in(n: Complex64, n_sin: Complex64) -> Complex64 {
    let s = n_sin / n;
    let c = (Complex64::new(1.0, 0.0) - s * s).sqrt();
    let forward = n * c;
    if forward.im < -1e-12 || (forward.im.abs() <= 1e-12 && forward.re < 0.0) { -c } else { c }
}

fn interface(pol: Pol, ni: Complex64, nf: Complex64, ci: Complex64, cf: Complex64) -> (Complex64, Complex64) {
    match pol {
        Pol::S => {
            let d = ni * ci + nf * cf;
            ((ni * ci - nf * cf) / d, 2.0 * ni * ci / d)
        }
        Pol::P => {
            let d = nf * ci + ni * cf;
            ((nf * ci - ni * cf) / d, 2.0 * ni * ci / d)
        }
    }
}

/// Power reflection and transmission of `layers` (index, thickness µm), top to bottom, between
/// `n_in` and the substrate `n_sub`, for light at `theta` (radians, in the incidence medium).
pub fn rt(pol: Pol, n_in: Complex64, layers: &[(Complex64, f64)], n_sub: Complex64, theta: f64, lambda: f64) -> Rt {
    let n_sin = n_in * theta.sin();
    let mut ns = vec![n_in];
    ns.extend(layers.iter().map(|l| l.0));
    ns.push(n_sub);
    let cs: Vec<Complex64> = ns.iter().map(|&n| cos_in(n, n_sin)).collect();
    let one = Complex64::new(1.0, 0.0);
    let zero = Complex64::new(0.0, 0.0);
    // M = (1/t01) [[1, r01], [r01, 1]] Π_j P_j (1/t_j,j+1) [[1, r], [r, 1]]
    let (r01, t01) = interface(pol, ns[0], ns[1], cs[0], cs[1]);
    let mut m = [[one / t01, r01 / t01], [r01 / t01, one / t01]];
    for j in 1..ns.len() - 1 {
        let delta = 2.0 * std::f64::consts::PI * ns[j] * cs[j] * layers[j - 1].1 / lambda;
        let i = Complex64::new(0.0, 1.0);
        let (em, ep) = ((-i * delta).exp(), (i * delta).exp());
        let (r, t) = interface(pol, ns[j], ns[j + 1], cs[j], cs[j + 1]);
        let l = [[em / t, em * r / t], [ep * r / t, ep / t]];
        let mut out = [[zero; 2]; 2];
        for a in 0..2 {
            for b in 0..2 {
                out[a][b] = m[a][0] * l[0][b] + m[a][1] * l[1][b];
            }
        }
        m = out;
    }
    let r = m[1][0] / m[0][0];
    let t = one / m[0][0];
    let (n0, c0) = (ns[0], cs[0]);
    let (nn, cn) = (ns[ns.len() - 1], cs[cs.len() - 1]);
    let tp = match pol {
        Pol::S => (nn * cn).re / (n0 * c0).re,
        Pol::P => (nn * cn.conj()).re / (n0 * c0.conj()).re,
    } * t.norm_sqr();
    Rt { r: r.norm_sqr(), t: tp }
}

/// Unpolarised reflection and transmission (mean of s and p).
pub fn rt_unpolarised(n_in: Complex64, layers: &[(Complex64, f64)], n_sub: Complex64, theta: f64, lambda: f64) -> Rt {
    let s = rt(Pol::S, n_in, layers, n_sub, theta, lambda);
    let p = rt(Pol::P, n_in, layers, n_sub, theta, lambda);
    Rt { r: 0.5 * (s.r + p.r), t: 0.5 * (s.t + p.t) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: f64) -> Complex64 {
        Complex64::new(n, 0.0)
    }

    #[test]
    fn single_interface_is_fresnel() {
        // Air to glass at normal incidence: R = ((n-1)/(n+1))².
        let x = rt(Pol::S, c(1.0), &[], c(1.5), 0.0, 0.55);
        assert!((x.r - 0.04).abs() < 1e-12 && (x.r + x.t - 1.0).abs() < 1e-12);
        // Brewster angle: p reflection vanishes.
        let b = 1.5f64.atan();
        assert!(rt(Pol::P, c(1.0), &[], c(1.5), b, 0.55).r < 1e-12);
        // Total internal reflection from glass to air beyond the critical angle.
        let tir = rt(Pol::S, c(1.5), &[], c(1.0), 0.8, 0.55);
        assert!((tir.r - 1.0).abs() < 1e-9 && tir.t.abs() < 1e-9, "{tir:?}");
    }

    #[test]
    fn quarter_wave_coating_cancels_reflection() {
        // n_c = sqrt(n_s) at λ/4n thickness.
        let ns: f64 = 2.25;
        let nc = ns.sqrt();
        let d = 0.55 / (4.0 * nc);
        let x = rt(Pol::S, c(1.0), &[(c(nc), d)], c(ns), 0.0, 0.55);
        assert!(x.r < 1e-12 && (x.t - 1.0).abs() < 1e-12, "{x:?}");
        // A half-wave layer is absent at its design wavelength.
        let half = rt(Pol::P, c(1.0), &[(c(1.8), 0.55 / 3.6)], c(1.5), 0.0, 0.55);
        assert!((half.r - 0.04).abs() < 1e-12);
    }

    #[test]
    fn energy_is_conserved_and_absorbers_absorb() {
        let si = Complex64::new(4.08, 0.03);
        for th in [0.0, 0.3, 0.7] {
            for pol in [Pol::S, Pol::P] {
                let lossless = rt(pol, c(1.46), &[(c(2.1), 0.05), (c(1.9), 0.01)], si, th, 0.55);
                assert!((lossless.r + lossless.t - 1.0).abs() < 1e-9, "{pol:?} {th}: {lossless:?}");
                let lossy = rt(pol, c(1.46), &[(Complex64::new(2.0, 0.3), 0.05)], si, th, 0.55);
                assert!(lossy.r + lossy.t < 0.999);
            }
        }
        // An oxide-to-silicon interface reflects about 19 % at normal incidence.
        let x = rt_unpolarised(c(1.46), &[], si, 0.0, 0.55);
        assert!((x.r - ((4.08f64 - 1.46) / (4.08 + 1.46)).powi(2)).abs() < 1e-3, "{x:?}");
    }
}
