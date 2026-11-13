//! Spec-driven optimizer driver: evaluates candidates of a cell's [`OptSetup`] by netlisting
//! (optionally after regenerating, routing and extracting the layout) and simulating, and runs
//! the [`Optimizer`] of `ics_core::optimize` over them.

use ics_core::netlist::netlist_for;
use ics_core::optimize::{self, Bounds, OptSetup, Optimizer, spice_number};
use ics_core::project::Library;
use ics_core::sweep::Spec;
use ics_core::{Cell, CellRef, Layout, Project, Schematic};

use crate::calc::{self, CalcValue};
use crate::{SimOptions, SimResults};

/// One evaluated candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct OptEval {
    /// Variable values, in the order of [`OptSetup::vars`].
    pub values: Vec<f64>,
    /// Measured value of every spec of the cell's sweep setup (with or without limits).
    pub specs: Vec<Result<f64, String>>,
    /// Combined objective ([`optimize::objective`]), lower is better.
    pub objective: f64,
    /// Every spec with limits passes.
    pub passed: bool,
    /// Layout / routing / extraction summary, or the error that stopped the evaluation.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OptReport {
    pub evals: Vec<OptEval>,
    /// Index of the lowest objective in `evals`.
    pub best: Option<usize>,
    /// Index of the starting point (the first evaluation).
    pub start: Option<usize>,
}

impl OptReport {
    pub fn best(&self) -> Option<&OptEval> {
        self.best.map(|b| &self.evals[b])
    }
}

/// A spec measured on simulation results: the calculator expression on the spec's dataset,
/// which must give a scalar (or a one-point waveform).
pub fn measure(s: &Spec, res: &SimResults) -> Result<f64, String> {
    let ds = res.dataset(&s.analysis).ok_or_else(|| format!("no {} results", s.analysis))?;
    match calc::eval(&s.expr, ds)? {
        CalcValue::Scalar(x) => Ok(x),
        CalcValue::Wave(w) if w.re.len() == 1 => Ok(w.re[0]),
        CalcValue::Wave(_) => Err("the expression gives a waveform; use a measurement such as max(…)".into()),
    }
}

/// `INST.param` of a target, the parameter lower case; `None` for a design variable.
fn inst_param(target: &str) -> Option<(&str, String)> {
    target.trim().split_once('.').map(|(i, p)| (i, p.to_ascii_lowercase()))
}

/// The schematic with the values written into the instance parameters of `INST.param`
/// variables (as SPICE numbers); design variables and unknown instances are left alone.
pub fn apply_values(sch: &Schematic, setup: &OptSetup, values: &[f64]) -> Schematic {
    let mut sch = sch.clone();
    for (v, x) in setup.vars.iter().zip(values) {
        let Some((inst, param)) = inst_param(&v.target) else { continue };
        if let Some(i) = sch.instances.iter_mut().find(|i| i.name.eq_ignore_ascii_case(inst)) {
            i.params.insert(param, spice_number(*x));
        }
    }
    sch
}

/// Current values of the variables in the schematic (`None` for design variables, unknown
/// instances and unparsable values).
pub fn current_values(sch: &Schematic, setup: &OptSetup) -> Vec<Option<f64>> {
    setup
        .vars
        .iter()
        .map(|v| {
            let (inst, param) = inst_param(&v.target)?;
            let i = sch.instances.iter().find(|i| i.name.eq_ignore_ascii_case(inst))?;
            ics_core::units::parse_spice(i.param(&param)).filter(|x| x.is_finite())
        })
        .collect()
}

/// The optimizer's starting point: the current values clamped into the bounds and snapped to
/// their grids, the box centre where unknown.
pub fn start_values(sch: &Schematic, setup: &OptSetup) -> Result<Vec<f64>, String> {
    let cur = current_values(sch, setup);
    setup
        .vars
        .iter()
        .zip(cur)
        .map(|(v, c)| {
            let b = Bounds::of(v)?;
            Ok(b.to_real(c.map_or(0.5, |x| b.to_unit(x))))
        })
        .collect()
}

