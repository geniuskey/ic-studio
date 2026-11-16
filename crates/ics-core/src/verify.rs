//! Verification plans (ADE Assembler style): a project's tests are cells whose corner / sweep /
//! Monte Carlo setup (`sim.sweep`) is run together; each spec is summarised over all the
//! points of its test (worst value, margin to the nearest limit, pass count). Runs are kept
//! as history (`results/verify/*.json` in the project) so a run can be compared with an
//! earlier one, and written as JUnit XML for continuous integration.
//!
//! Running the tests needs a simulator and lives in `ics_sim::verify`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::{CellRef, Project};
use crate::sweep::Spec;
use crate::units::{format_eng, parse_spice};

/// The tests of a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VerifyPlan {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<TestDef>,
}

impl VerifyPlan {
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }

    /// The tests to run: the plan's enabled tests whose cell exists, or for an empty plan
    /// every cell whose sweep setup has specs (named after the cell).
    pub fn resolve(&self, p: &Project) -> Vec<(String, CellRef)> {
        if self.tests.is_empty() {
            return default_tests(p);
        }
        self.tests.iter().filter(|t| t.enabled).filter_map(|t| find_cell(p, &t.cell).map(|r| (t.display_name(), r))).collect()
    }
}

/// Every cell with a schematic and specs, as a test named after the cell.
pub fn default_tests(p: &Project) -> Vec<(String, CellRef)> {
    p.all_cells()
        .filter(|(_, c)| c.schematic.is_some() && !c.sim.sweep.specs.is_empty())
        .map(|(r, _)| (r.cell.clone(), r))
        .collect()
}

/// `lib/cell` or a cell name unique in the project.
pub fn find_cell(p: &Project, name: &str) -> Option<CellRef> {
    match name.split_once('/') {
        Some((l, c)) => {
            let r = CellRef::new(l, c);
            p.cell(&r).is_some().then_some(r)
        }
        None => {
            let mut it = p.all_cells().filter(|(r, _)| r.cell == name).map(|(r, _)| r);
            let first = it.next()?;
            it.next().is_none().then_some(first)
        }
    }
}

fn yes() -> bool {
    true
}

/// One test: the sweep setup of a testbench cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestDef {
    /// Name shown in results (default: the cell name).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `lib/cell` (or a unique cell name).
    pub cell: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl TestDef {
    pub fn new(cell: &str) -> Self {
        TestDef { name: String::new(), cell: cell.to_string(), enabled: true }
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            self.cell.rsplit('/').next().unwrap_or(&self.cell).to_string()
        } else {
            self.name.trim().to_string()
        }
    }
}

/// The outcome of a spec or test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Measured, no limits.
    Info,
    Pass,
    /// Some points could not be measured (and none failed).
    Error,
    Fail,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Info => "info",
            Status::Pass => "pass",
            Status::Error => "error",
            Status::Fail => "FAIL",
        }
    }
}

/// A spec over every point of its test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SpecResult {
    pub name: String,
    pub analysis: String,
    pub expr: String,
    pub min: String,
    pub max: String,
    /// Value at each point of the test (`None`: not measured).
    pub values: Vec<Option<f64>>,
    /// Points that could not be measured, with the first error.
    pub errors: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub first_error: String,
    pub passed: usize,
    pub failed: usize,
    /// The value with the least margin (with limits), and its point.
    pub worst: Option<f64>,
    pub worst_point: Option<usize>,
    /// Distance of the worst value to its nearest limit (negative: outside).
    pub margin: Option<f64>,
    /// Smallest and largest value.
    pub range: Option<(f64, f64)>,
    pub status: Option<Status>,
}

impl SpecResult {
    pub fn status(&self) -> Status {
        self.status.unwrap_or(Status::Error)
    }

    /// The limits as text (`>= 60`, `<= 1m`, `1 .. 2`, `-`).
    pub fn limits(&self) -> String {
        match (self.min.trim(), self.max.trim()) {
            ("", "") => "-".into(),
            (lo, "") => format!(">= {lo}"),
            ("", hi) => format!("<= {hi}"),
            (lo, hi) => format!("{lo} .. {hi}"),
        }
    }
}

