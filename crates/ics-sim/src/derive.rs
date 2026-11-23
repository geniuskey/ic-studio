//! Finishing a derived design (see `ics_core::derive`): write the optimizer's best sizes into
//! the copy, build its layout like the reference's, check it (DRC with automatic fixes, LVS) and
//! summarise the result next to the reference's numbers.

use ics_core::derive::Plan;
use ics_core::drc::parse_any;
use ics_core::lvs::{LvsOptions, compare};
use ics_core::units::format_eng;
use ics_core::{CellRef, Project};

use crate::optimize::{OptReport, apply_values};

/// What deriving achieved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// Spec names with their limits text.
    pub specs: Vec<(String, String)>,
    /// Each spec measured on the reference sizes and on the chosen sizes.
    pub before: Vec<Result<f64, String>>,
    pub after: Vec<Result<f64, String>>,
    /// Every spec with a limit passes at the chosen sizes.
    pub passed: bool,
    /// The chosen value of every variable.
    pub values: Vec<(String, f64)>,
    pub evals: usize,
    /// Layout steps: placement, routing, DRC, LVS (empty without a layout).
    pub layout: Vec<String>,
    /// The layout is DRC clean and matches the schematic.
    pub layout_clean: Option<bool>,
}

impl Outcome {
    /// A plain-text report.
    pub fn text(&self, cell: &CellRef) -> String {
        let fmt = |r: &Result<f64, String>| r.as_ref().map_or_else(|e| format!("({e})"), |v| format_eng(*v, ""));
        let mut s = format!(
            "{cell}: {} after {} simulation(s)\n",
            if self.passed { "every spec passes" } else { "some specs still fail" },
            self.evals
        );
        for (k, (name, limits)) in self.specs.iter().enumerate() {
            let (b, a) = (self.before.get(k).map(fmt).unwrap_or_default(), self.after.get(k).map(fmt).unwrap_or_default());
            s += &format!("  {name:<10} {limits:<14} {b:>10} -> {a}\n");
        }
        if !self.values.is_empty() {
            s += "Sizes:\n";
            for (t, v) in &self.values {
                s += &format!("  {t} = {}\n", format_eng(*v, ""));
            }
        }
        for l in &self.layout {
            s += &format!("Layout: {l}\n");
        }
        s
    }
}

/// Write the best point of `report` into `target`'s schematic, record the sizes in its notes
/// and, with `layout`, place it like `reference`'s layout, route, fix DRC and run LVS.
pub fn finish(
    p: &mut Project,
    reference: &CellRef,
    target: &CellRef,
    report: &OptReport,
    layout: bool,
) -> Result<Outcome, String> {
    let c = p.cell(target).ok_or_else(|| format!("no cell '{target}'"))?;
    let setup = c.sim.optimize.clone();
    let specs = c.sim.sweep.specs.clone();
    let best = report.best().ok_or("the optimizer evaluated nothing")?;
    let start = report.start.map(|k| &report.evals[k]).unwrap_or(best);
    let mut out = Outcome {
        specs: specs.iter().map(|s| (s.name.clone(), limits(s))).collect(),
        before: start.specs.clone(),
        after: best.specs.clone(),
        passed: best.passed,
        values: setup.vars.iter().map(|v| v.target.clone()).zip(best.values.iter().copied()).collect(),
        evals: report.evals.len(),
        ..Default::default()
    };
    let sch = c.schematic.as_ref().ok_or("the cell has no schematic")?;
    let sch = apply_values(sch, &setup, &best.values);
    let sizes: Vec<String> = out.values.iter().map(|(t, v)| format!("{t} = {}", format_eng(*v, ""))).collect();
    let c = p.cell_mut(target).expect("checked");
    c.schematic = Some(sch.clone());
    if !sizes.is_empty() {
        c.notes = format!("Sized by the optimizer: {}.\n{}", sizes.join(", "), c.notes);
    }
    if layout {
        let has_ref_layout = p.cell(reference).is_some_and(|c| c.layout.is_some());
        let mut best: Option<Built> = None;
        if has_ref_layout && let Ok(rep) = ics_core::by_example::place_like(p, reference, target) {
            let lay = p.cell_mut(target).expect("checked").layout.take().unwrap_or_default();
            best = Some(build(p, &sch, lay, format!("placed {} device(s) like {reference}", rep.placed)));
        }
        if best.as_ref().is_none_or(|b| b.issues > 0) {
            // New sizes may not fit the reference's placement: generate from the schematic.
            let plan = ics_core::sdl::plan(&sch, &p.tech);
            let mut lay = ics_core::Layout::default();
            if let Ok(placed) = ics_core::sdl::apply(p, &mut lay, &plan) {
                let how = match &best {
                    Some(_) => format!(
                        "the reference placement did not fit the new sizes; generated {} device(s) from the schematic",
                        placed.len()
                    ),
                    None => format!("generated {} device(s) from the schematic", placed.len()),
                };
                let b = build(p, &sch, lay, how);
                if best.as_ref().is_none_or(|old| b.issues < old.issues) {
                    best = Some(b);
                }
            }
        }
        if let Some(b) = best {
            out.layout = b.lines;
            out.layout_clean = Some(b.issues == 0);
            p.cell_mut(target).expect("checked").layout = Some(b.layout);
        }
    }
    Ok(out)
}

