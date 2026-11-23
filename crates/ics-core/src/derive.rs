//! Deriving a new design from a finished one: which device parameters to let the optimizer
//! change (matched devices tied together so they stay equal), what each of them does in words a
//! beginner can follow, common specs to add, and the note recording where the copy came from.
//! The simulation side (sizing, layout, checks) is `ics_sim::derive`.

use std::collections::BTreeMap;

use crate::connectivity;
use crate::optimize::OptVar;
use crate::schematic::{DeviceKind, Schematic};
use crate::sim_setup::SimSetup;
use crate::sweep::Spec;
use crate::tech::Tech;
use crate::units::parse_spice;

/// What a group of transistors does in the circuit, found from how it is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Two equal transistors with a common source and their gates on different nets.
    InputPair,
    /// Transistors sharing gate and source, one of them diode connected (gate on drain).
    Mirror,
    /// The output side of a current mirror, sized on its own to set the mirror ratio.
    MirrorOutput,
    /// A transistor from a supply rail with its gate on a bias net: it sets a current.
    CurrentSource,
    /// Anything else: logic, switches, followers.
    Device,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::InputPair => "input pair",
            Role::Mirror => "current mirror",
            Role::MirrorOutput => "mirror output",
            Role::CurrentSource => "current source",
            Role::Device => "transistor",
        }
    }
}

/// A parameter the optimizer may change, with the devices it drives and why it matters.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub var: OptVar,
    pub devices: Vec<String>,
    pub role: Role,
    /// `w` or `l`.
    pub param: &'static str,
    pub why: &'static str,
}

impl Suggestion {
    /// `W of M1, M2 (input pair)`.
    pub fn title(&self) -> String {
        format!("{} of {} ({})", self.param.to_ascii_uppercase(), self.devices.join(", "), self.role.label())
    }
}

fn is_rail(net: &str) -> bool {
    let n = net.to_ascii_lowercase();
    matches!(n.as_str(), "0" | "gnd" | "gnd!" | "vss" | "vss!" | "vcc") || n.starts_with("vdd")
}

/// Smallest drawable value of a layer (m), e.g. the minimum channel length from `poly`.
fn layer_min(tech: &Tech, layers: &[&str], fallback: f64) -> f64 {
    layers
        .iter()
        .filter_map(|n| tech.layer_index(n).and_then(|i| tech.layer(i)))
        .map(|l| l.min_width)
        .find(|w| *w > 0)
        .map_or(fallback, |w| w as f64 / tech.dbu_per_um.max(1) as f64 * 1e-6)
}

fn why(role: Role, param: &str) -> &'static str {
    match (role, param) {
        (Role::InputPair, "w") => {
            "Wider input transistors amplify more (higher gm): more gain and bandwidth, more input capacitance."
        }
        (Role::InputPair, _) => "Longer input transistors give more gain and better matching, but a slower amplifier.",
        (Role::Mirror, "w") => "Wider mirror transistors need less voltage headroom; the current ratio stays the same.",
        (Role::Mirror, _) => "Longer mirror transistors copy the current more exactly and, as a load, give more gain.",
        (Role::MirrorOutput, "w") => {
            "The copied current is the reference times (W/L output) / (W/L input): a wider output copies more current."
        }
        (Role::MirrorOutput, _) => {
            "The output's L sets the mirror ratio with its W; keep it equal to the input's L for a precise copy."
        }
        (Role::CurrentSource, "w") => {
            "With the same gate voltage, a wider current source passes more current: faster, more power."
        }
        (Role::CurrentSource, _) => "A longer current source keeps its current steadier when its drain voltage moves.",
        (_, "w") => "A wider transistor is stronger: faster edges and more current, more area and capacitance.",
        _ => "A longer transistor is weaker but has a higher output resistance.",
    }
}

