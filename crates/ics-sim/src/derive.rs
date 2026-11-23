//! Finishing a derived design (see `ics_core::derive`): write the optimizer's best sizes into
//! the copy, build its layout like the reference's, check it (DRC with automatic fixes, LVS) and
//! summarise the result next to the reference's numbers.

use ics_core::derive::Plan;
use ics_core::drc::parse_any;
use ics_core::lvs::{LvsOptions, compare};
use ics_core::units::format_eng;
use ics_core::{CellRef, Project};

use crate::SimResults;
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
    /// What the new sizes changed that the specs do not cover (see [`health`]).
    pub health: Vec<Finding>,
}

/// One result of the health check.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// Likely a problem (`true`) or worth knowing.
    pub warn: bool,
    pub text: String,
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
        for f in &self.health {
            s += &format!("{}: {}\n", if f.warn { "Check" } else { "Note" }, f.text);
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
    let original = c.schematic.clone().ok_or("the cell has no schematic")?;
    let sch = apply_values(&original, &setup, &best.values);
    out.health = health(p, target, &original, &sch);
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

/// A guard spec keeping the supply current of the new design within 0.5x to 2x of `cell`'s
/// (the `idd` preset, measured on the cell as it is), so the optimizer cannot buy a target by
/// starving or flooding the circuit. `None` without an operating point or a supply source.
pub fn power_guard(p: &Project, cell: &CellRef) -> Option<ics_core::sweep::Spec> {
    let c = p.cell(cell)?;
    let sch = c.schematic.as_ref()?;
    let mut sim = c.sim.clone();
    sim.set_enabled("op", true);
    let mut spec = ics_core::derive::spec_presets(sch, &sim, &[]).into_iter().find(|s| s.name == "idd")?;
    let d = op(p, cell, sch)?;
    let res = SimResults { backend: String::new(), datasets: vec![d], log: String::new(), elapsed_ms: 0.0, warnings: vec![] };
    let i = crate::optimize::measure(&spec, &res).ok().filter(|i| *i > 0.0)?;
    let round3 = |x: f64| {
        let e = 10f64.powf(x.log10().floor() - 2.0);
        (x / e).round() * e
    };
    spec.min = ics_core::optimize::spice_number(round3(i * 0.5));
    spec.max = ics_core::optimize::spice_number(round3(i * 2.0));
    Some(spec)
}

/// The operating point of `sch` simulated with `cell`'s setup.
fn op(p: &Project, cell: &CellRef, sch: &ics_core::Schematic) -> Option<crate::Dataset> {
    let c = p.cell(cell)?;
    let deck = ics_core::netlist::netlist_for(sch, &p.tech, &c.sim.corner).deck(&cell.to_string(), &c.sim);
    let opts = crate::SimOptions { backend: ics_core::Backend::Builtin, ..Default::default() };
    let res = crate::simulate(&deck, &[ics_core::Analysis::Op], &opts).ok()?;
    res.datasets.into_iter().find(|d| d.analysis == "op")
}

/// Power delivered by the DC voltage sources (W).
fn supply_power(sch: &ics_core::Schematic, d: &crate::Dataset) -> f64 {
    let conn = ics_core::connectivity::extract(sch);
    let v = |net: &str| if net == "0" { Some(0.0) } else { d.get(&format!("v({net})")).and_then(|x| x.re.first().copied()) };
    sch.instances
        .iter()
        .filter(|i| i.kind == ics_core::schematic::DeviceKind::VSource)
        .filter_map(|i| {
            let (np, nn) = (conn.net_of(i.id, "p")?, conn.net_of(i.id, "n")?);
            let cur = d.get(&format!("i({})", i.name))?.re.first().copied()?;
            Some(-(v(np)? - v(nn)?) * cur)
        })
        .filter(|w| *w > 0.0)
        .sum()
}

/// What the new sizes changed that the specs do not measure, comparing the operating points
/// and sizes of `before` and `after`: transistors that left saturation or sit at its edge,
/// supply power, the gate area of matched pairs (random mismatch grows as 1/sqrt(W L)), bias
/// currents and channel lengths made shorter than the original's.
pub fn health(p: &Project, cell: &CellRef, before: &ics_core::Schematic, after: &ics_core::Schematic) -> Vec<Finding> {
    let mut out = vec![];
    let warn = |text: String| Finding { warn: true, text };
    let note = |text: String| Finding { warn: false, text };
    let (Some(b), Some(a)) = (op(p, cell, before), op(p, cell, after)) else { return out };
    let dev = |d: &crate::Dataset, name: &str| d.devices.iter().find(|x| x.name.eq_ignore_ascii_case(name)).cloned();
    let val = |x: &crate::results::DeviceOp, k: &str| x.values.iter().find(|(n, _)| n == k).map(|(_, v)| v.abs());
    let spare = |x: &crate::results::DeviceOp| Some(val(x, "vds")? - (val(x, "vgs")? - val(x, "vth")?));
    for i in after.instances.iter().filter(|i| i.kind.is_mos()) {
        let (Some(x0), Some(x1)) = (dev(&b, &i.name), dev(&a, &i.name)) else { continue };
        let (r0, r1) = (x0.region.as_deref().unwrap_or(""), x1.region.as_deref().unwrap_or(""));
        if r0 == "saturation" && r1 != "saturation" {
            out.push(warn(format!(
                "{} left saturation (now {r1}): as an amplifier, load or current source it no longer works as intended",
                i.name
            )));
        } else if r1 == "saturation"
            && let (Some(s0), Some(s1)) = (spare(&x0), spare(&x1))
            && s1 < 0.1
            && s0 >= 0.1
        {
            out.push(warn(format!("{} is now only {:.0} mV from leaving saturation", i.name, s1 * 1e3)));
        }
    }
    let (p0, p1) = (supply_power(before, &b), supply_power(after, &a));
    if p0 > 0.0 && p1 > 0.0 {
        let k = p1 / p0;
        if k > 1.5 {
            out.push(warn(format!("supply power {} -> {} W ({k:.1}x)", format_eng(p0, ""), format_eng(p1, ""))));
        } else if k < 0.5 {
            out.push(note(format!(
                "supply power {} -> {} W ({:.1}x less): lower current usually also means a slower circuit",
                format_eng(p0, ""),
                format_eng(p1, ""),
                1.0 / k
            )));
        }
    }
    let num = |i: &ics_core::schematic::Instance, k: &str| ics_core::units::parse_spice(i.param(k)).filter(|x| *x > 0.0);
    let find = |s: &ics_core::Schematic, n: &str| s.instances.iter().find(|i| i.name == n).cloned();
    for g in ics_core::derive::suggest(before, &p.tech, &[]).iter().filter(|g| g.param == "w") {
        let (Some(i0), Some(i1)) = (find(before, &g.devices[0]), find(after, &g.devices[0])) else { continue };
        let (Some(w0), Some(l0), Some(w1), Some(l1)) = (num(&i0, "w"), num(&i0, "l"), num(&i1, "w"), num(&i1, "l")) else {
            continue;
        };
        let names = g.devices.join(", ");
        use ics_core::derive::Role;
        if g.devices.len() >= 2 && matches!(g.role, Role::InputPair | Role::Mirror) {
            let k = ((w0 * l0) / (w1 * l1)).sqrt();
            if k > 1.2 {
                out.push(warn(format!(
                    "{names} ({}) have {:.1}x less gate area: random mismatch (offset) about {k:.1}x larger",
                    g.role.label(),
                    (w0 * l0) / (w1 * l1)
                )));
            }
        }
        if l1 < l0 * 0.9 && matches!(g.role, Role::InputPair | Role::Mirror | Role::CurrentSource | Role::MirrorOutput) {
            out.push(note(format!(
                "{names} ({}) got a shorter L ({} -> {}): lower output resistance and worse matching than the original",
                g.role.label(),
                format_eng(l0, ""),
                format_eng(l1, "")
            )));
        }
        if g.role == Role::CurrentSource
            && let (Some(x0), Some(x1)) = (dev(&b, &g.devices[0]), dev(&a, &g.devices[0]))
            && let (Some(c0), Some(c1)) = (val(&x0, "id"), val(&x1, "id"))
            && c0 > 0.0
            && !(0.5..=2.0).contains(&(c1 / c0))
        {
            out.push(note(format!(
                "bias current of {names}: {} -> {} A; speed and power follow it",
                format_eng(c0, ""),
                format_eng(c1, "")
            )));
        }
    }
    out
}

/// A routed and checked layout candidate.
struct Built {
    layout: ics_core::Layout,
    lines: Vec<String>,
    /// Unrouted nets + DRC violations left + LVS differences.
    issues: usize,
}

/// Route a placed layout, fix what DRC can fix automatically and run LVS.
fn build(p: &Project, sch: &ics_core::Schematic, lay: ics_core::Layout, how: String) -> Built {
    // The router's rip-up budget changes the wiring: when the default leaves a DRC problem the
    // automatic fixes cannot repair, another budget often routes it clean.
    let mut best: Option<Built> = None;
    for rips in [ics_core::route::MAX_RIPS, 0, 8] {
        let b = build_once(p, sch, lay.clone(), how.clone(), rips);
        if best.as_ref().is_none_or(|old| b.issues < old.issues) {
            let clean = b.issues == 0;
            best = Some(b);
            if clean {
                break;
            }
        }
    }
    best.expect("one candidate at least")
}

fn build_once(p: &Project, sch: &ics_core::Schematic, mut lay: ics_core::Layout, how: String, rips: u32) -> Built {
    let mut lines = vec![how];
    let mut issues = 0;
    match ics_core::route::route_with(sch, &mut lay, p, &p.tech, rips) {
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
    // The generator draws transistors only: compare them, and list the passives still to draw.
    use ics_core::schematic::DeviceKind;
    let passives = sch
        .instances
        .iter()
        .filter(|i| matches!(i.kind, DeviceKind::Resistor | DeviceKind::Capacitor | DeviceKind::Inductor) && !i.lvs_ignore())
        .count();
    let mut opts = LvsOptions::for_tech(&p.tech);
    if passives > 0 {
        opts.ignore_kinds.extend([DeviceKind::Resistor, DeviceKind::Capacitor, DeviceKind::Inductor]);
    }
    let lvs = compare(sch, &x, &opts);
    issues += passives;
    if passives > 0 {
        lines.push(format!(
            "{passives} resistor(s) / capacitor(s) still to draw by hand (Create > PCell: poly resistor, MOM capacitor): \
             the generator places transistors only"
        ));
    }
    if lvs.matched {
        lines.push(
            if passives > 0 { "LVS: the transistors match the schematic" } else { "LVS: the layout matches the schematic" }
                .into(),
        );
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

/// Derive in one call (for the CLI and MCP): create the copy, size it, finish it. With
/// `keep_power`, the [`power_guard`] spec is added unless the plan has an `idd` spec.
pub fn derive(
    p: &mut Project,
    reference: &CellRef,
    plan: &Plan,
    layout: bool,
    keep_power: bool,
    opts: &crate::SimOptions,
) -> Result<(CellRef, Outcome), String> {
    let mut plan = plan.clone();
    if keep_power
        && !plan.specs.iter().any(|s| s.name == "idd")
        && let Some(g) = power_guard(p, reference)
    {
        plan.specs.push(g);
    }
    let target = ics_core::derive::create(p, reference, &plan)?;
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
        let (to, out) = derive(&mut p, &from, &plan, true, true, &crate::SimOptions::default()).unwrap();
        assert!(out.passed, "{}", out.text(&to));
        assert!(*out.after[0].as_ref().unwrap() >= 50.0);
        assert!(*out.before[0].as_ref().unwrap() < 50.0, "the reference misses the new target");
        let c = p.cell(&to).unwrap();
        assert!(c.notes.starts_with("Sized by the optimizer"), "{}", c.notes);
        assert!(c.notes.contains("Derived from work/ota5t for: gain_db >= 50"));
        assert!(c.layout.is_some());
        assert_eq!(out.layout_clean, Some(true), "{:?}", out.layout);
        // Least change, the current guard and the L floor keep the design sound: no warnings, the
        // long channels stay long.
        assert!(out.health.iter().all(|f| !f.warn), "{}", out.text(&to));
        assert!(out.values.iter().filter(|(t, _)| t.contains(".l")).all(|(_, l)| *l >= 500e-9 - 1e-12), "{:?}", out.values);
        // A starved tail and short channels are what the health check is for.
        let mut bad = sch.clone();
        for i in bad.instances.iter_mut() {
            match i.name.as_str() {
                "M5" => _ = i.params.insert("w".into(), "500n".into()),
                "M1" | "M2" => _ = i.params.insert("l".into(), "250n".into()),
                _ => {}
            }
        }
        let h = health(&p, &from, &sch, &bad);
        assert!(h.iter().any(|f| f.text.starts_with("supply power")), "{h:?}");
        assert!(h.iter().any(|f| f.text.contains("got a shorter L")), "{h:?}");
        // The reference is untouched.
        assert_eq!(p.cell(&from).unwrap().schematic.as_ref(), Some(&sch));
    }
}
