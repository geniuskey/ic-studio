//! Correlation of two simulation results, typically the built-in engine against ngspice:
//! every node voltage and branch current both backends report is compared point by point
//! (the test result interpolated onto the reference scale) and summarized as its worst error.

use ics_core::Analysis;
use ics_core::units::format_eng;

use crate::results::{Dataset, SimResults, VecKind, Vector};
use crate::{SimError, SimOptions, simulate};

/// The worst disagreement of one vector.
#[derive(Debug, Clone, PartialEq)]
pub struct VecDiff {
    /// Analysis kind ("op", "dc", "ac", "tran").
    pub analysis: String,
    pub name: String,
    pub kind: VecKind,
    /// Largest absolute error (complex magnitude for AC), in the vector's unit.
    pub max_abs: f64,
    /// `max_abs` relative to the largest magnitude of the reference vector.
    pub rel: f64,
    /// Scale value (time, frequency, sweep value) of the largest error.
    pub at: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Correlation {
    pub reference: String,
    pub test: String,
    pub diffs: Vec<VecDiff>,
    /// Reference vectors (per analysis) the test result does not report.
    pub missing: Vec<String>,
}

impl Correlation {
    pub fn worst(&self) -> Option<&VecDiff> {
        self.diffs.iter().max_by(|a, b| a.rel.total_cmp(&b.rel))
    }

    /// True when every compared vector is within `tol` relative error.
    pub fn passes(&self, tol: f64) -> bool {
        self.diffs.iter().all(|d| d.rel <= tol)
    }

    pub fn table(&self) -> String {
        let mut s = format!("correlation: {} (test) vs {} (reference)\n", self.test, self.reference);
        let mut diffs: Vec<&VecDiff> = self.diffs.iter().collect();
        diffs.sort_by(|a, b| b.rel.total_cmp(&a.rel));
        for d in diffs {
            let at = d.at.map(|a| format!(" at {}", format_eng(a, scale_unit(&d.analysis)))).unwrap_or_default();
            s += &format!(
                "  {:<5} {:<18} {:>8.3} %  max |err| {}{at}\n",
                d.analysis,
                d.name,
                d.rel * 100.0,
                format_eng(d.max_abs, d.kind.unit())
            );
        }
        if !self.missing.is_empty() {
            s += &format!("  not reported by {}: {}\n", self.test, self.missing.join(", "));
        }
        s
    }
}

fn scale_unit(analysis: &str) -> &'static str {
    match analysis {
        "tran" => "s",
        "ac" => "Hz",
        _ => "",
    }
}

/// Magnitude below which a reference vector counts as zero (relative errors use this floor).
fn floor(kind: VecKind) -> f64 {
    match kind {
        VecKind::Current => 1e-9,
        _ => 1e-6,
    }
}

/// Only node voltages and branch currents are compared; backend-internal vectors (ngspice's
/// `m1#dbody` and the like) are skipped.
fn comparable(v: &Vector) -> bool {
    matches!(v.kind, VecKind::Voltage | VecKind::Current) && !v.name.contains('#')
}

/// Linear interpolation of `ys` over the increasing `xs` at `x` (clamped at the ends).
fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let k = xs.partition_point(|&p| p < x);
    if k == 0 {
        return ys[0];
    }
    if k >= xs.len() {
        return ys[xs.len() - 1];
    }
    let (x0, x1) = (xs[k - 1], xs[k]);
    let f = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
    ys[k - 1] + f * (ys[k] - ys[k - 1])
}

/// Pointwise error of `t` against `r`. In a transient the test may also be read up to half a
/// reference step earlier or later: a current jumping at a waveform corner then counts as the
/// jump both backends resolve, not as the difference of which side of it each sampled.
fn diff_vectors(analysis: &str, r: &Vector, rs: Option<&[f64]>, t: &Vector, ts: Option<&[f64]>) -> Option<VecDiff> {
    let scaled = match ts {
        Some(s) if s.len() == t.re.len() && s.len() > 1 => Some(monotonic(s, &t.re, t.im.as_deref())),
        _ => None,
    };
    let at = |x: Option<f64>, i: usize| -> (f64, f64) {
        match (&scaled, x) {
            (Some((s, re, im)), Some(x)) => (interp(s, re, x), im.as_ref().map_or(0.0, |im| interp(s, im, x))),
            _ => (t.re.get(i).copied().unwrap_or(f64::NAN), t.im.as_ref().and_then(|im| im.get(i).copied()).unwrap_or(0.0)),
        }
    };
    let window = |i: usize| -> f64 {
        match rs {
            Some(s) if analysis == "tran" && s.len() > 1 => {
                let prev = if i > 0 { s[i] - s[i - 1] } else { f64::INFINITY };
                let next = if i + 1 < s.len() { s[i + 1] - s[i] } else { f64::INFINITY };
                0.5 * prev.min(next)
            }
            _ => 0.0,
        }
    };
    let mut max_abs = 0.0f64;
    let mut peak = 0.0f64;
    let mut worst_at = None;
    for i in 0..r.re.len() {
        let x = rs.map(|s| s[i]);
        let (rr, ri) = (r.re[i], r.im.as_ref().map_or(0.0, |im| im[i]));
        let dx = window(i);
        let e = [0.0, -dx, dx]
            .into_iter()
            .map(|d| {
                let (tr, ti) = at(x.map(|x| x + d), i);
                (tr - rr).hypot(ti - ri)
            })
            .fold(f64::INFINITY, f64::min);
        if !e.is_finite() {
            return None;
        }
        peak = peak.max(rr.hypot(ri));
        if e > max_abs {
            max_abs = e;
            worst_at = x;
        }
    }
    Some(VecDiff {
        analysis: analysis.into(),
        name: r.name.clone(),
        kind: r.kind,
        max_abs,
        rel: max_abs / peak.max(floor(r.kind)),
        at: worst_at,
    })
}