/// The parameters to size for a new target: one W per group of matched transistors (devices of
/// the same type and size sharing source and either gate or source with an equal partner), and
/// an L for groups drawn longer than the minimum (they were made long on purpose: gain,
/// matching). Matched devices share one variable (`M1.w,M2.w`), so they stay equal.
///
/// When a spec measures a current, the output side of a current mirror gets a W of its own:
/// the mirror ratio sets the copied current, so it must be free to change.
pub fn suggest(sch: &Schematic, tech: &Tech, specs: &[Spec]) -> Vec<Suggestion> {
    let ratio = specs.iter().any(|s| crate::advice::measures_current(&s.expr));
    let conn = connectivity::extract(sch);
    let net = |id: u64, pin: &str| conn.net_of(id, pin).unwrap_or("").to_string();
    let lmin = layer_min(tech, &["poly", "po"], 180e-9);
    let wmin = layer_min(tech, &["active", "diff", "od"], 220e-9);
    let num = |s: &str| parse_spice(s).filter(|x| x.is_finite() && *x > 0.0);
    // Group by type, source net and size.
    let mut groups: BTreeMap<(bool, String, String, String, String), Vec<&crate::schematic::Instance>> = BTreeMap::new();
    for i in sch.instances.iter().filter(|i| i.kind.is_mos()) {
        let pmos = matches!(i.kind, DeviceKind::Pmos | DeviceKind::Pmos3);
        let key = (pmos, net(i.id, "s"), i.param("w").to_string(), i.param("l").to_string(), i.param("m").to_string());
        groups.entry(key).or_default().push(i);
    }
    let mut out = vec![];
    let mut grouped: Vec<(Vec<&crate::schematic::Instance>, Role)> = vec![];
    for ((_, source, ..), members) in groups {
        // Split a size group into matched sets: an input pair (different gates, two devices,
        // source not a rail) stays one set; otherwise devices sharing a gate net form a set.
        let gates: Vec<String> = members.iter().map(|i| net(i.id, "g")).collect();
        if members.len() == 2 && gates[0] != gates[1] && !is_rail(&source) {
            grouped.push((members, Role::InputPair));
            continue;
        }
        let mut by_gate: BTreeMap<String, Vec<&crate::schematic::Instance>> = BTreeMap::new();
        for (i, g) in members.into_iter().zip(gates) {
            by_gate.entry(g).or_default().push(i);
        }
        for (gate, set) in by_gate {
            let diode = set.iter().any(|i| net(i.id, "d") == gate);
            let driven = sch.instances.iter().filter(|o| o.kind.is_mos()).any(|o| net(o.id, "d") == gate);
            let role = if set.len() >= 2 && diode {
                if ratio {
                    let (inp, outp): (Vec<_>, Vec<_>) = set.into_iter().partition(|i| net(i.id, "d") == gate);
                    grouped.push((inp, Role::Mirror));
                    for o in outp {
                        grouped.push((vec![o], Role::MirrorOutput));
                    }
                    continue;
                }
                Role::Mirror
            } else if is_rail(&source) && !driven && !gate.is_empty() && !is_rail(&gate) && !conn_is_input(&gate) {
                Role::CurrentSource
            } else {
                Role::Device
            };
            grouped.push((set, role));
        }
    }
    grouped.sort_by(|a, b| a.0[0].name.cmp(&b.0[0].name));
    for (set, role) in grouped {
        let names: Vec<String> = set.iter().map(|i| i.name.clone()).collect();
        let first = set[0];
        for (param, lo_min, long_only) in [("w", wmin, false), ("l", lmin, true)] {
            let Some(cur) = num(first.param(param)) else { continue };
            if long_only && cur < lmin * 1.2 {
                continue;
            }
            let (lo, hi) = if param == "w" { (cur / 4.0, cur * 4.0) } else { (cur / 2.0, cur * 4.0) };
            let var = OptVar {
                target: names.iter().map(|n| format!("{n}.{param}")).collect::<Vec<_>>().join(","),
                min: crate::units::format_eng(lo.max(lo_min), ""),
                max: crate::units::format_eng(hi, ""),
                log: true,
                step: "10n".into(),
            };
            out.push(Suggestion { var, devices: names.clone(), role, param, why: why(role, param) });
        }
    }
    out
}

/// Inputs named like signal inputs are not bias nets.
fn conn_is_input(net: &str) -> bool {
    let n = net.to_ascii_lowercase();
    n.starts_with("in") || n == "a" || n == "b" || n == "ci"
}

/// Common specs a beginner may want to add, not yet in `specs`: gain, unity-gain frequency
/// and phase margin for a cell with an AC analysis and an `out` net, delays for one with a
/// transient and `in` / `out` nets. Limits are left empty for the user to fill in.
pub fn spec_presets(sch: &Schematic, sim: &SimSetup, specs: &[Spec]) -> Vec<Spec> {
    let nets = connectivity::extract(sch).nets();
    let has_net = |n: &str| nets.iter().any(|x| x == n);
    let enabled = |k: &str| sim.enabled().any(|a| a.kind_name() == k);
    let mk = |analysis: &str, name: &str, expr: &str| Spec {
        name: name.into(),
        analysis: analysis.into(),
        expr: expr.into(),
        min: String::new(),
        max: String::new(),
    };
    let mut v = vec![];
    if enabled("ac") && has_net("out") {
        v.push(mk("ac", "gain_db", "max(db(v(out)))"));
        v.push(mk("ac", "ugf", "ugf(v(out))"));
        v.push(mk("ac", "pm", "phasemargin(v(out))"));
        v.push(mk("ac", "bw", "bandwidth(v(out))"));
    }
    if enabled("tran") && has_net("in") && has_net("out") {
        v.push(mk("tran", "tphl", "delay(v(in), 0.9, v(out), 0.9, rise, fall)"));
        v.push(mk("tran", "tplh", "delay(v(in), 0.9, v(out), 0.9, fall, rise)"));
    }
    v.retain(|p| !specs.iter().any(|s| s.name == p.name || s.expr.replace(' ', "") == p.expr.replace(' ', "")));
    v
}