fn check_setup<'a>(project: &'a Project, cell: &CellRef) -> Result<(&'a Cell, &'a Schematic), String> {
    let c = project.cell(cell).ok_or_else(|| format!("no cell '{cell}'"))?;
    let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
    let setup = &c.sim.optimize;
    if setup.vars.is_empty() {
        return Err("no optimizer variables".into());
    }
    if !c.sim.sweep.specs.iter().any(optimize::has_limit) {
        return Err("no spec with a limit to optimise for".into());
    }
    for v in &setup.vars {
        Bounds::of(v)?;
        if let Some((inst, _)) = inst_param(&v.target)
            && !sch.instances.iter().any(|i| i.name.eq_ignore_ascii_case(inst))
        {
            return Err(format!("{}: no instance '{inst}' in the schematic", v.target.trim()));
        }
    }
    Ok((c, sch))
}

/// The deck of a candidate (and a note): `sch` netlisted with the cell's setup, design
/// variables substituted, and layout parasitics applied for post-layout evaluation (or when the
/// setup asks for parasitics of the existing layout).
fn candidate_deck(
    project: &Project,
    cell: &CellRef,
    c: &Cell,
    sch: &Schematic,
    setup: &OptSetup,
    values: &[f64],
) -> Result<(String, String), String> {
    let nl = netlist_for(sch, &project.tech, &c.sim.corner);
    let mut deck = nl.deck(&cell.to_string(), &c.sim);
    for (v, x) in setup.vars.iter().zip(values) {
        if inst_param(&v.target).is_none() {
            deck = deck.replace(&format!("{{{}}}", v.target.trim()), &spice_number(*x));
        }
    }
    let mut note = vec![];
    if setup.post_layout {
        let (pl, summary) = regenerate(project, cell, c, sch)?;
        deck = pl.apply(&deck);
        note.push(summary);
    } else if c.sim.parasitics {
        let pl = ics_core::pex::post_layout(project, cell)?;
        deck = pl.apply(&deck);
        note.push(format!("existing layout: {}", pl.summary));
    }
    Ok((deck, note.join("; ")))
}

/// Place `sch` into a fresh layout (schematic-driven placement), route it and compute its
/// parasitics, on a scratch project holding only the cell and its generated pcells.
fn regenerate(
    project: &Project,
    cell: &CellRef,
    c: &Cell,
    sch: &Schematic,
) -> Result<(ics_core::pex::PostLayout, String), String> {
    let mut p = Project::new(&project.name);
    p.tech = project.tech.clone();
    p.settings = project.settings.clone();
    let mut lib = Library::new(&cell.lib);
    lib.cells.push(Cell { schematic: Some(sch.clone()), layout: None, ..c.clone() });
    p.libraries = vec![lib];
    let mut lay = Layout::default();
    let plan = ics_core::sdl::plan(sch, &p.tech);
    let placed = ics_core::sdl::apply(&mut p, &mut lay, &plan)?;
    if placed.is_empty() {
        return Err("post-layout: no device could be placed".into());
    }
    let rep = ics_core::route::route(sch, &mut lay, &p, &p.tech)?;
    p.cell_mut(cell).ok_or("post-layout: scratch cell missing")?.layout = Some(lay);
    let pl = ics_core::pex::post_layout(&p, cell)?;
    let summary = format!("placed {} device(s); {}; {}", placed.len(), rep.summary(), pl.summary);
    Ok((pl, summary))
}

/// Evaluate one candidate: the cell's schematic with the variable values applied
/// (`INST.param` set on the instance, design variables substituted as `{name}`), netlisted with
/// the cell's [`SimSetup`](ics_core::SimSetup) (corner, enabled analyses, backend; `opts` gives
/// the ngspice executable) and, with `post_layout`, simulated with the parasitics of a freshly
/// placed, routed and extracted layout. Every spec of the cell's sweep setup is measured.
pub fn evaluate(project: &Project, cell: &CellRef, setup: &OptSetup, values: &[f64], opts: &SimOptions) -> OptEval {
    let fail = |specs: usize, e: String| OptEval {
        values: values.to_vec(),
        specs: vec![Err(e.clone()); specs],
        objective: optimize::FAIL_PENALTY * specs.max(1) as f64,
        passed: false,
        note: e,
    };
    let Some(c) = project.cell(cell) else { return fail(0, format!("no cell '{cell}'")) };
    let specs = &c.sim.sweep.specs;
    let Some(base) = c.schematic.as_ref() else { return fail(specs.len(), "cell has no schematic".into()) };
    let sch = apply_values(base, setup, values);
    let (deck, note) = match candidate_deck(project, cell, c, &sch, setup, values) {
        Ok(d) => d,
        Err(e) => return fail(specs.len(), e),
    };
    let analyses: Vec<_> = c.sim.enabled().cloned().collect();
    let opts = SimOptions { backend: c.sim.backend, ..opts.clone() };
    let (measured, note) = match crate::simulate(&deck, &analyses, &opts) {
        Ok(res) => (specs.iter().map(|s| measure(s, &res)).collect::<Vec<_>>(), note),
        Err(e) => {
            let e = format!("simulation failed: {e}");
            let note = if note.is_empty() { e.clone() } else { format!("{note}; {e}") };
            (vec![Err(e); specs.len()], note)
        }
    };
    let area_ref = start_values(base, setup).map_or(0.0, |s| optimize::area(base, setup, &s));
    let area = optimize::area(&sch, setup, values);
    let objective = optimize::objective(setup.goal, specs, &measured, area, area_ref);
    let passed = specs.iter().any(optimize::has_limit) && optimize::score(specs, &measured) == 0.0;
    OptEval { values: values.to_vec(), specs: measured, objective, passed, note }
}

