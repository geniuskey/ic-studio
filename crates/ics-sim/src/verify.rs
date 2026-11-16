//! Run a project's verification plan ([`ics_core::verify`]): each test's sweep setup on the
//! simulator, every spec summarised over the test's points.

use std::time::Instant;

use ics_core::verify::{TestResult, VerifyRun, summarize};
use ics_core::{CellRef, Project};

use crate::SimOptions;

/// Run one test: the sweep setup of `cell`.
pub fn run_test(
    project: &Project,
    name: &str,
    cell: &CellRef,
    opts: &SimOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> TestResult {
    let started = Instant::now();
    let mut t = TestResult { name: name.to_string(), cell: cell.to_string(), ..Default::default() };
    let specs = match project.cell(cell) {
        Some(c) if !c.sim.sweep.specs.is_empty() => c.sim.sweep.specs.clone(),
        Some(_) => {
            t.error = Some(format!("{cell} has no specs in its sweep setup"));
            return t;
        }
        None => {
            t.error = Some(format!("no cell {cell}"));
            return t;
        }
    };
    match crate::sweep::run(project, cell, opts, progress) {
        Ok(run) => {
            t.points = run.points.iter().map(|(p, _)| p.label()).collect();
            t.specs = specs
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    let vals: Vec<Result<f64, String>> =
                        run.points.iter().map(|(_, v)| v.get(k).cloned().unwrap_or_else(|| Err("not measured".into()))).collect();
                    summarize(s, &vals)
                })
                .collect();
            t.notes = run.notes;
        }
        Err(e) => t.error = Some(e),
    }
    t.seconds = started.elapsed().as_secs_f64();
    t
}

/// Run `tests` one after the other; `progress(test, done, total)` reports the points of the
/// running test.
pub fn run(
    project: &Project,
    tests: &[(String, CellRef)],
    opts: &SimOptions,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> VerifyRun {
    let mut out = VerifyRun::now();
    for (name, cell) in tests {
        let p = |d: usize, n: usize| progress(name, d, n);
        out.tests.push(run_test(project, name, cell, opts, &p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::sweep::Spec;
    use ics_core::verify::Status;

    #[test]
    fn runs_tests_over_corners() {
        let mut p = ics_core::examples::demo_project();
        let r = CellRef::new("work", "rc_lowpass");
        let c = p.cell_mut(&r).unwrap();
        c.sim.backend = ics_core::Backend::Builtin;
        for a in &mut c.sim.analyses {
            a.enabled = a.analysis.kind_name() == "ac";
        }
        c.sim.sweep.vars = vec![ics_core::sweep::SweepVar { name: "R1.r".into(), values: "1k 2k 4k".into() }];
        let bw = |min: &str| Spec {
            name: format!("bw>{min}"),
            analysis: "ac".into(),
            expr: "bandwidth(v(out), 3)".into(),
            min: min.into(),
            max: "".into(),
        };
        c.sim.sweep.specs = vec![bw("1"), bw("1e12"), Spec { expr: "nosuch(".into(), ..bw("1") }];
        let tests = vec![("rc".to_string(), r.clone()), ("missing".to_string(), CellRef::new("work", "nope"))];
        let run = run(&p, &tests, &SimOptions::default(), &|_, _, _| {});
        let t = &run.tests[0];
        assert_eq!(t.points.len(), 3, "{t:?}");
        assert_eq!(t.specs[0].status(), Status::Pass, "{:?}", t.specs[0]);
        // The worst bandwidth is at the largest resistance.
        assert!(t.points[t.specs[0].worst_point.unwrap()].contains("R1.r=4k"), "{t:?}");
        assert_eq!((t.specs[1].status(), t.specs[1].failed), (Status::Fail, 3));
        assert_eq!(t.specs[2].status(), Status::Error);
        assert!(run.tests[1].error.is_some());
        assert_eq!(run.status(), Status::Fail);
        assert!(run.junit().contains("<testsuite name=\"missing\""));
    }
}