/// Summarise one spec from its value at each point.
pub fn summarize(spec: &Spec, values: &[Result<f64, String>]) -> SpecResult {
    let (lo, hi) = (parse_spice(&spec.min), parse_spice(&spec.max));
    let mut r = SpecResult {
        name: spec.name.clone(),
        analysis: spec.analysis.clone(),
        expr: spec.expr.clone(),
        min: spec.min.clone(),
        max: spec.max.clone(),
        ..Default::default()
    };
    let margin = |v: f64| -> Option<f64> {
        match (lo, hi) {
            (None, None) => None,
            (Some(l), None) => Some(v - l),
            (None, Some(h)) => Some(h - v),
            (Some(l), Some(h)) => Some((v - l).min(h - v)),
        }
    };
    for (i, v) in values.iter().enumerate() {
        match v {
            Ok(x) if x.is_finite() => {
                r.values.push(Some(*x));
                r.range = Some(r.range.map_or((*x, *x), |(a, b)| (a.min(*x), b.max(*x))));
                match spec.check(*x) {
                    Some(true) => r.passed += 1,
                    Some(false) => r.failed += 1,
                    None => {}
                }
                if let Some(m) = margin(*x)
                    && r.margin.is_none_or(|w| m < w)
                {
                    r.margin = Some(m);
                    r.worst = Some(*x);
                    r.worst_point = Some(i);
                }
            }
            Ok(_) => {
                r.values.push(None);
                r.failed += (lo.is_some() || hi.is_some()) as usize;
                r.errors += 1;
                if r.first_error.is_empty() {
                    r.first_error = "not a finite number".into();
                }
            }
            Err(e) => {
                r.values.push(None);
                r.errors += 1;
                if r.first_error.is_empty() {
                    r.first_error = e.clone();
                }
            }
        }
    }
    r.status = Some(if r.failed > 0 {
        Status::Fail
    } else if r.errors > 0 || r.range.is_none() {
        Status::Error
    } else if lo.is_none() && hi.is_none() {
        Status::Info
    } else {
        Status::Pass
    });
    r
}

/// One test of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TestResult {
    pub name: String,
    pub cell: String,
    /// Label of every point (corner, temperature, swept values, Monte Carlo run).
    pub points: Vec<String>,
    pub specs: Vec<SpecResult>,
    /// The test could not run at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub seconds: f64,
}

impl TestResult {
    pub fn status(&self) -> Status {
        if self.error.is_some() {
            return Status::Error;
        }
        self.specs.iter().map(SpecResult::status).max().unwrap_or(Status::Error).max(Status::Pass)
    }
}

/// A verification run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VerifyRun {
    /// UTC time the run started (`2026-11-16T09:30:00Z`).
    pub time: String,
    pub unix: u64,
    pub tests: Vec<TestResult>,
}

/// Counts of a run by spec status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub pass: usize,
    pub fail: usize,
    pub error: usize,
    pub info: usize,
}

impl VerifyRun {
    /// A run starting now.
    pub fn now() -> Self {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        VerifyRun { time: utc(unix), unix, tests: vec![] }
    }

    pub fn status(&self) -> Status {
        self.tests.iter().map(TestResult::status).max().unwrap_or(Status::Error)
    }

