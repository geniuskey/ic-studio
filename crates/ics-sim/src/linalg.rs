//! Small dense LU solver with partial pivoting, generic over real and complex numbers.

use num_complex::Complex64;
use std::ops::{Add, Div, Mul, Sub};

pub trait Scalar: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> {
    fn zero() -> Self;
    fn magnitude(self) -> f64;
}

impl Scalar for f64 {
    fn zero() -> Self {
        0.0
    }
    fn magnitude(self) -> f64 {
        self.abs()
    }
}

impl Scalar for Complex64 {
    fn zero() -> Self {
        Complex64::new(0.0, 0.0)
    }
    fn magnitude(self) -> f64 {
        self.norm()
    }
}

/// Dense row-major matrix.
#[derive(Clone)]
pub struct Matrix<T> {
    pub n: usize,
    pub a: Vec<T>,
}

impl<T: Scalar> Matrix<T> {
    pub fn new(n: usize) -> Self {
        Self { n, a: vec![T::zero(); n * n] }
    }
    #[inline]
    pub fn add(&mut self, r: usize, c: usize, v: T) {
        let i = r * self.n + c;
        self.a[i] = self.a[i] + v;
    }
}

/// Solve `m * x = b` in place (b becomes x). Returns false if singular.
pub fn solve<T: Scalar>(m: &mut Matrix<T>, b: &mut [T]) -> bool {
    let n = m.n;
    let a = &mut m.a;
    for k in 0..n {
        // pivot
        let mut p = k;
        let mut best = a[k * n + k].magnitude();
        for r in (k + 1)..n {
            let v = a[r * n + k].magnitude();
            if v > best {
                best = v;
                p = r;
            }
        }
        if best < 1e-300 {
            return false;
        }
        if p != k {
            for c in 0..n {
                a.swap(k * n + c, p * n + c);
            }
            b.swap(k, p);
        }
        let pivot = a[k * n + k];
        for r in (k + 1)..n {
            let f = a[r * n + k] / pivot;
            if f.magnitude() == 0.0 {
                continue;
            }
            for c in k..n {
                let v = a[k * n + c];
                a[r * n + c] = a[r * n + c] - f * v;
            }
            b[r] = b[r] - f * b[k];
        }
    }
    for k in (0..n).rev() {
        let mut s = b[k];
        for c in (k + 1)..n {
            s = s - a[k * n + c] * b[c];
        }
        b[k] = s / a[k * n + k];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_2x2() {
        let mut m = Matrix::<f64>::new(2);
        m.add(0, 0, 2.0);
        m.add(0, 1, 1.0);
        m.add(1, 0, 1.0);
        m.add(1, 1, 3.0);
        let mut b = vec![3.0, 5.0];
        assert!(solve(&mut m, &mut b));
        assert!((b[0] - 0.8).abs() < 1e-12 && (b[1] - 1.4).abs() < 1e-12);
    }
}
