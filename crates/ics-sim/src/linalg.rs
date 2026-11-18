//! Linear solver of the MNA systems, generic over real and complex numbers: dense LU with
//! partial pivoting for small circuits, sparse LU (faer, with fill-reducing ordering) for
//! large ones such as post-layout RC networks, where a dense matrix costs n² memory and n³ time.

// The generic arithmetic is written out: `Scalar` does not require the assign operators.
#![allow(clippy::assign_op_pattern)]

use num_complex::Complex64;
use std::ops::{Add, Div, Mul, Sub};

/// Systems up to this size are solved densely (faster for small, dense-ish MNA matrices).
pub const DENSE_MAX: usize = 120;

pub trait Scalar:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + faer::traits::ComplexField
{
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

/// MNA matrix: dense row-major up to [`DENSE_MAX`], else rows of (column, value) entries.
#[derive(Clone)]
pub struct Matrix<T> {
    pub n: usize,
    dense: Vec<T>,
    rows: Vec<Vec<(usize, T)>>,
}

impl<T: Scalar> Matrix<T> {
    pub fn new(n: usize) -> Self {
        Self::with_dense_max(n, DENSE_MAX)
    }

    /// A matrix stored densely only up to `dense_max` (tests compare both solvers).
    pub fn with_dense_max(n: usize, dense_max: usize) -> Self {
        if n <= dense_max {
            Self { n, dense: vec![T::zero(); n * n], rows: vec![] }
        } else {
            Self { n, dense: vec![], rows: vec![Vec::new(); n] }
        }
    }

    fn is_dense(&self) -> bool {
        self.rows.is_empty() && self.n > 0 || self.n == 0
    }

    #[inline]
    pub fn add(&mut self, r: usize, c: usize, v: T) {
        if self.is_dense() {
            let i = r * self.n + c;
            self.dense[i] = self.dense[i] + v;
            return;
        }
        let row = &mut self.rows[r];
        match row.iter_mut().find(|(k, _)| *k == c) {
            Some(e) => e.1 = e.1 + v,
            None => row.push((c, v)),
        }
    }

    pub fn get(&self, r: usize, c: usize) -> T {
        if self.is_dense() {
            self.dense[r * self.n + c]
        } else {
            self.rows[r].iter().find(|(k, _)| *k == c).map_or(T::zero(), |e| e.1)
        }
    }