    /// Spec counts; a test that did not run counts as one error.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for t in &self.tests {
            if t.error.is_some() {
                c.error += 1;
                continue;
            }
            for s in &t.specs {
                match s.status() {
                    Status::Pass => c.pass += 1,
                    Status::Fail => c.fail += 1,
                    Status::Error => c.error += 1,
                    Status::Info => c.info += 1,
                }
            }
        }
        c
    }

    /// JUnit XML: a test suite per test, a test case per spec.
    pub fn junit(&self) -> String {
        let c = self.counts();
        let total = c.pass + c.fail + c.error + c.info;
        let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        x += &format!(
            "<testsuites name=\"ic-studio verify\" tests=\"{total}\" failures=\"{}\" errors=\"{}\" timestamp=\"{}\">\n",
            c.fail,
            c.error,
            xml(&self.time)
        );
        for t in &self.tests {
            let n = |st: Status| t.specs.iter().filter(|s| s.status() == st).count();
            x += &format!(
                "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{:.3}\">\n",
                xml(&t.name),
                t.specs.len().max(t.error.is_some() as usize),
                n(Status::Fail),
                n(Status::Error) + t.error.is_some() as usize,
                t.seconds
            );
            if let Some(e) = &t.error {
                x += &format!(
                    "    <testcase classname=\"{}\" name=\"(setup)\"><error message=\"{}\"/></testcase>\n",
                    xml(&t.name),
                    xml(e)
                );
            }
            for s in &t.specs {
                x += &format!("    <testcase classname=\"{}\" name=\"{}\">", xml(&t.name), xml(&s.name));
                let at = |k: Option<usize>| k.and_then(|k| t.points.get(k)).map_or(String::new(), |p| format!(" at {p}"));
                match s.status() {
                    Status::Fail => {
                        x += &format!(
                            "<failure message=\"{}\">{} of {} point(s) outside {}; worst {}{}</failure>",
                            xml(&format!("{} {} (limits {})", s.name, fmt(s.worst), s.limits())),
                            s.failed,
                            s.values.len(),
                            xml(&s.limits()),
                            fmt(s.worst),
                            xml(&at(s.worst_point))
                        )
                    }
                    Status::Error => {
                        x += &format!("<error message=\"{}\"/>", xml(&format!("{} point(s): {}", s.errors, s.first_error)))
                    }
                    Status::Pass | Status::Info => {
                        if let Some((a, b)) = s.range {
                            x += &format!("<system-out>{} .. {}</system-out>", fmt(Some(a)), fmt(Some(b)));
                        }
                    }
                }
                x += "</testcase>\n";
            }
            x += "  </testsuite>\n";
        }
        x += "</testsuites>\n";
        x
    }

    /// Specs whose status or worst value changed since `before`, plus specs new in this run.
    pub fn compare(&self, before: &VerifyRun) -> Vec<Change> {
        let mut out = vec![];
        for t in &self.tests {
            let bt = before.tests.iter().find(|b| b.name == t.name);
            for s in &t.specs {
                let bs = bt.and_then(|b| b.specs.iter().find(|x| x.name == s.name));
                let ch = Change {
                    test: t.name.clone(),
                    spec: s.name.clone(),
                    before: bs.map(SpecResult::status),
                    after: s.status(),
                    worst_before: bs.and_then(|b| b.worst.or(b.range.map(|r| r.0))),
                    worst_after: s.worst.or(s.range.map(|r| r.0)),
                };
                let moved = match (ch.worst_before, ch.worst_after) {
                    (Some(a), Some(b)) => (a - b).abs() > 1e-9 * a.abs().max(b.abs()).max(1e-30),
                    (a, b) => a.is_some() != b.is_some(),
                };
                if ch.before != Some(ch.after) || moved {
                    out.push(ch);
                }
            }
        }
        out
    }
}

/// A spec that changed between two runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub test: String,
    pub spec: String,
    /// `None`: new in this run.
    pub before: Option<Status>,
    pub after: Status,
    pub worst_before: Option<f64>,
    pub worst_after: Option<f64>,
}

impl Change {
    /// The spec passed (or did not exist) before and fails now.
    pub fn is_regression(&self) -> bool {
        self.after == Status::Fail && self.before != Some(Status::Fail)
    }
}

