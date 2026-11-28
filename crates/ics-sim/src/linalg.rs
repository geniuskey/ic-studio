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

/// MNA matrix: dense row-major up to [`DENSE_MAX`], else the list of every `add` (row, column,
/// value), duplicates summed when it is solved. Adding is O(1) however many devices share a node
/// (a row search per add was quadratic on high-fanout nodes such as a supply), and the sorted
/// pattern is reused while the circuit stamps the same entries in the same order, as every
/// Newton iteration and time step does.
#[derive(Clone)]
pub struct Matrix<T> {
    pub n: usize,
    dense: Vec<T>,
    sparse: bool,
    idx: Vec<(usize, usize)>,
    val: Vec<T>,
}

impl<T: Scalar> Matrix<T> {
    pub fn new(n: usize) -> Self {
        Self::with_dense_max(n, DENSE_MAX)
    }

    /// A matrix stored densely only up to `dense_max` (tests compare both solvers).
    pub fn with_dense_max(n: usize, dense_max: usize) -> Self {
        if n <= dense_max {
            Self { n, dense: vec![T::zero(); n * n], sparse: false, idx: vec![], val: vec![] }
        } else {
            Self { n, dense: vec![], sparse: true, idx: Vec::with_capacity(8 * n), val: Vec::with_capacity(8 * n) }
        }
    }

    fn is_dense(&self) -> bool {
        !self.sparse
    }

    #[inline]
    pub fn add(&mut self, r: usize, c: usize, v: T) {
        if self.is_dense() {
            let i = r * self.n + c;
            self.dense[i] = self.dense[i] + v;
            return;
        }
        self.idx.push((r, c));
        self.val.push(v);
    }

    pub fn get(&self, r: usize, c: usize) -> T {
        if self.is_dense() {
            self.dense[r * self.n + c]
        } else {
            self.idx.iter().zip(&self.val).filter(|(k, _)| **k == (r, c)).fold(T::zero(), |a, (_, &v)| a + v)
        }
    }

    /// The transpose (for adjoint solves).
    pub fn transposed(&self) -> Self {
        let mut t = self.clone();
        if self.is_dense() {
            for r in 0..self.n {
                for c in 0..self.n {
                    t.dense[c * self.n + r] = self.dense[r * self.n + c];
                }
            }
        } else {
            for k in t.idx.iter_mut() {
                *k = (k.1, k.0);
            }
        }
        t
    }
}

/// Solve `m * x = b` in place (b becomes x). Returns false if singular.
pub fn solve<T: Scalar>(m: &mut Matrix<T>, b: &mut [T]) -> bool {
    if m.is_dense() { solve_dense(m.n, &mut m.dense, b) } else { solve_sparse(m, b) }
}

/// The analysis of the last sparse pattern solved: the `add` sequence it came from, the
/// compressed structure with the order summing the values into it, and its LU analysis
/// (fill-reducing ordering, elimination tree). Only the numeric factorization runs again
/// while the pattern repeats.
struct Pattern {
    n: usize,
    idx: Vec<(usize, usize)>,
    csc: faer::sparse::SymbolicSparseColMat<usize>,
    order: faer::sparse::Argsort<usize>,
    lu: faer::sparse::linalg::solvers::SymbolicLu<usize>,
}

fn solve_sparse<T: Scalar>(m: &Matrix<T>, b: &mut [T]) -> bool {
    use faer::sparse::linalg::solvers::{Lu, SymbolicLu};
    use faer::sparse::{Pair, SparseColMat, SymbolicSparseColMat};
    use std::cell::RefCell;
    thread_local! {
        static LAST: RefCell<Option<Pattern>> = const { RefCell::new(None) };
    }
    let n = m.n;
    let lu = LAST.with(|cell| {
        let mut last = cell.borrow_mut();
        if !last.as_ref().is_some_and(|p| p.n == n && p.idx == m.idx) {
            let pairs: Vec<Pair<usize, usize>> = m.idx.iter().map(|&(row, col)| Pair { row, col }).collect();
            let (csc, order) = SymbolicSparseColMat::try_new_from_indices(n, n, &pairs).ok()?;
            let lu = SymbolicLu::try_new(csc.as_ref()).ok()?;
            *last = Some(Pattern { n, idx: m.idx.clone(), csc, order, lu });
        }
        let p = last.as_ref()?;
        let a = SparseColMat::<usize, T>::new_from_argsort(p.csc.clone(), &p.order, &m.val).ok()?;
        Lu::try_new_with_symbolic(p.lu.clone(), a.as_ref()).ok()
    });
    let Some(lu) = lu else { return false };
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