    /// The transpose (for adjoint solves).
    pub fn transposed(&self) -> Self {
        let mut t = Self { n: self.n, dense: vec![], rows: vec![] };
        if self.is_dense() {
            t.dense = vec![T::zero(); self.n * self.n];
            for r in 0..self.n {
                for c in 0..self.n {
                    t.dense[c * self.n + r] = self.dense[r * self.n + c];
                }
            }
        } else {
            t.rows = vec![Vec::new(); self.n];
            for (r, row) in self.rows.iter().enumerate() {
                for &(c, v) in row {
                    t.rows[c].push((r, v));
                }
            }
        }
        t
    }
}

/// Solve `m * x = b` in place (b becomes x). Returns false if singular.
pub fn solve<T: Scalar>(m: &mut Matrix<T>, b: &mut [T]) -> bool {
    if m.is_dense() { solve_dense(m.n, &mut m.dense, b) } else { solve_sparse(m, b) }
}

fn solve_sparse<T: Scalar>(m: &Matrix<T>, b: &mut [T]) -> bool {
    use faer::sparse::{SparseColMat, Triplet};
    let n = m.n;
    let triplets: Vec<Triplet<usize, usize, T>> =
        m.rows.iter().enumerate().flat_map(|(r, row)| row.iter().map(move |&(c, v)| Triplet::new(r, c, v))).collect();
    let Ok(a) = SparseColMat::<usize, T>::try_new_from_triplets(n, n, &triplets) else { return false };
    let Some(lu) = factorize(&a) else { return false };
    let mut rhs = faer::Mat::<T>::from_fn(n, 1, |i, _| b[i]);
    use faer::linalg::solvers::Solve;
    lu.solve_in_place(rhs.as_mut());
    for (i, x) in b.iter_mut().enumerate() {
        let v = rhs[(i, 0)];
        if !v.magnitude().is_finite() {
            return false;
        }
        *x = v;
    }
    true
}

/// LU of `a`, reusing the symbolic analysis (fill-reducing ordering, elimination tree) of the
/// last matrix with the same sparsity pattern: Newton iterations and time steps of one circuit
/// repeat the pattern, so only the numeric factorization runs again.
fn factorize<T: Scalar>(a: &faer::sparse::SparseColMat<usize, T>) -> Option<faer::sparse::linalg::solvers::Lu<usize, T>> {
    use faer::sparse::linalg::solvers::{Lu, SymbolicLu};
    use std::cell::RefCell;
    /// Column pointers and row indices of the pattern, and its analysis.
    type Cached = Option<(Vec<usize>, Vec<usize>, SymbolicLu<usize>)>;
    thread_local! {
        static LAST: RefCell<Cached> = const { RefCell::new(None) };
    }
    let sym = a.symbolic();
    let (ptr, idx) = (sym.col_ptr(), sym.row_idx());
    let cached = LAST
        .with(|c| c.borrow().as_ref().filter(|(p, i, _)| p.as_slice() == ptr && i.as_slice() == idx).map(|(_, _, s)| s.clone()));
    let symbolic = match cached {
        Some(s) => s,
        None => {
            let s = SymbolicLu::try_new(sym).ok()?;
            LAST.with(|c| *c.borrow_mut() = Some((ptr.to_vec(), idx.to_vec(), s.clone())));
            s
        }
    };
    Lu::try_new_with_symbolic(symbolic, a.as_ref()).ok()
}

fn solve_dense<T: Scalar>(n: usize, a: &mut [T], b: &mut [T]) -> bool {
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

    #[test]
    fn sparse_matches_dense() {
        // A ladder with a zero diagonal (a voltage-source row), real and complex.
        let n = 40;
        let fill = |m: &mut Matrix<f64>| {
            for i in 0..n - 1 {
                m.add(i, i, 2.0 + i as f64 * 0.1);
                m.add(i, i + 1, -1.0);
                m.add(i + 1, i, -1.0);
            }
            m.add(0, n - 1, 1.0);
            m.add(n - 1, 0, 1.0);
        };
        let (mut d, mut s) = (Matrix::with_dense_max(n, n), Matrix::with_dense_max(n, 0));
        fill(&mut d);
        fill(&mut s);
        let b: Vec<f64> = (0..n).map(|i| (i as f64).sin()).collect();
        let (mut xd, mut xs) = (b.clone(), b.clone());
        assert!(solve(&mut d, &mut xd) && solve(&mut s, &mut xs));
        for (a, c) in xd.iter().zip(&xs) {
            assert!((a - c).abs() < 1e-9, "{a} vs {c}");
        }
        let mut z = Matrix::<Complex64>::with_dense_max(3, 0);
        z.add(0, 0, Complex64::new(1.0, 1.0));
        z.add(1, 1, Complex64::new(0.0, 2.0));
        z.add(2, 2, Complex64::new(3.0, 0.0));
        let mut zb = vec![Complex64::new(1.0, 1.0), Complex64::new(0.0, 2.0), Complex64::new(3.0, 0.0)];
        assert!(solve(&mut z, &mut zb));
        assert!(zb.iter().all(|v| (v - Complex64::new(1.0, 0.0)).norm() < 1e-12));
        // Singular: an empty row.
        let mut e = Matrix::<f64>::with_dense_max(3, 0);
        e.add(0, 0, 1.0);
        e.add(1, 1, 1.0);
        assert!(!solve(&mut e, &mut [1.0, 1.0, 1.0]));
        let t = s.transposed();
        assert_eq!(t.get(0, n - 1), s.get(n - 1, 0));
    }
}