/// The line put at the top of a derived cell's notes.
pub fn derived_note(from: &str, specs: &[Spec]) -> String {
    let targets: Vec<String> = specs
        .iter()
        .filter(|s| !s.min.trim().is_empty() || !s.max.trim().is_empty())
        .map(|s| match (s.min.trim(), s.max.trim()) {
            (lo, "") => format!("{} >= {lo}", s.name),
            ("", hi) => format!("{} <= {hi}", s.name),
            (lo, hi) => format!("{lo} <= {} <= {hi}", s.name),
        })
        .collect();
    if targets.is_empty() { format!("Derived from {from}.") } else { format!("Derived from {from} for: {}.", targets.join(", ")) }
}

/// A new design to derive from a reference cell.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub lib: String,
    pub name: String,
    /// The new cell's specs (targets).
    pub specs: Vec<Spec>,
    /// Parameters the optimizer may change.
    pub vars: Vec<OptVar>,
    /// Simulation budget (0: the optimizer default).
    pub max_evals: usize,
}

/// Copy `from` as the plan's cell with its specs, optimizer setup and a note saying what it
/// was derived for. The reference is left untouched.
pub fn create(p: &mut crate::Project, from: &crate::CellRef, plan: &Plan) -> Result<crate::CellRef, String> {
    if !plan.specs.iter().any(crate::optimize::has_limit) {
        return Err("give at least one spec a target (a min or a max)".into());
    }
    let to = p.copy_cell(from, &plan.lib, plan.name.trim())?;
    let c = p.cell_mut(&to).expect("copied");
    c.sim.sweep.specs = plan.specs.clone();
    c.sim.optimize.vars = plan.vars.clone();
    c.sim.optimize.max_evals = plan.max_evals;
    let note = derived_note(&from.to_string(), &plan.specs);
    c.notes = if c.notes.is_empty() { note } else { format!("{note}\n\n{}", c.notes) };
    c.description = format!("{} (derived from {})", c.description, from.cell).trim_start_matches(' ').to_string();
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::CellRef;

    fn cell(name: &str) -> (Schematic, SimSetup, Vec<Spec>) {
        let p = crate::examples::demo_project();
        let c = p.cell(&CellRef::new("work", name)).unwrap();
        (c.schematic.clone().unwrap(), c.sim.clone(), c.sim.sweep.specs.clone())
    }

    #[test]
    fn ota_groups_and_roles() {
        let (sch, sim, specs) = cell("ota5t");
        let s = suggest(&sch, &Tech::generic(), &specs);
        let t: Vec<(String, Role)> = s.iter().map(|s| (s.var.target.clone(), s.role)).collect();
        assert!(t.contains(&("M1.w,M2.w".into(), Role::InputPair)), "{t:?}");
        assert!(t.contains(&("M1.l,M2.l".into(), Role::InputPair)), "long L is a knob: {t:?}");
        assert!(t.iter().any(|(v, r)| v.starts_with("M3.w") && v.contains("M4.w") && *r == Role::Mirror), "{t:?}");
        assert!(t.contains(&("M5.w".into(), Role::CurrentSource)), "{t:?}");
        assert!(s.iter().all(|s| s.var.log && !s.var.min.is_empty()));
        let presets = spec_presets(&sch, &sim, &specs);
        assert!(presets.iter().any(|p| p.name == "bw"));
        assert!(!presets.iter().any(|p| p.name == "gain_db"), "already a spec");
    }

    #[test]
    fn inverter_sizes_w_only() {
        let (sch, _, specs) = cell("cmos_inverter");
        let s = suggest(&sch, &Tech::generic(), &specs);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s.iter().all(|s| s.param == "w" && s.role == Role::Device));
        let (sch, _, specs) = cell("current_mirror");
        let s = suggest(&sch, &Tech::generic(), &specs);
        assert!(
            s.iter().any(|s| s.var.target == "M2.w" && s.role == Role::MirrorOutput),
            "a current spec frees the ratio: {s:?}"
        );
        let n = derived_note("work/ota5t", &[Spec { name: "gain_db".into(), min: "40".into(), ..Default::default() }]);
        assert_eq!(n, "Derived from work/ota5t for: gain_db >= 40.");
    }
}