/// A number in engineering notation, `-` for none.
pub fn fmt(v: Option<f64>) -> String {
    v.map_or("-".into(), |x| format_eng(x, ""))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// UTC time of a Unix timestamp, `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let s = unix % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}

/// The history directory of a project.
pub fn history_dir(root: &Path) -> PathBuf {
    root.join("results").join("verify")
}

/// Save a run in the project's history; returns the file.
pub fn save_history(root: &Path, run: &VerifyRun) -> Result<PathBuf, String> {
    let dir = history_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stem: String = run.time.chars().filter(|c| c.is_ascii_digit()).collect();
    let stem = if stem.len() >= 14 { format!("{}-{}", &stem[..8], &stem[8..14]) } else { run.unix.to_string() };
    let mut path = dir.join(format!("{stem}.json"));
    let mut k = 1;
    while path.exists() {
        path = dir.join(format!("{stem}-{k}.json"));
        k += 1;
    }
    let text = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The project's saved runs, oldest first.
pub fn list_history(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(history_dir(root))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

pub fn load_run(path: &Path) -> Result<VerifyRun, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(min: &str, max: &str) -> Spec {
        Spec { name: "gain".into(), analysis: "ac".into(), expr: "x".into(), min: min.into(), max: max.into() }
    }

    #[test]
    fn worst_case_and_status() {
        let v = vec![Ok(62.0), Ok(58.0), Ok(70.0), Err("no ac results".to_string())];
        let s = summarize(&spec("60", ""), &v);
        assert_eq!((s.passed, s.failed, s.errors), (2, 1, 1));
        assert_eq!((s.worst, s.worst_point, s.margin), (Some(58.0), Some(1), Some(-2.0)));
        assert_eq!(s.range, Some((58.0, 70.0)));
        assert_eq!(s.status(), Status::Fail);
        // A window: the worst value is the one nearest either limit.
        let s = summarize(&spec("1", "2"), &[Ok(1.4), Ok(1.9), Ok(1.2)]);
        assert_eq!((s.worst, s.status()), (Some(1.9), Status::Pass));
        assert!((s.margin.unwrap() - 0.1).abs() < 1e-12);
        assert_eq!(summarize(&spec("", ""), &[Ok(3.0)]).status(), Status::Info);
        assert_eq!(summarize(&spec("1", ""), &[Ok(3.0), Err("x".into())]).status(), Status::Error);
        assert_eq!(summarize(&spec("1", ""), &[]).status(), Status::Error);
        assert_eq!(spec("1m", "").min, "1m");
        assert_eq!(summarize(&spec("1m", "2m"), &[]).limits(), "1m .. 2m");
    }

    #[test]
    fn junit_compare_and_history() {
        let t = |name: &str, vals: Vec<Result<f64, String>>| TestResult {
            name: name.into(),
            cell: format!("work/{name}"),
            points: (0..vals.len()).map(|k| format!("tt 27°C mc#{k}")).collect(),
            specs: vec![summarize(&spec("60", ""), &vals)],
            ..Default::default()
        };
        let before = VerifyRun { time: utc(0), unix: 0, tests: vec![t("ota", vec![Ok(65.0), Ok(61.0)])] };
        let after = VerifyRun {
            time: utc(86_400 * 365),
            unix: 1,
            tests: vec![t("ota", vec![Ok(65.0), Ok(59.0)]), t("ldo", vec![Ok(70.0)])],
        };
        assert_eq!(before.time, "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_763_285_400), "2025-11-16T09:30:00Z");
        assert_eq!(after.status(), Status::Fail);
        assert_eq!(after.counts(), Counts { pass: 1, fail: 1, error: 0, info: 0 });
        let x = after.junit();
        assert!(x.contains("<testsuites name=\"ic-studio verify\" tests=\"2\" failures=\"1\""), "{x}");
        assert!(x.contains("<failure message=\"gain 59 (limits &gt;= 60)\">1 of 2 point(s)"), "{x}");
        assert!(x.contains("at tt 27°C mc#1"), "{x}");
        let ch = after.compare(&before);
        assert_eq!(ch.len(), 2, "{ch:?}");
        assert!(ch[0].is_regression() && ch[0].worst_before == Some(61.0));
        assert_eq!((ch[1].before, ch[1].after), (None, Status::Pass));
        assert!(before.compare(&before).is_empty());

        let dir = std::env::temp_dir().join(format!("ics-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = save_history(&dir, &before).unwrap();
        let b = save_history(&dir, &before).unwrap();
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("19700101-000000"));
        assert_eq!(list_history(&dir).len(), 2);
        assert_eq!(load_run(&a).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_resolves_cells() {
        let mut p = crate::examples::demo_project();
        let with_specs = default_tests(&p);
        let plan = VerifyPlan::default();
        assert_eq!(plan.resolve(&p), with_specs);
        let any = p.all_cells().find(|(_, c)| c.schematic.is_some()).map(|(r, _)| r).unwrap();
        let mut plan = VerifyPlan { tests: vec![TestDef::new(&any.to_string()), TestDef::new("nosuch")] };
        plan.tests.push(TestDef { name: "named".into(), cell: any.cell.clone(), enabled: false });
        assert_eq!(plan.resolve(&p), vec![(any.cell.clone(), any.clone())]);
        p.settings.verification = plan.clone();
        let json = serde_json::to_string(&p.settings).unwrap();
        assert!(json.contains("\"verification\""), "{json}");
    }
}