/// Run the cell's optimizer setup to completion (budget or convergence), starting from the
/// schematic's current values. `progress` is called after every evaluation with it and the
/// number of evaluations so far; returning false stops the run.
pub fn run(
    project: &Project,
    cell: &CellRef,
    opts: &SimOptions,
    progress: &mut dyn FnMut(&OptEval, usize) -> bool,
) -> Result<OptReport, String> {
    let (c, sch) = check_setup(project, cell)?;
    let setup = &c.sim.optimize;
    let mut opt = Optimizer::new(setup, start_values(sch, setup)?)?;
    let mut report = OptReport::default();
    while let Some(x) = opt.ask() {
        let e = evaluate(project, cell, setup, &x, opts);
        opt.tell(e.objective);
        let k = report.evals.len();
        if report.best.is_none_or(|b| e.objective < report.evals[b].objective) {
            report.best = Some(k);
        }
        report.evals.push(e);
        report.start.get_or_insert(0);
        if !progress(&report.evals[k], k + 1) {
            break;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::examples;
    use ics_core::optimize::{Goal, OptVar};

    fn project_with(mut cell: Cell, specs: Vec<Spec>, setup: OptSetup) -> (Project, CellRef) {
        cell.sim.sweep.specs = specs;
        cell.sim.optimize = setup;
        let mut p = Project::new("t");
        let r = p.add_cell("work", cell).unwrap();
        (p, r)
    }

    fn bandwidth_spec() -> Spec {
        Spec { name: "bw".into(), analysis: "ac".into(), expr: "bandwidth(v(out))".into(), min: "9k".into(), max: "11k".into() }
    }

    fn r_var() -> OptVar {
        OptVar { target: "R1.r".into(), min: "100".into(), max: "1meg".into(), log: true, step: String::new() }
    }

    #[test]
    fn rc_lowpass_hits_bandwidth_window() {
        let setup = OptSetup { vars: vec![r_var()], max_evals: 40, ..Default::default() };
        let (p, r) = project_with(examples::rc_lowpass(), vec![bandwidth_spec()], setup.clone());
        let sch = p.cell(&r).unwrap().schematic.clone().unwrap();
        assert_eq!(current_values(&sch, &setup), [Some(1e3)]);
        let mut calls = 0;
        let rep = run(&p, &r, &SimOptions::default(), &mut |_, n| {
            calls = n;
            true
        })
        .unwrap();
        assert_eq!(calls, rep.evals.len());
        assert_eq!(rep.start, Some(0));
        assert!(!rep.evals[0].passed, "1k gives ~159 kHz");
        let best = rep.best().unwrap();
        assert!(best.passed, "{:?}", rep.evals.iter().map(|e| (e.values[0], &e.specs[0])).collect::<Vec<_>>());
        let bw = *best.specs[0].as_ref().unwrap();
        assert!((9e3..=11e3).contains(&bw), "{bw}");
        assert!(best.objective <= 0.0 && rep.evals.iter().filter(|e| !e.passed).all(|e| e.objective > 0.0));
        // Margin goal centres the bandwidth in the window: R ~ 1 / (2 pi 10k 1n) = 15.9k.
        assert!((best.values[0] / 15.9e3 - 1.0).abs() < 0.05, "{}", best.values[0]);
        let applied = apply_values(&sch, &setup, &best.values);
        let v = current_values(&applied, &setup)[0].unwrap();
        assert!((v / best.values[0] - 1.0).abs() < 1e-5, "{v}");
        assert_eq!(applied.instances.iter().find(|i| i.name == "R1").unwrap().param("r"), spice_number(best.values[0]));

        let mut stopped = 0;
        let early = run(&p, &r, &SimOptions::default(), &mut |_, n| {
            stopped = n;
            n < 3
        })
        .unwrap();
        assert_eq!((early.evals.len(), stopped), (3, 3));
    }

    #[test]
    fn design_variables_and_errors() {
        let mut cell = examples::rc_lowpass();
        let sch = cell.schematic.as_mut().unwrap();
        sch.instances.iter_mut().find(|i| i.name == "C1").unwrap().params.insert("c".into(), "{cval}".into());
        let setup = OptSetup {
            vars: vec![OptVar { target: "cval".into(), min: "100p".into(), max: "10n".into(), log: true, ..Default::default() }],
            goal: Goal::MinArea,
            ..Default::default()
        };
        let (p, r) = project_with(cell, vec![bandwidth_spec()], setup.clone());
        let e = evaluate(&p, &r, &setup, &[1.59e-8], &SimOptions::default());
        assert!(e.passed, "{e:?}");
        assert!(e.objective < 0.0 && e.objective > -1.0);
        assert_eq!(current_values(p.cell(&r).unwrap().schematic.as_ref().unwrap(), &setup), [None]);

        let (p, r) = project_with(examples::rc_lowpass(), vec![], OptSetup { vars: vec![r_var()], ..Default::default() });
        assert!(run(&p, &r, &SimOptions::default(), &mut |_, _| true).unwrap_err().contains("no spec"));
        let bad = OptSetup { vars: vec![OptVar { target: "R9.r".into(), ..r_var() }], ..Default::default() };
        let (p, r) = project_with(examples::rc_lowpass(), vec![bandwidth_spec()], bad);
        assert!(run(&p, &r, &SimOptions::default(), &mut |_, _| true).unwrap_err().contains("R9"));
        let (p, r) = project_with(examples::rc_lowpass(), vec![bandwidth_spec()], OptSetup::default());
        assert!(run(&p, &r, &SimOptions::default(), &mut |_, _| true).unwrap_err().contains("variables"));
    }

    #[test]
    fn post_layout_inverter() {
        let specs = vec![
            Spec {
                name: "vm".into(),
                analysis: "dc".into(),
                expr: "cross(v(out), 0.9)".into(),
                min: "0.7".into(),
                max: "1.1".into(),
            },
            Spec {
                name: "tpd".into(),
                analysis: "tran".into(),
                expr: "delay(v(in), 0.9, v(out), 0.9)".into(),
                min: String::new(),
                max: "1n".into(),
            },
        ];
        let cell = examples::cmos_inverter();
        let pmos = cell.schematic.as_ref().unwrap().instances.iter().find(|i| i.kind == ics_core::DeviceKind::Pmos).unwrap();
        let target = format!("{}.w", pmos.name);
        let setup = OptSetup {
            vars: vec![OptVar { target, min: "500n".into(), max: "8u".into(), log: true, step: "10n".into() }],
            post_layout: true,
            ..Default::default()
        };
        let (p, r) = project_with(cell, specs, setup.clone());
        let t0 = std::time::Instant::now();
        let pre = evaluate(&p, &r, &OptSetup { post_layout: false, ..setup.clone() }, &[2e-6], &SimOptions::default());
        let post = evaluate(&p, &r, &setup, &[2e-6], &SimOptions::default());
        eprintln!("post-layout evaluation pair: {:.2} s; {}", t0.elapsed().as_secs_f64(), post.note);
        for e in [&pre, &post] {
            assert!(e.specs.iter().all(|s| s.as_ref().is_ok_and(|v| v.is_finite())), "{e:?}");
        }
        assert!(post.note.contains("routed"), "{}", post.note);
        let (d0, d1) = (*pre.specs[1].as_ref().unwrap(), *post.specs[1].as_ref().unwrap());
        assert!(d1 > d0, "parasitics slow the inverter down: {d0} -> {d1}");
    }
}