/// A reversed sweep (`.dc V1 1.8 0 -0.1`) is flipped so interpolation sees increasing x.
fn monotonic(s: &[f64], re: &[f64], im: Option<&[f64]>) -> (Vec<f64>, Vec<f64>, Option<Vec<f64>>) {
    if s.len() > 1 && s[0] > s[s.len() - 1] {
        let rev = |v: &[f64]| v.iter().rev().copied().collect::<Vec<_>>();
        (rev(s), rev(re), im.map(rev))
    } else {
        (s.to_vec(), re.to_vec(), im.map(<[f64]>::to_vec))
    }
}

fn compare_datasets(r: &Dataset, t: &Dataset, out: &mut Correlation) {
    let rs = r.scale_vector().map(|v| v.re.as_slice());
    let ts = t.scale_vector().map(|v| v.re.as_slice());
    for rv in r.signals().filter(|v| comparable(v)) {
        match t.get(&rv.name) {
            Some(tv) => {
                if let Some(d) = diff_vectors(&r.analysis, rv, rs, tv, ts) {
                    out.diffs.push(d);
                }
            }
            None => out.missing.push(format!("{}:{}", r.analysis, rv.name)),
        }
    }
}

/// Compare `test` against `reference`: the n-th dataset of each analysis kind in one is
/// paired with the n-th of that kind in the other.
pub fn compare(reference: &SimResults, test: &SimResults) -> Correlation {
    let mut out = Correlation { reference: reference.backend.clone(), test: test.backend.clone(), ..Default::default() };
    let mut used = vec![false; test.datasets.len()];
    for r in &reference.datasets {
        let Some(k) = (0..test.datasets.len()).find(|&k| !used[k] && test.datasets[k].analysis == r.analysis) else {
            out.missing.push(format!("{} analysis", r.analysis));
            continue;
        };
        used[k] = true;
        compare_datasets(r, &test.datasets[k], &mut out);
    }
    out
}

/// Run `deck` on ngspice (the reference) and on `opts.backend`'s counterpart, the built-in
/// engine, and compare them.
pub fn correlate(
    deck: &str,
    analyses: &[Analysis],
    opts: &SimOptions,
) -> Result<(SimResults, SimResults, Correlation), SimError> {
    let reference = simulate(deck, analyses, &SimOptions { backend: ics_core::Backend::Ngspice, ..opts.clone() })?;
    let test = simulate(deck, analyses, &SimOptions { backend: ics_core::Backend::Builtin, ..opts.clone() })?;
    let c = compare(&reference, &test);
    Ok((reference, test, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tran(backend: &str, t: Vec<f64>, v: Vec<f64>) -> SimResults {
        SimResults {
            backend: backend.into(),
            datasets: vec![Dataset {
                analysis: "tran".into(),
                title: String::new(),
                scale: Some(0),
                vectors: vec![Vector::real("time", VecKind::Time, t), Vector::real("v(out)", VecKind::Voltage, v)],
                devices: vec![],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn interpolates_onto_the_reference_scale() {
        let r = tran("ngspice", vec![0.0, 1.0, 2.0], vec![0.0, 1.0, 2.0]);
        let t = tran("built-in", vec![0.0, 0.5, 2.0], vec![0.0, 0.5, 2.1]);
        let c = compare(&r, &t);
        assert_eq!(c.diffs.len(), 1);
        let d = &c.diffs[0];
        // At t=1 the test interpolates to 0.5 + 0.5 * 1.6/1.5 ≈ 1.033; at t=2 it is off by 0.1.
        assert!((d.max_abs - 0.1).abs() < 1e-12, "{d:?}");
        assert_eq!(d.at, Some(2.0));
        assert!((d.rel - 0.05).abs() < 1e-12);
        assert!(c.passes(0.06) && !c.passes(0.04));
    }

    #[test]
    fn reports_missing_vectors_and_skips_internal_ones() {
        let mut r = tran("ngspice", vec![0.0, 1.0], vec![1.0, 1.0]);
        r.datasets[0].vectors.push(Vector::real("v(m1#dbody)", VecKind::Voltage, vec![0.0, 0.0]));
        r.datasets[0].vectors.push(Vector::real("v(x)", VecKind::Voltage, vec![0.0, 0.0]));
        let t = tran("built-in", vec![0.0, 1.0], vec![1.0, 1.0]);
        let c = compare(&r, &t);
        assert_eq!(c.missing, vec!["tran:v(x)".to_string()]);
        assert_eq!(c.worst().unwrap().rel, 0.0);
        assert!(c.table().contains("v(out)"));
    }

    #[test]
    fn complex_error_includes_phase() {
        let ac = |im: f64| SimResults {
            datasets: vec![Dataset {
                analysis: "ac".into(),
                title: String::new(),
                scale: Some(0),
                vectors: vec![
                    Vector::real("frequency", VecKind::Frequency, vec![1.0]),
                    Vector { name: "v(out)".into(), kind: VecKind::Voltage, re: vec![1.0], im: Some(vec![im]) },
                ],
                devices: vec![],
            }],
            ..Default::default()
        };
        let c = compare(&ac(0.0), &ac(0.1));
        assert!((c.diffs[0].rel - 0.1).abs() < 1e-12);
    }
}