/// A routed and checked layout candidate.
struct Built {
    layout: ics_core::Layout,
    lines: Vec<String>,
    /// Unrouted nets + DRC violations left + LVS differences.
    issues: usize,
}

/// Route a placed layout, fix what DRC can fix automatically and run LVS.
fn build(p: &Project, sch: &ics_core::Schematic, mut lay: ics_core::Layout, how: String) -> Built {
    let mut lines = vec![how];
    let mut issues = 0;
    match ics_core::route::route(sch, &mut lay, p, &p.tech) {
        Ok(r) => {
            issues += r.failed.len();
            lines.push(r.summary());
        }
        Err(e) => {
            issues += 1;
            lines.push(format!("routing failed: {e}"));
        }
    }
    if let Ok(parsed) = parse_any(&p.tech.deck_text(), &p.tech) {
        let deck = parsed.deck;
        let before = ics_core::drc::run(&deck, &lay, p, &p.tech).violations.len();
        let fixes = if before > 0 { ics_core::drc_fix::fix_all(&deck, &mut lay, p, &p.tech, 200).len() } else { 0 };
        let after = ics_core::drc::run(&deck, &lay, p, &p.tech).violations.len();
        issues += after;
        lines.push(match (before, after) {
            (0, _) => "DRC clean".into(),
            (_, 0) => format!("DRC clean after {fixes} automatic fix(es) of {before} violation(s)"),
            _ => format!("{after} DRC violation(s) left (Ctrl+. in the layout offers fixes)"),
        });
    }
    let x = ics_core::extract::extract(&lay, p, &p.tech);
    let lvs = compare(sch, &x, &LvsOptions::for_tech(&p.tech));
    if lvs.matched {
        lines.push("LVS: the layout matches the schematic".into());
    } else {
        issues += lvs.errors.len().max(1);
        lines.push(format!("LVS: {} difference(s) (Verify > Run LVS lists them)", lvs.errors.len()));
    }
    Built { layout: lay, lines, issues }
}

fn limits(s: &ics_core::sweep::Spec) -> String {
    match (s.min.trim(), s.max.trim()) {
        ("", "") => "-".into(),
        (lo, "") => format!(">= {lo}"),
        ("", hi) => format!("<= {hi}"),
        (lo, hi) => format!("{lo} .. {hi}"),
    }
}

/// Derive in one call (for the CLI and MCP): create the copy, size it, finish it.
pub fn derive(
    p: &mut Project,
    reference: &CellRef,
    plan: &Plan,
    layout: bool,
    opts: &crate::SimOptions,
) -> Result<(CellRef, Outcome), String> {
    let target = ics_core::derive::create(p, reference, plan)?;
    let report = crate::optimize::run(p, &target, opts, &mut |_, _| true)?;
    let out = finish(p, reference, &target, &report, layout)?;
    Ok((target, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::sweep::Spec;

    #[test]
    fn derive_ota_for_more_gain() {
        let mut p = ics_core::examples::demo_project();
        let from = CellRef::new("work", "ota5t");
        let sch = p.cell(&from).unwrap().schematic.clone().unwrap();
        let vars = ics_core::derive::suggest(&sch, &p.tech, &[]).into_iter().map(|s| s.var).collect();
        let spec = |name: &str, expr: &str, min: &str| Spec {
            name: name.into(),
            analysis: "ac".into(),
            expr: expr.into(),
            min: min.into(),
            max: String::new(),
        };
        let plan = Plan {
            lib: "work".into(),
            name: "ota_40db".into(),
            specs: vec![spec("gain_db", "max(db(v(out)))", "50"), spec("pm", "phasemargin(v(out))", "45")],
            vars,
            max_evals: 80,
        };
        let (to, out) = derive(&mut p, &from, &plan, true, &crate::SimOptions::default()).unwrap();
        assert!(out.passed, "{}", out.text(&to));
        assert!(*out.after[0].as_ref().unwrap() >= 50.0);
        assert!(*out.before[0].as_ref().unwrap() < 50.0, "the reference misses the new target");
        let c = p.cell(&to).unwrap();
        assert!(c.notes.starts_with("Sized by the optimizer"), "{}", c.notes);
        assert!(c.notes.contains("Derived from work/ota5t for: gain_db >= 50"));
        assert!(c.layout.is_some());
        assert_eq!(out.layout_clean, Some(true), "{:?}", out.layout);
        // The reference is untouched.
        assert_eq!(p.cell(&from).unwrap().schematic.as_ref(), Some(&sch));
    }
}
