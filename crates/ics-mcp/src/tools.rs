//! MCP tools over one in-memory IC Studio project.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use ics_core::drc::{DrcReport, parse_any, run as run_drc_deck};
use ics_core::extract::{Extraction, extract};
use ics_core::hier::{Hier, LayoutSource, descendants, would_cycle};
use ics_core::layout::{Geometry, Layout, Trans};
use ics_core::lvs::{LvsOptions, LvsReport, compare};
use ics_core::pcell::{PCELL_LIB, PCell};
use ics_core::project::{Library, MANIFEST};
use ics_core::{Backend, Cell, CellRef, IRect, Project, Tech, sdl};
use ics_sim::{Dataset, SimOptions, SimResults};
use serde_json::{Map, Value, json};

const MAX_LISTED: usize = 500;

/// Read-only tools that can take long on large designs. Live mode runs them on a worker
/// thread against a [`snapshot`] so the window stays responsive.
pub const BACKGROUND: [&str; 14] = [
    "get_netlist",
    "simulate",
    "run_spice",
    "layout_summary",
    "flylines",
    "run_drc",
    "extract",
    "run_lvs",
    "run_signoff",
    "post_layout_parasitics",
    "emir",
    "export_gds",
    "export_lef",
    "export_def",
];

/// Verification results behind a live-mode run_drc / extract / run_lvs call, for the app to show.
pub enum Verified {
    Drc(DrcReport),
    Extract(Extraction),
    Lvs(Extraction, LvsReport),
    Signoff(Result<ics_core::signoff::SignoffReport, String>),
    Emir(Result<ics_core::emir::EmirReport, String>),
}

/// A copy of the part of `p` that `cell` depends on: the cell itself, every cell its layout
/// places (recursively), the technology and the settings. `layout` stands in for the cell's
/// own layout, e.g. while an editor holds it. Without a cell, the copy has no cells at all.
pub fn snapshot(p: &Project, cell: Option<&CellRef>, layout: Option<&Layout>) -> Project {
    let top_layout = cell.and_then(|c| layout.or_else(|| p.layout(c)));
    let mut keep: HashSet<CellRef> = cell.into_iter().cloned().collect();
    let mut todo: Vec<&Layout> = top_layout.into_iter().collect();
    while let Some(l) = todo.pop() {
        for i in &l.instances {
            if keep.insert(i.cell.clone())
                && let Some(child) = p.layout(&i.cell)
            {
                todo.push(child);
            }
        }
    }
    let libraries = p
        .libraries
        .iter()
        .filter_map(|lib| {
            let cells: Vec<Cell> = lib
                .cells
                .iter()
                .filter_map(|c| {
                    let r = CellRef::new(&lib.name, &c.name);
                    if !keep.contains(&r) {
                        return None;
                    }
                    Some(match layout.filter(|_| Some(&r) == cell) {
                        Some(l) => Cell {
                            name: c.name.clone(),
                            description: c.description.clone(),
                            schematic: c.schematic.clone(),
                            layout: Some(l.clone()),
                            sim: c.sim.clone(),
                            pcell: c.pcell.clone(),
                        },
                        None => c.clone(),
                    })
                })
                .collect();
            (!cells.is_empty()).then(|| Library { name: lib.name.clone(), cells, storage: lib.storage })
        })
        .collect();
    Project {
        root: p.root.clone(),
        name: p.name.clone(),
        description: p.description.clone(),
        tech: p.tech.clone(),
        libraries,
        settings: p.settings.clone(),
        disk: p.disk.clone(),
    }
}

#[derive(Default)]
pub struct State {
    project: Option<Project>,
    dirty: bool,
    in_gui: bool,
    verified: Option<Verified>,
}

struct Args<'a>(&'a Map<String, Value>);

impl Args<'_> {
    fn get(&self, k: &str) -> Option<&Value> {
        self.0.get(k).filter(|v| !v.is_null())
    }

    fn str(&self, k: &str) -> Result<&str, String> {
        self.opt_str(k)?.ok_or_else(|| format!("missing string argument '{k}'"))
    }

    fn opt_str(&self, k: &str) -> Result<Option<&str>, String> {
        self.get(k).map(|v| v.as_str().ok_or_else(|| format!("argument '{k}' must be a string"))).transpose()
    }

    fn num(&self, k: &str) -> Result<f64, String> {
        self.opt_num(k)?.ok_or_else(|| format!("missing number argument '{k}'"))
    }

    fn opt_num(&self, k: &str) -> Result<Option<f64>, String> {
        self.get(k).map(|v| v.as_f64().ok_or_else(|| format!("argument '{k}' must be a number"))).transpose()
    }

    fn opt_bool(&self, k: &str) -> Result<Option<bool>, String> {
        self.get(k).map(|v| v.as_bool().ok_or_else(|| format!("argument '{k}' must be a boolean"))).transpose()
    }
}

fn um(tech: &Tech, v: i64) -> f64 {
    v as f64 / tech.dbu_per_um.max(1) as f64
}

/// µm to dbu, snapped to the technology grid.
fn dbu(tech: &Tech, v: f64) -> i64 {
    let g = tech.grid.max(1) as f64;
    ((v * tech.dbu_per_um as f64 / g).round() * g) as i64
}

fn pt_um(tech: &Tech, p: (i64, i64)) -> Value {
    json!([um(tech, p.0), um(tech, p.1)])
}

fn rect_um(tech: &Tech, r: IRect) -> Value {
    json!([um(tech, r.x0), um(tech, r.y0), um(tech, r.x1), um(tech, r.y1)])
}

/// Round to 6 significant digits to keep results compact.
fn sig(v: f64) -> Value {
    if v.is_finite() { json!(format!("{v:.5e}").parse::<f64>().unwrap_or(v)) } else { Value::Null }
}

fn layer_name(tech: &Tech, l: usize) -> String {
    tech.layer(l).map_or_else(|| format!("#{l}"), |d| d.name.clone())
}

fn layer_index(tech: &Tech, name: &str) -> Result<usize, String> {
    tech.layer_index(name).ok_or_else(|| {
        let names: Vec<&str> = tech.layers.iter().map(|l| l.name.as_str()).collect();
        format!("unknown layer '{name}'; layers: {}", names.join(", "))
    })
}

fn resolve(p: &Project, cell: &str) -> Result<CellRef, String> {
    find(p, cell).ok_or_else(|| format!("no cell '{cell}' (use project_info to list cells)"))
}

/// Look up a cell given as `cell` or `lib/cell`.
pub fn find(p: &Project, cell: &str) -> Option<CellRef> {
    match cell.split_once('/') {
        Some((l, c)) => {
            let r = CellRef::new(l, c);
            p.cell(&r).map(|_| r)
        }
        None => p.all_cells().find(|(_, c)| c.name == cell).map(|(r, _)| r),
    }
}

fn layout_of<'p>(p: &'p Project, r: &CellRef) -> Result<&'p Layout, String> {
    p.layout(r).ok_or_else(|| format!("cell '{r}' has no layout view"))
}

fn backend(a: &Args) -> Result<Option<Backend>, String> {
    match a.opt_str("backend")? {
        None => Ok(None),
        Some("builtin") => Ok(Some(Backend::Builtin)),
        Some("ngspice") => Ok(Some(Backend::Ngspice)),
        Some("auto") => Ok(Some(Backend::Auto)),
        Some(b) => Err(format!("unknown backend '{b}' (use builtin, ngspice or auto)")),
    }
}

fn sim_options(p: Option<&Project>, backend: Backend) -> SimOptions {
    // Relative `.include` / `.lib` paths are relative to the project.
    let mut opts = SimOptions { backend, deck_dir: p.and_then(|p| p.root.clone()), ..Default::default() };
    if let Some(exe) = p.and_then(|p| p.trusted_ngspice()) {
        opts.ngspice_exe = exe;
    }
    opts
}

fn stats(v: &[f64]) -> Value {
    let (min, max) = v.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| (a.min(x), b.max(x)));
    json!({"min": sig(min), "max": sig(max), "first": sig(v.first().copied().unwrap_or(f64::NAN)), "final": sig(v.last().copied().unwrap_or(f64::NAN))})
}

fn dataset_json(d: &Dataset) -> Value {
    if d.analysis == "op" {
        let values: Map<String, Value> =
            d.vectors.iter().map(|v| (v.name.clone(), sig(v.re.first().copied().unwrap_or(f64::NAN)))).collect();
        let devices: Vec<Value> = d
            .devices
            .iter()
            .map(|dev| {
                let vals: Map<String, Value> = dev.values.iter().map(|(k, v)| (k.clone(), sig(*v))).collect();
                json!({"name": dev.name, "region": dev.region, "values": vals})
            })
            .collect();
        return json!({"analysis": d.analysis, "title": d.title, "values": values, "devices": devices});
    }
    let scale = d.scale_vector().map(|s| {
        let (a, b) = (s.re.first().copied().unwrap_or(f64::NAN), s.re.last().copied().unwrap_or(f64::NAN));
        json!({"name": s.name, "unit": s.kind.unit(), "from": sig(a), "to": sig(b)})
    });
    let vectors: Map<String, Value> = d
        .signals()
        .map(|v| {
            let mut s = match &v.im {
                Some(_) => {
                    let mut s = stats(&v.db());
                    s["unit"] = json!("dB");
                    s["final_phase_deg"] = sig(v.phase_deg().last().copied().unwrap_or(f64::NAN));
                    s
                }
                None => stats(&v.re),
            };
            if v.im.is_none() {
                s["unit"] = json!(v.kind.unit());
            }
            (v.name.clone(), s)
        })
        .collect();
    json!({"analysis": d.analysis, "title": d.title, "points": d.len(), "scale": scale, "vectors": vectors})
}

fn results_json(res: &SimResults) -> Value {
    let lines: Vec<&str> = res.log.lines().collect();
    let tail = lines[lines.len().saturating_sub(20)..].join("\n");
    json!({
        "backend": res.backend,
        "elapsed_ms": sig(res.elapsed_ms),
        "datasets": res.datasets.iter().map(dataset_json).collect::<Vec<_>>(),
        "warnings": res.warnings,
        "log_tail": tail,
    })
}

/// Currents per layout device name for [`ics_core::emir`]: the drain current magnitudes of an
/// operating-point simulation of the cell's schematic (built-in engine), matched via LVS.
pub fn op_device_currents(p: &Project, r: &CellRef) -> Result<BTreeMap<String, f64>, String> {
    let c = p.cell(r).ok_or_else(|| format!("no cell '{r}'"))?;
    let sch = c.schematic.as_ref().ok_or("cell has no schematic to simulate; give the device currents")?;
    let deck = ics_core::netlist::netlist_for(sch, &p.tech, &c.sim.corner).deck(&r.to_string(), &c.sim);
    let res = ics_sim::simulate(&deck, &[ics_core::Analysis::Op], &sim_options(Some(p), Backend::Builtin))
        .map_err(|e| format!("operating point: {e}"))?;
    let by_element: BTreeMap<String, f64> = res
        .dataset("op")
        .into_iter()
        .flat_map(|d| &d.devices)
        .filter_map(|d| d.values.iter().find(|(k, _)| k == "id").map(|(_, v)| (d.name.clone(), *v)))
        .collect();
    ics_core::emir::layout_currents(p, r, &by_element)
}

/// An IR drop / EM report as JSON, listing up to `max` branches and violations.
/// Optional OPC settings; lengths in µm.
#[derive(Debug, Clone, Default)]
pub struct OpcOverrides {
    pub sigma: Option<f64>,
    pub threshold: Option<f64>,
    pub fragment: Option<f64>,
    pub max_out: Option<f64>,
    pub max_in: Option<f64>,
    pub iterations: Option<u32>,
    pub gain: Option<f64>,
}

/// OPC parameters for `layer`: the defaults of [`ics_core::opc::OpcParams::for_layer`] with the
/// given overrides.
pub fn opc_params(tech: &Tech, layer: usize, o: &OpcOverrides) -> ics_core::opc::OpcParams {
    let mut p = ics_core::opc::OpcParams::for_layer(tech, layer);
    let dbu = |v: f64| (v * tech.dbu_per_um as f64).round() as i64;
    for (v, f) in [(o.sigma, &mut p.sigma), (o.fragment, &mut p.fragment), (o.max_out, &mut p.max_out), (o.max_in, &mut p.max_in)]
    {
        if let Some(v) = v {
            *f = dbu(v);
        }
    }
    p.threshold = o.threshold.unwrap_or(p.threshold);
    p.gain = o.gain.unwrap_or(p.gain);
    p.iterations = o.iterations.unwrap_or(p.iterations);
    p
}

/// An OPC result: EPE statistics before / after (nm), mask area and the `max` worst fragments.
pub fn opc_json(rep: &ics_core::opc::OpcReport, p: &ics_core::opc::OpcParams, tech: &Tech, max: usize) -> Value {
    let nm = |v: f64| (v * 1000.0 / tech.dbu_per_um.max(1) as f64 * 100.0).round() / 100.0;
    let um2 = |a: i128| a as f64 / (tech.dbu_per_um.max(1) as f64).powi(2);
    let n = rep.fragments.len();
    let stats = |s: &ics_core::opc::EpeStats| json!({"rms_nm": nm(s.rms), "max_nm": nm(s.max), "on_target": s.within});
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| rep.fragments[b].epe_after.abs().total_cmp(&rep.fragments[a].epe_after.abs()));
    let worst: Vec<Value> = order
        .iter()
        .take(max)
        .map(|&i| {
            let f = &rep.fragments[i];
            json!({
                "at_um": [um(tech, f.a.0 + f.b.0) / 2.0, um(tech, f.a.1 + f.b.1) / 2.0],
                "length_um": um(tech, (f.b.0 - f.a.0).abs() + (f.b.1 - f.a.1).abs()),
                "normal": [f.normal.0, f.normal.1],
                "offset_nm": nm(f.offset as f64),
                "epe_before_nm": nm(f.epe_before),
                "epe_after_nm": nm(f.epe_after),
            })
        })
        .collect();
    json!({
        "layer": layer_name(tech, p.layer),
        "params": {
            "sigma_um": um(tech, p.sigma),
            "threshold": p.threshold,
            "fragment_um": um(tech, p.fragment),
            "max_out_um": um(tech, p.max_out),
            "max_in_um": um(tech, p.max_in),
            "iterations": p.iterations,
            "gain": p.gain,
            "tolerance_nm": nm(p.tolerance as f64),
        },
        "fragments": n,
        "iterations": rep.iterations,
        "epe_before": stats(&rep.before),
        "epe_after": stats(&rep.after),
        "drawn_area_um2": um2(rep.target.area()),
        "mask_area_um2": um2(rep.mask.area()),
        "mask_shapes": rep.mask.to_polygons().len(),
        "worst_fragments": worst,
    })
}

pub fn emir_json(rep: &ics_core::emir::EmirReport, tech: &Tech, max: usize) -> Value {
    use ics_core::emir::BranchKind;
    let kind = |k: BranchKind| match k {
        BranchKind::Wire { layer, .. } => layer_name(tech, layer),
        BranchKind::Via { via } => tech.vias.get(via).map_or_else(String::new, |v| v.name.clone()),
    };
    let unit = |k: BranchKind| if matches!(k, BranchKind::Via { .. }) { "A/cut" } else { "A/um" };
    let mut order: Vec<usize> = (0..rep.branches.len()).collect();
    let ratio = |i: usize| {
        let b = &rep.branches[i];
        if b.limit > 0.0 { b.density / b.limit } else { 0.0 }
    };
    order.sort_by(|&i, &j| ratio(j).total_cmp(&ratio(i)).then(rep.branches[j].density.total_cmp(&rep.branches[i].density)));
    let branches: Vec<Value> = order
        .into_iter()
        .take(max)
        .map(|i| {
            let b = &rep.branches[i];
            json!({"layer": kind(b.kind), "at_um": rect_um(tech, b.at), "amps": sig(b.current.abs()), "density": sig(b.density),
                   "limit": sig(b.limit), "unit": unit(b.kind)})
        })
        .collect();
    let violations: Vec<Value> = rep
        .violations
        .iter()
        .take(max)
        .map(|v| {
            json!({"layer": v.layer, "at_um": rect_um(tech, v.at), "density": sig(v.density), "limit": sig(v.limit),
                        "unit": unit(rep.branches[v.branch].kind)})
        })
        .collect();
    let loads: Vec<Value> = rep
        .loads
        .iter()
        .take(MAX_LISTED)
        .map(|l| json!({"device": l.device, "amps": sig(l.amps), "drop_v": sig(rep.nodes[l.node].drop)}))
        .collect();
    let worst = rep.worst.map(|w| {
        let n = &rep.nodes[w];
        json!({"drop_v": sig(n.drop), "layer": layer_name(tech, n.layer), "at_um": rect_um(tech, n.rect)})
    });
    json!({
        "net": rep.net,
        "summary": rep.summary(tech),
        "worst": worst,
        "total_current_a": sig(rep.total_current),
        "nodes": rep.nodes.len(),
        "resistors": rep.branches.len(),
        "pads": rep.pads.len(),
        "loads": loads,
        "em_violations": rep.violations.len(),
        "violations": violations,
        "highest_density": branches,
        "warnings": rep.warnings,
    })
}

fn extraction_json(x: &Extraction, tech: &Tech, title: &str) -> Value {
    let net = |i: usize| x.nets.get(i).map_or("?", |n| n.name.as_str());
    let nets: Vec<Value> = x
        .nets
        .iter()
        .take(MAX_LISTED)
        .map(|n| json!({"name": n.name, "labelled": n.labelled, "bbox_um": n.bbox.map(|b| rect_um(tech, b))}))
        .collect();
    let devices: Vec<Value> = x
        .devices
        .iter()
        .take(MAX_LISTED)
        .map(|d| {
            json!({
                "name": d.name, "kind": d.kind.model(), "w_um": um(tech, d.w), "l_um": um(tech, d.l),
                "d": net(d.d), "g": net(d.g), "s": net(d.s), "b": net(d.b),
            })
        })
        .collect();
    let passives: Vec<Value> = x
        .passives
        .iter()
        .take(MAX_LISTED)
        .map(|p| json!({"name": p.name, "kind": p.kind.label(), "model": p.model, "value": sig(p.value), "a": net(p.a), "b": net(p.b)}))
        .collect();
    json!({
        "net_count": x.nets.len(),
        "device_count": x.devices.len(),
        "nets": nets,
        "devices": devices,
        "passives": passives,
        "warnings": x.warnings,
        "spice": x.spice(title, tech.dbu_per_um),
    })
}

fn pcell_from(kind: &str, params: &Args, tech: &Tech) -> Result<PCell, String> {
    let len = |k: &str, default: f64| -> Result<i64, String> {
        let v = params.opt_num(k)?.unwrap_or(default);
        if v <= 0.0 {
            return Err(format!("parameter '{k}' must be > 0"));
        }
        Ok(dbu(tech, v))
    };
    Ok(match kind {
        "nmos" | "pmos" => {
            let pmos = kind == "pmos";
            let fingers = params.opt_num("fingers")?.unwrap_or(1.0);
            if fingers < 1.0 || fingers.fract() != 0.0 || fingers > 1000.0 {
                return Err("parameter 'fingers' must be an integer from 1 to 1000".into());
            }
            PCell::Mos {
                pmos,
                w: len("w", if pmos { 2.0 } else { 1.0 })?,
                l: len("l", 0.18)?,
                fingers: fingers as u32,
                device: params.opt_str("device")?.unwrap_or_default().to_string(),
            }
        }
        "poly_res" => PCell::PolyRes { w: len("w", 0.4)?, l: len("l", 5.0)? },
        "mom_cap" => PCell::MomCap { w: len("w", 5.0)?, h: len("h", 5.0)? },
        "ntap" | "ptap" => PCell::Tap { well: kind == "ntap", w: len("w", 1.0)? },
        "nring_path" | "pring_path" => {
            let pts =
                params.get("points").and_then(Value::as_array).ok_or("parameter 'points' must be an array of [x, y] in µm")?;
            let points = pts
                .iter()
                .map(|p| match p.as_array().map(|v| v.iter().filter_map(Value::as_f64).collect::<Vec<_>>()).as_deref() {
                    Some([x, y]) => Ok((dbu(tech, *x), dbu(tech, *y))),
                    _ => Err("each point must be [x, y] in µm".to_string()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let width = params.opt_num("width")?.map_or(0, |v| dbu(tech, v.max(0.0)));
            PCell::RingPath { well: kind == "nring_path", points, closed: params.opt_bool("closed")?.unwrap_or(false), width }
        }
        "nring" | "pring" => PCell::GuardRing { well: kind == "nring", w: len("w", 5.0)?, h: len("h", 5.0)?, width: 0, gap: 0 },
        k => {
            return Err(format!(
                "unknown pcell kind '{k}' (use nmos, pmos, poly_res, mom_cap, ntap, ptap, nring, pring, nring_path or pring_path)"
            ));
        }
    })
}

/// An analysis from tool arguments; numbers are accepted where the setup stores SPICE strings.
fn parse_analysis(v: &Value) -> Result<ics_core::sim_setup::Analysis, String> {
    let mut obj = v.as_object().ok_or("each analysis must be an object with a 'type'")?.clone();
    for (k, val) in obj.iter_mut() {
        match (k.as_str(), &*val) {
            ("points_per_decade", Value::String(s)) => {
                let n: u32 = s.trim().parse().map_err(|_| format!("points_per_decade '{s}' is not an integer"))?;
                *val = json!(n);
            }
            ("points_per_decade" | "type", _) => {}
            (_, Value::Number(n)) => *val = json!(n.to_string()),
            _ => {}
        }
    }
    let a: ics_core::sim_setup::Analysis =
        serde_json::from_value(Value::Object(obj)).map_err(|e| format!("invalid analysis {v}: {e}"))?;
    a.validate()?;
    Ok(a)
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
    })
}

fn cell_prop() -> Value {
    json!({"type": "string", "description": "Cell name, or 'lib/cell' when the name is ambiguous."})
}

fn num_prop(d: &str) -> Value {
    json!({"type": "number", "description": d})
}

const NOT_SAVED: &str = "Changes stay in memory until save_project is called.";

/// A cell's simulation with its saved setup.
pub struct CellSim {
    pub results: SimResults,
    pub deck: String,
    /// Netlister warnings.
    pub warnings: Vec<String>,
    /// Summary of the parasitics included (post-layout).
    pub parasitics: Option<String>,
}

/// Simulate cell `r` with its saved analyses, with its layout parasitics when `parasitics` or
/// the setup asks for them; `backend` replaces the setup's.
pub fn simulate_cell(p: &Project, r: &CellRef, parasitics: bool, backend: Option<Backend>) -> Result<CellSim, String> {
    let c = p.cell(r).ok_or_else(|| format!("no cell {r}"))?;
    let sch = c.schematic.as_ref().ok_or("cell has no schematic view")?;
    let nl = ics_core::netlist::netlist_for(sch, &p.tech, &c.sim.corner);
    let mut deck = nl.deck(&r.to_string(), &c.sim);
    let mut summary = None;
    if parasitics || c.sim.parasitics {
        let pl = ics_core::pex::post_layout(p, r)?;
        deck = pl.apply(&deck);
        summary = Some(pl.summary);
    }
    let opts = sim_options(Some(p), backend.unwrap_or(c.sim.backend));
    let analyses: Vec<_> = c.sim.enabled().cloned().collect();
    let results = ics_sim::simulate(&deck, &analyses, &opts).map_err(|e| e.to_string())?;
    Ok(CellSim { results, deck, warnings: nl.warnings, parasitics: summary })
}

pub fn definitions() -> Vec<Value> {
    let cell = cell_prop;
    let edit_cell = || json!({"type": "string", "description": "Target cell name or 'lib/cell'. Created (in library 'work' unless a lib is given) if it does not exist; a layout view is added if missing."});
    let backend = json!({"type": "string", "enum": ["builtin", "ngspice", "auto"], "description": "Simulator: the built-in engine (default unless the cell's setup says otherwise), an external ngspice, or auto (built-in unless the deck needs ngspice)."});
    vec![
        tool(
            "open_project",
            "Open an IC Studio project directory (the folder containing ic-studio.json) and make it the current project. Unsaved changes to a previously open project are discarded. Returns the same overview as project_info.",
            json!({"path": {"type": "string", "description": "Project directory (or the path of its ic-studio.json)."}}),
            &["path"],
        ),
        tool(
            "new_project",
            "Create a new project directory, save it to disk immediately and make it the current project. Template 'demo' (default) contains example cells: cmos_inverter (schematic + layout), rc_lowpass, diode_clipper, inv_chain/inv_array/inv_tiles (layout hierarchy), devices (PCells). 'empty' has only an empty 'work' library.",
            json!({
                "path": {"type": "string", "description": "Directory to create; must not already hold a project."},
                "template": {"type": "string", "enum": ["empty", "demo"], "description": "Starting content (default demo)."},
            }),
            &["path"],
        ),
        tool("save_project", "Write the current project to its directory on disk.", json!({}), &[]),
        tool(
            "load_pdk",
            "Replace the current project's technology with a PDK file (TOML with layers, vias, layer roles, MOS devices and the SPICE model library, e.g. pdks/sky130A.toml). Layout shapes keep their GDS numbers, PCells are regenerated; extraction, LVS and netlists then use the PDK device names and models. ${VAR} in its paths come from the server's environment. Returns the new technology and warnings.",
            json!({"path": {"type": "string", "description": "PDK file path."}}),
            &["path"],
        ),
        tool(
            "project_info",
            "Overview of the current project: libraries and cells with their views (schematic, layout), technology name, database units per µm, grid, layer names and via names. Call this first to learn cell and layer names.",
            json!({}),
            &[],
        ),
        tool(
            "get_netlist",
            "Netlist generated from a cell's schematic: a SPICE deck including the analyses of its saved simulation setup (default), the same deck in Spectre, or CDL for LVS.",
            json!({"cell": cell(), "format": {"type": "string", "enum": ["spice", "spectre", "cdl"], "description": "Netlist language (default spice)."}}),
            &["cell"],
        ),
        tool(
            "import_cdl",
            &format!(
                "Import a hierarchical CDL netlist as Cadence Virtuoso writes it (auCdl: .SUBCKT with *.PININFO, X instances with '/ cell', subcircuit parameters and expressions, .GLOBAL and '!' global nets, $ annotations) as the flat schematic of one cell. The top subcircuit is the one no other instantiates unless 'top' is given; instance paths become element names (MI0/M0 is MM0 inside XI0) and local nets get the path (I0/net7). Returns the top, its pins with directions, the device count, all subcircuits and warnings. {NOT_SAVED}"
            ),
            json!({
                "cell": {"type": "string", "description": "Cell to create or replace (default work/<top>)."},
                "cdl": {"type": "string", "description": "CDL text."},
                "path": {"type": "string", "description": "A CDL file instead of the text."},
                "top": {"type": "string", "description": "Subcircuit to import."},
                "overwrite": {"type": "boolean", "description": "Replace an existing schematic (default false)."}
            }),
            &[],
        ),
        tool(
            "import_virtuoso",
            &format!(
                "Import a Cadence Virtuoso library dump written by icsDumpLib (the SKILL script icsDump.il; 'ics dump-skill' prints it): every schematic becomes a cell (instances matched to devices through a symbol map, analogLib by default; instances of other dumped schematics flattened with instance paths; the Virtuoso placement kept when it gives the same connectivity) and every layout its cell's layout (shapes on technology layers, instances, mosaics, masters from other libraries and PCell variants as cells of their own). Existing views are kept unless overwrite. Returns the cells with what was imported and warnings. {NOT_SAVED}"
            ),
            json!({
                "dump": {"type": "string", "description": "Dump text."},
                "path": {"type": "string", "description": "A dump file instead of the text; or a cds.lib, a directory with one, or a library directory, which Virtuoso (ICS_VIRTUOSO or on the PATH) dumps in batch mode."},
                "lib": {"type": "string", "description": "Library to import when path is a cds.lib."},
                "map": {"type": "string", "description": "Symbol map TOML ([[symbol]] entries with kind, model, lib, cell, terms, params)."},
                "scale": {"type": "number", "description": "Virtuoso user units per schematic grid point (default 0.1875)."},
                "overwrite": {"type": "boolean", "description": "Replace existing schematics and layouts (default false)."}
            }),
            &[],
        ),
        tool(
            "export_skill_schematic",
            "Write a cell's schematic as a SKILL script that recreates it in Cadence Virtuoso (load it in the CIW). Devices become analogLib symbols (nmos4, pmos4, res, cap, ind, vdc, idc, diode) or the symbols of a map; every terminal gets a wire stub and a wire label with its net at the symbol's pin, found when the script runs. Returns the script (and writes it to path when given) with warnings for devices without a symbol.",
            json!({
                "cell": cell(),
                "path": {"type": "string", "description": "Write the script to this file."},
                "lib": {"type": "string", "description": "Virtuoso library (default: the cell's library)."},
                "map": {"type": "string", "description": "Symbol map TOML: [[symbol]] entries with kind, model (optional), lib, cell, view, terms = { d = \"D\", ... }, params = { w = \"w\", ... }."},
                "scale": {"type": "number", "description": "Virtuoso user units per schematic grid point (default 0.1875)."}
            }),
            &["cell"],
        ),
        tool(
            "write_schematic",
            "Create or replace a cell's schematic from a flat SPICE netlist (R, C, L, V, I, D, M lines; X lines for PDK MOS subcircuits). Node 0 or gnd is ground. Every pin gets a net label and devices are laid out in rows (PMOS, NMOS, passives, sources). Analysis lines (.op, .dc, .ac dec, .tran) and .temp set the cell's simulation setup; other dot lines (.model, .include, .param, .ic, .meas) become its extra lines. Lengths are SPICE metres (w=2u). Returns the generated deck to check. Changes stay in memory until save_project is called.",
            json!({
                "cell": {"type": "string", "description": "Cell name or 'lib/cell'. Created (in library 'work' unless a lib is given) if it does not exist."},
                "netlist": {"type": "string", "description": "SPICE netlist, one element per line, e.g. 'M1 out in vdd vdd pmos w=2u l=180n', 'M2 out in 0 0 nmos w=1u l=180n', 'V1 vdd 0 1.8', 'V2 in 0 PULSE(0 1.8 1n 50p 50p 2n 4n)', '.tran 10p 10n'. MOS: d g s b model w= l= [m=] [match=]."},
                "overwrite": {"type": "boolean", "description": "Replace an existing schematic (default false: refuse)."},
                "description": {"type": "string", "description": "Cell description."},
                "pins": {"type": "object", "additionalProperties": {"type": "string", "enum": ["input", "output", "inout"]}, "description": "Schematic pins (cell ports) by net name and direction, e.g. {\"in\": \"input\", \"out\": \"output\"}. They become the CDL subcircuit ports and the nets checked for layout pins."},
            }),
            &["cell", "netlist"],
        ),
        tool(
            "set_simulation",
            "Change a cell's simulation setup; only the given fields change. 'analyses' lists the analyses to run (the others are disabled). Returns the resulting setup. Changes stay in memory until save_project is called.",
            json!({
                "cell": cell(),
                "analyses": {"type": "array", "items": {"type": "object"}, "description": "E.g. [{\"type\":\"op\"}, {\"type\":\"dc\",\"source\":\"V2\",\"start\":\"0\",\"stop\":\"1.8\",\"step\":\"10m\"}, {\"type\":\"ac\",\"points_per_decade\":20,\"fstart\":\"1\",\"fstop\":\"10g\"}, {\"type\":\"tran\",\"tstep\":\"10p\",\"tstop\":\"20n\"}]. Values are SPICE numbers."},
                "backend": backend.clone(),
                "temp": num_prop("Temperature in °C."),
                "extra": {"type": "string", "description": "Raw SPICE lines appended to the deck (.model, .include, .ic, .meas, ...); replaces the current ones."},
                "parasitics": {"type": "boolean", "description": "Include layout parasitics in every simulation of the cell."},
                "corner": {"type": "string", "description": "PDK model corner (e.g. tt, ff, ss); empty for the PDK default."},
            }),
            &["cell"],
        ),
        tool(
            "simulate",
            "Simulate a cell's schematic with its saved analyses (op/dc/ac/tran). Returns per analysis a compact summary, not full waveforms: OP node values and device operating points; for sweeps the scale range plus min/max/first/final of every vector (AC vectors in dB with final phase).",
            json!({
                "cell": cell(),
                "backend": backend.clone(),
                "parasitics": {"type": "boolean", "description": "Add layout wire capacitances extracted from the cell's layout (post-layout simulation). Also enabled by the cell's setup."},
            }),
            &["cell"],
        ),
        tool(
            "run_spice",
            "Simulate a raw SPICE deck using its own dot analyses (.op/.dc/.ac/.tran). Built-in models 'nmos'/'pmos' are available. Returns the same compact summary as simulate.",
            json!({"deck": {"type": "string", "description": "Full SPICE deck text."}, "backend": backend}),
            &["deck"],
        ),
        tool(
            "layout_summary",
            "Summary of a cell layout: hierarchical bounding box (µm), shape counts per layer, labels, and placed instances (id, name, cell, position µm, rotation, mirror). Set list_shapes to also list this cell's own shapes with ids and geometry, e.g. to pick ids for delete_items.",
            json!({"cell": cell(), "list_shapes": {"type": "boolean", "description": "List shapes with ids (up to 500)."}}),
            &["cell"],
        ),
        tool(
            "add_rect",
            &format!(
                "Add a rectangle on a layer. Coordinates in µm (layout y points up), snapped to the technology grid. Returns the new shape id. {NOT_SAVED}"
            ),
            json!({
                "cell": edit_cell(),
                "layer": {"type": "string", "description": "Layer name (see project_info)."},
                "x0": num_prop("Left/any corner x (µm)."), "y0": num_prop("Corner y (µm)."),
                "x1": num_prop("Opposite corner x (µm)."), "y1": num_prop("Opposite corner y (µm)."),
            }),
            &["cell", "layer", "x0", "y0", "x1", "y1"],
        ),
        tool(
            "add_path",
            &format!(
                "Add a wire (path) of a given width through points, e.g. a metal route. Keep segments horizontal/vertical so DRC and extraction see them. Coordinates in µm, snapped to grid. {NOT_SAVED}"
            ),
            json!({
                "cell": edit_cell(),
                "layer": {"type": "string", "description": "Layer name."},
                "points": {"type": "array", "minItems": 2, "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "Centre-line points [[x, y], ...] in µm."},
                "width": num_prop("Wire width in µm."),
            }),
            &["cell", "layer", "points", "width"],
        ),
        tool(
            "add_label",
            &format!(
                "Add a text label. A label on a routing layer (e.g. metal1) names the net under it for extraction and LVS. {NOT_SAVED}"
            ),
            json!({
                "cell": edit_cell(),
                "layer": {"type": "string", "description": "Layer name (a routing layer to name a net, or 'text')."},
                "text": {"type": "string", "description": "Label text / net name."},
                "x": num_prop("Anchor x (µm)."), "y": num_prop("Anchor y (µm)."),
                "size": num_prop("Text height in µm (default 0.2)."),
            }),
            &["cell", "layer", "text", "x", "y"],
        ),
        tool(
            "place_instance",
            &format!(
                "Place an instance of another cell's layout. The transform mirrors about the x axis first (if mirror), then rotates counter-clockwise, then moves the child origin to (x, y). {NOT_SAVED}"
            ),
            json!({
                "cell": edit_cell(),
                "child": {"type": "string", "description": "Cell to place (name or 'lib/cell'); must have a layout."},
                "x": num_prop("Origin x (µm)."), "y": num_prop("Origin y (µm)."),
                "rot": {"type": "integer", "enum": [0, 90, 180, 270], "description": "Counter-clockwise rotation in degrees (default 0)."},
                "mirror": {"type": "boolean", "description": "Mirror about the x axis before rotating."},
                "name": {"type": "string", "description": "Instance name, e.g. 'M1' to tie it to a schematic device."},
            }),
            &["cell", "child", "x", "y"],
        ),
        tool(
            "place_pcell",
            &format!(
                "Generate a parametric device (variant cell in the 'pcells' library, reused if it exists) and place it. MOS terminals carry labels D/G/S. Name MOS instances after their schematic devices (e.g. 'M1') for LVS and flylines. {NOT_SAVED}"
            ),
            json!({
                "cell": edit_cell(),
                "kind": {"type": "string", "enum": ["nmos", "pmos", "poly_res", "mom_cap", "ntap", "ptap", "nring", "pring", "nring_path", "pring_path"]},
                "params": {
                    "type": "object",
                    "description": "Sizes in µm. nmos/pmos: w (per finger, default 1 / 2), l (default 0.18), fingers (default 1), device (PDK device name, e.g. a low-Vt model; default: the technology's default device). poly_res: w (0.4), l (5). mom_cap: w (5), h (5). ntap/ptap: w (tap height, 1). nring/pring: guard ring around a w x h area (5 x 5). nring_path/pring_path: guard ring along the Manhattan centre line points ([[x, y], ...] µm, relative to the origin), closed into a loop when closed is true; width (0 = minimum).",
                    "properties": {"w": {"type": "number"}, "l": {"type": "number"}, "h": {"type": "number"}, "fingers": {"type": "integer", "minimum": 1}, "device": {"type": "string"}, "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}}, "closed": {"type": "boolean"}, "width": {"type": "number"}},
                    "additionalProperties": false,
                },
                "x": num_prop("Origin x (µm)."), "y": num_prop("Origin y (µm)."),
                "rot": {"type": "integer", "enum": [0, 90, 180, 270], "description": "Counter-clockwise rotation in degrees."},
                "mirror": {"type": "boolean", "description": "Mirror about the x axis before rotating."},
                "name": {"type": "string", "description": "Instance name, e.g. 'M1'."},
            }),
            &["cell", "kind", "x", "y"],
        ),
        tool(
            "delete_items",
            &format!(
                "Delete shapes and/or instances of a cell layout by id (ids from layout_summary or the add/place tools). {NOT_SAVED}"
            ),
            json!({"cell": cell(), "ids": {"type": "array", "items": {"type": "integer"}, "minItems": 1, "description": "Shape or instance ids."}}),
            &["cell", "ids"],
        ),
        tool(
            "generate_layout",
            &format!(
                "Schematic-driven layout: place every schematic MOS device that is not yet in the layout as a PCell instance named after it (PMOS row above NMOS row, DRC-clean spacing, shared diffusion, a bulk tap per row). Differential pairs, current mirrors and devices with the same 'match' parameter are split into unit fingers named <device>.<k> in common-centroid order (two cross-coupled rows from 8 units or with match_rows=2), closed by off dummy fingers (dummy:...) that LVS filters; reported under 'matched'. Sources, R and C are skipped. Wiring is not done: call route, or use flylines to see what to connect. {NOT_SAVED}"
            ),
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "flylines",
            "Unrouted connections: for each schematic net, segments between device terminals (D/G/S of named MOS instances) that are not yet connected in the layout. Empty when the layout is fully wired.",
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "route",
            &format!(
                "Maze-route the flylines of a cell: connect the device terminals of every schematic net that are not yet connected, with DRC-clean wires and vias on the metal stack (metal1 and up) added to the cell's top level. Existing geometry is avoided; when a connection is blocked by wires routed earlier, those nets are ripped up and rerouted. Returns routed and failed nets and the flylines left. Place the devices first with generate_layout. {NOT_SAVED}"
            ),
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "check_against_source",
            "Layout XL 'Check Against Source': compare a schematic-driven layout with its schematic. Lists schematic MOS devices not placed (missing), named layout devices no longer in the schematic (extra), placed devices whose W / L / m / model differ (parameters; matched units compared by total width), layout conductors joining several schematic nets (short) and schematic nets split over several conductors (open = unrouted). Boxes in µm.",
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "update_from_source",
            &format!(
                "Layout XL 'Update Components and Nets': place schematic devices missing from the layout, give changed devices the schematic's parameters in place (new PCell master, same placement; compact afterwards if they grew), and with remove_extra delete devices no longer in the schematic. Shorts and opens are reported, not fixed: call route. {NOT_SAVED}"
            ),
            json!({"cell": cell(), "remove_extra": {"type": "boolean", "description": "Delete layout devices that are not in the schematic (default false)."}}),
            &["cell"],
        ),
        tool(
            "abut",
            &format!(
                "Layout XL 'Abut': slide the named MOS PCell instances together, left to right, so each one's outer source / drain column sits on its neighbour's (shared diffusion, active areas aligned). With a schematic the shared columns must be on the same net; a device is flipped left-right when that makes them match, else the call fails without changing anything. All devices must have the same type and finger width and not be arrays or turned 90°. {NOT_SAVED}"
            ),
            json!({"cell": cell(), "instances": {"type": "array", "items": {"type": "string"}, "minItems": 2, "description": "Instance names (e.g. [\"M1\", \"M2\"])."}}),
            &["cell", "instances"],
        ),
        tool(
            "run_drc",
            "Run design rule checks on a cell layout (flattened hierarchy) with the technology rule deck. Violations are grouped by rule with up to max_markers marker boxes (µm) each.",
            json!({"cell": cell(), "max_markers": {"type": "integer", "minimum": 0, "description": "Markers listed per rule (default 20)."}}),
            &["cell"],
        ),
        tool(
            "fix_drc",
            &format!(
                "Repair DRC violations automatically: for each violation of the technology deck, candidate edits of the cell's top-level geometry (widen, grow, extend, move apart, shrink, fill notch, add enclosure) are each checked with DRC, and the best one that adds no new violation is applied; repeats until nothing more can be fixed. Geometry inside placed cells is not changed. Returns the fixes applied and the violations left. {NOT_SAVED}"
            ),
            json!({"cell": cell(), "max_fixes": {"type": "integer", "minimum": 1, "description": "Most fixes to apply (default 200)."}}),
            &["cell"],
        ),
        tool(
            "compact",
            &format!(
                "Compact a cell's layout: slide its top-level items (or only the given ones) toward each direction in turn until the design rules stop them. Geometry that touches on a layer moves together, fixed items stay, symmetry and alignment constraints keep holding, and a move that would add DRC violations is undone. Returns the moves and the extent before and after (µm). {NOT_SAVED}"
            ),
            json!({
                "cell": cell(),
                "directions": {"type": "array", "items": {"type": "string", "enum": ["left", "right", "down", "up"]}, "description": "Directions in order (default [\"left\", \"down\"])."},
                "infer_constraints": {"type": "boolean", "description": "First add symmetry constraints for the schematic's matched devices (default true)."}
            }),
            &["cell"],
        ),
        tool(
            "optimize",
            &format!(
                "Size device parameters to meet specs: a deterministic pattern search over the variables, each candidate simulated with the cell's simulation setup (and, with post_layout, placed, routed and extracted first so the specs hold after layout). Specs are waveform-calculator expressions on an analysis dataset with min / max limits (e.g. analysis \"ac\", expr \"bandwidth(v(out))\", min \"9k\"). Given specs / vars replace the cell's saved ones. Returns the best point, its spec values and whether every spec passes; apply writes the values into the schematic. {NOT_SAVED}"
            ),
            json!({
                "cell": cell(),
                "vars": {"type": "array", "items": {"type": "object", "properties": {
                    "target": {"type": "string", "description": "INST.param (e.g. M1.w) or a design variable name"},
                    "min": {"type": "string"}, "max": {"type": "string"},
                    "log": {"type": "boolean"}, "step": {"type": "string", "description": "Grid, e.g. 10n (optional)"}
                }, "required": ["target", "min", "max"]}},
                "specs": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "analysis": {"type": "string", "enum": ["op", "dc", "ac", "tran", "noise"]},
                    "expr": {"type": "string"}, "min": {"type": "string"}, "max": {"type": "string"}
                }, "required": ["name", "analysis", "expr"]}},
                "max_evals": {"type": "integer", "minimum": 4, "description": "Simulation budget (default 60)."},
                "post_layout": {"type": "boolean", "description": "Evaluate every candidate after layout and extraction."},
                "goal": {"type": "string", "enum": ["margin", "min_area"], "description": "Once all specs pass: widest margin (default) or least gate area."},
                "apply": {"type": "boolean", "description": "Write the best values into the schematic (default false)."}
            }),
            &["cell"],
        ),
        tool(
            "verify",
            "Run the project's verification plan (ADE Assembler style): every test (a cell whose sweep setup has specs; without a plan every such cell) over its corners, temperatures, swept values and Monte Carlo runs. Per spec: limits, worst value and the point it occurs at, margin to the nearest limit (negative: outside), passing points and status (pass, FAIL, error, info). Compared with the latest saved run of the project (results/verify), listing changed specs and regressions; with save: true the run is added to that history.",
            json!({
                "tests": {"type": "array", "items": {"type": "string"}, "description": "Run only these tests (names or cells)."},
                "save": {"type": "boolean", "description": "Save the run in the project's history (default false)."}
            }),
            &[],
        ),
        tool(
            "predict_yield",
            &format!(
                "Monte Carlo yield prediction: simulate the cell's sweep setup (corners, temperatures, Monte Carlo runs with random parameter variation), fit a response-surface model of every spec over the random inputs and sample it 10^5 times per condition. Returns per condition the simulated yield with its 95% confidence interval, the predicted yield, and per spec mean, sigma, Cpk, the model's leave-one-out Q2 (trust it above about 0.9) and each varied parameter's share of the spec's variance. Given specs / vars / runs / temps replace the cell's saved ones. {NOT_SAVED}"
            ),
            json!({
                "cell": cell(),
                "vars": {"type": "array", "items": {"type": "object", "properties": {
                    "target": {"type": "string", "description": "INST.param with * / ? wildcards (M*.w, R1.r) or a design variable"},
                    "sigma": {"type": "string", "description": "5% of the nominal or an absolute value"},
                    "dist": {"type": "string", "enum": ["gauss", "uniform"]},
                    "mismatch": {"type": "boolean", "description": "Draw every matching instance independently (default false: one shared draw)"},
                    "nominal": {"type": "string", "description": "Nominal value of a design variable"}
                }, "required": ["target", "sigma"]}},
                "specs": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "analysis": {"type": "string", "enum": ["op", "dc", "ac", "tran", "noise"]},
                    "expr": {"type": "string"}, "min": {"type": "string"}, "max": {"type": "string"}
                }, "required": ["name", "analysis", "expr"]}},
                "runs": {"type": "integer", "minimum": 4, "description": "Monte Carlo runs per condition."},
                "temps": {"type": "string", "description": "Temperatures, e.g. \"-40 27 125\"."},
                "process": {"type": "boolean", "description": "Also draw the technology's measured process variation (extract_corners with apply): correlated model parameters such as nmos.vto, one draw per run."},
                "samples": {"type": "integer", "minimum": 1000, "description": "Model samples per condition (default 100000)."}
            }),
            &["cell"],
        ),
        tool(
            "correlate_silicon",
            &format!(
                "Silicon-to-simulation correlation: compare measured curves (CSV with a header row) with the cell's simulation and optionally fit parameters to them by Levenberg-Marquardt. Each trace maps a measured column to a waveform-calculator expression on the analysis dataset (e.g. \"-i(vd)\" on \"dc\") at its bias (\"VG.dc=0.9\"); the x axis is the analysis sweep. Parameters are model card parameters (\"model:nmos.vto\"), instance parameters (\"M1.w\") or design variables, each with min / max bounds. Returns the RMS relative error per trace before and after the fit, the fitted values and the fitted model cards. Given fields replace the cell's saved setup (sim.silicon). {NOT_SAVED}"
            ),
            json!({
                "cell": cell(),
                "data": {"type": "string", "description": "CSV file, relative to the project directory or absolute."},
                "csv": {"type": "string", "description": "The CSV text itself, instead of a file."},
                "x": {"type": "string", "description": "Column of the x values (default: the first)."},
                "analysis": {"type": "string", "enum": ["dc", "ac", "tran"]},
                "traces": {"type": "array", "items": {"type": "object", "properties": {
                    "column": {"type": "string"}, "expr": {"type": "string"},
                    "bias": {"type": "string", "description": "INST.param=value or design_var=value, space separated"},
                    "log": {"type": "boolean", "description": "Compare on a log scale"}
                }, "required": ["column", "expr"]}},
                "params": {"type": "array", "items": {"type": "object", "properties": {
                    "target": {"type": "string", "description": "model:NAME.PARAM, INST.param or a design variable"},
                    "min": {"type": "string"}, "max": {"type": "string"}, "log": {"type": "boolean"}
                }, "required": ["target", "min", "max"]}},
                "fit": {"type": "boolean", "description": "Fit the parameters (default false: compare only)."},
                "iterations": {"type": "integer", "minimum": 1, "description": "Most fit iterations (default 40)."}
            }),
            &["cell"],
        ),
        tool(
            "process_cross_section",
            "Process emulation: the cross-section of a cell's layout along a cut line, built by running a process flow (deposit, etch, implant, anneal, oxidize, planarize, strip on a 2-D grid; masks are layout layers or derived-layer expressions; implants with species, dose and energy add Gaussian dopant profiles, anneal and oxidize diffuse them, oxidize grows Deal-Grove oxide). Without a flow, the generic 180 nm flow (STI, wells and drive-in, gate oxidation, spacers, S/D implants and RTA, contacts, three metals). Returns the materials and, at points along the cut, the material stack (bottom up, µm), the doped layers top down (n/p, top and bottom z, peak net doping cm^-3, sheet resistance ohm/sq) and the junction depths; svg_path writes the picture.",
            json!({
                "cell": cell(),
                "flow": {"type": "string", "description": "Process flow TOML text (see process/generic180.toml)."},
                "flow_path": {"type": "string", "description": "Process flow file instead of the text."},
                "cut": {"type": "array", "items": {"type": "number"}, "description": "[x0, y0, x1, y1] in µm (default: horizontal across the largest diffusion area)."},
                "steps": {"type": "integer", "minimum": 0, "description": "Stop after this many steps."},
                "at": {"type": "array", "items": {"type": "number"}, "description": "Positions along the cut (µm) for the stacks (default: 5 points)."},
                "svg_path": {"type": "string", "description": "Write the cross-section as SVG to this file."}
            }),
            &["cell"],
        ),
        tool(
            "process_data",
            "Process data: WAT / PCM results (CSV text or file with lot, wafer, site and one column per parameter) against the process specification (TOML; default the generic 180 nm WAT spec). Returns per parameter mean, sigma, Cp, Cpk, out-of-spec sites, the control chart of wafer means (centre, limits from a baseline) with Western Electric rule alarms, the z-score of each model corner and whether the corners cover the measured ±3σ, and the strongly correlated parameter pairs.",
            json!({
                "csv": {"type": "string", "description": "The CSV text."},
                "path": {"type": "string", "description": "A CSV file instead of the text."},
                "spec": {"type": "string", "description": "Specification TOML text (see process/generic180_wat.toml)."},
                "spec_path": {"type": "string", "description": "A specification file instead of the text."}
            }),
            &[],
        ),
        tool(
            "extract_mismatch",
            &format!(
                "Mismatch from silicon: fit Pelgrom's law (sigma^2 = A^2 / (W L) + floor^2) to matched-pair data per device. CSV columns: device (nmos / pmos or a model name; names starting with p are p-channel), w and l (um), dvt (V, threshold difference of the pair), dbeta (relative current factor difference, optional). Without data, the synthetic generic 180 nm pairs. Returns per device Avt (V*um) and Abeta (1*um) with standard errors, the size-independent floor, and the measured against fitted sigma of every size. Avt is the pair coefficient; Monte Carlo draws each device with Avt / sqrt(2 W L). apply stores the coefficients in the open project's technology so Monte Carlo draws them in place of its generic Avt / Abeta. {NOT_SAVED}"
            ),
            json!({
                "csv": {"type": "string", "description": "Matched-pair CSV text."},
                "path": {"type": "string", "description": "A CSV file instead of the text."},
                "apply": {"type": "boolean", "description": "Store the coefficients in the open project's technology (default false)."}
            }),
            &[],
        ),
        tool(
            "extract_corners",
            &format!(
                "Corner models from silicon: fit model card parameters (the spec's [extraction] knobs) so that simulated test structures (each parameter's measure: netlist, analysis, expression) reproduce the WAT data's means (tt) and its corners (ff, ss, fs, sf: primary parameters at mean ± z sigma, the other measured parameters following their correlation with them). Uses the open project's technology models as the start (the generic 180 nm models without a project). Returns every corner's knob values, the simulated against target parameters, the global process variation of the knobs (sigma and correlation, by backward propagation of the measured covariance through the test-structure sensitivities) and the corner library (.lib sections of .model cards, plus an mc section driven by standard normal parameters ics_z1... that a Monte Carlo loop sets, e.g. with ngspice alterparam); lib_path writes it. apply stores the variation in the open project's technology so Monte Carlo draws it. {NOT_SAVED}"
            ),
            json!({
                "csv": {"type": "string", "description": "WAT CSV text."},
                "path": {"type": "string", "description": "A WAT CSV file instead of the text."},
                "spec": {"type": "string", "description": "Specification TOML text with measures and [extraction] (see process/generic180_wat.toml)."},
                "spec_path": {"type": "string", "description": "A specification file instead of the text."},
                "lib_path": {"type": "string", "description": "Write the corner library to this file."},
                "apply": {"type": "boolean", "description": "Store the process variation in the open project's technology (default false)."}
            }),
            &[],
        ),
        tool(
            "place_like",
            &format!(
                "Layout by example: place a cell's devices like a finished reference cell with the same circuit topology. Devices and nets are matched by topology (names and sizes may differ); every MOS device goes where its counterpart is in the reference, sized from this cell's schematic, with the reference's wells, implants and taps. The cell's layout is replaced. Optionally route afterwards. {NOT_SAVED}"
            ),
            json!({"cell": cell(), "reference": {"type": "string", "description": "Reference cell name or lib/cell"}, "route": {"type": "boolean", "description": "Route the flylines afterwards (default true)."}}),
            &["cell", "reference"],
        ),
        tool(
            "diff_layout",
            "Compare a cell's layout with the version committed at a git revision of the project (default HEAD) or saved on disk: the flattened geometry XOR-ed per layer, as added / removed pieces with their boxes (µm), and placements added, removed or moved.",
            json!({"cell": cell(), "rev": {"type": "string", "description": "Git revision (default HEAD); \"saved\" compares with the project files on disk."}}),
            &["cell"],
        ),
        tool(
            "extract",
            "Extract connectivity and devices from a cell layout: nets (name, bbox µm), MOS devices (W/L µm, terminal nets), PDK resistors / capacitors (value in Ω / F, terminal nets) and the extracted circuit as SPICE.",
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "run_lvs",
            "Layout versus schematic for a cell that has both views: matched device and net pairs plus differences (missing/extra devices, W/L parameter mismatches, net errors with layout markers in µm). Voltage/current sources are ignored.",
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "run_signoff",
            "Run an external sign-off DRC or LVS runset of the technology (KLayout .drc/.lvs deck, Calibre SVRF rule file or netgen) on a cell. Without 'runset', lists the runsets. DRC violations are grouped by rule with marker boxes (µm); LVS returns matched (true / false / null when unreadable) and the tool's messages. Also returns the work directory with the inputs and reports.",
            json!({
                "cell": cell(),
                "runset": {"type": "string", "description": "Runset name, or a unique part of it."},
                "max_markers": {"type": "integer", "minimum": 0, "description": "Markers listed per rule (default 20)."},
            }),
            &["cell"],
        ),
        tool(
            "post_layout_parasitics",
            "Layout parasitics mapped to schematic nets and devices via LVS: capacitor lines (wire capacitance to ground, coupling between nets) and MOS junction parameters (ad/as/pd/ps) that simulate adds with parasitics=true, a summary, and the largest net and coupling capacitances.",
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "emir",
            "IR drop and electromigration of one net (e.g. a supply) of a cell layout. The net's wires on layers with a sheet resistance become a resistor mesh (each via cut a resistor), pins labelled with the net name are the pads, and every MOS with its source or drain on the net draws its current from the nearest wire. Currents come from 'currents' (layout device name as extract reports it -> amps), else from an operating-point simulation of the schematic (built-in engine) matched via LVS. Returns the worst IR drop and where (µm), the loads, the highest current densities and the EM violations against the technology limits (caps[].em in A per µm of width, vias[].em in A per cut).",
            json!({
                "cell": cell(),
                "net": {"type": "string", "description": "Net name, e.g. vdd or gnd."},
                "currents": {"type": "object", "additionalProperties": {"type": "number"}, "description": "Amps per layout device name (M1, M2, ...)."},
                "max_items": {"type": "integer", "minimum": 0, "description": "Branches and violations listed (default 20)."},
            }),
            &["cell", "net"],
        ),
        tool(
            "opc",
            &format!(
                "Optical proximity correction of one layer of a cell layout (hierarchy flattened, Manhattan shapes). The litho model is a Gaussian blur of the mask followed by a resist threshold; every edge is cut into fragments that are moved along their normal, over several iterations, against the edge placement error (EPE: printed minus drawn edge position, positive = outward) at their middle. Defaults come from the layer's min width and spacing. Returns the EPE statistics without and with correction, the mask area and the worst fragments. The drawn layer is never changed, so the correction always starts from the drawn shapes. With apply=true the mask is written to the layer <layer>_opc (added to the technology on first use, same GDS layer, next free datatype), replacing the cell's previous mask there. {NOT_SAVED}"
            ),
            json!({
                "cell": cell(),
                "layer": {"type": "string", "description": "Layer to correct, e.g. poly."},
                "apply": {"type": "boolean", "description": "Write the mask to <layer>_opc in the cell (default false: report only)."},
                "sigma": {"type": "number", "description": "Blur sigma in µm (default 0.45 x min width)."},
                "threshold": {"type": "number", "description": "Resist threshold, 0..1 of the open-frame intensity (default 0.5)."},
                "fragment": {"type": "number", "description": "Longest fragment in µm (default 2 x min width)."},
                "max_out": {"type": "number", "description": "Furthest outward move in µm (default min spacing / 3)."},
                "max_in": {"type": "number", "description": "Furthest inward move in µm (default min width / 4)."},
                "iterations": {"type": "integer", "minimum": 1, "description": "Correction iterations (default 12)."},
                "gain": {"type": "number", "description": "Fraction of the EPE corrected in the first step (default 0.6)."},
                "max_items": {"type": "integer", "minimum": 0, "description": "Worst fragments listed (default 20)."},
            }),
            &["cell", "layer"],
        ),
        tool(
            "export_gds",
            "Write a cell layout and every cell it places to a GDSII file, or to OASIS when the path ends in .oas / .oasis.",
            json!({"cell": cell(), "path": {"type": "string", "description": "Output .gds or .oas file path."}}),
            &["cell", "path"],
        ),
        tool(
            "import_gds",
            &format!(
                "Import a GDSII or OASIS file (detected from its contents) as a new library (cells mapped to technology layers by GDS layer/datatype; unknown layers are skipped). {NOT_SAVED}"
            ),
            json!({
                "path": {"type": "string", "description": "GDSII or OASIS file to read."},
                "lib": {"type": "string", "description": "Library name (defaults to the file name; made unique)."},
                "layermap": {"type": "string", "description": "Cadence Virtuoso layer map file (`RX drawing 2 0` per line) applied first: its rows join the technology's stream map and layers it names that the technology lacks are added."},
            }),
            &["path"],
        ),
        tool(
            "export_lef",
            "Write the LEF abstract of a cell with a technology header (units, routing and cut layers, default vias): MACRO with CLASS, ORIGIN, SIZE (boundary layer, else the cell extent), a PIN per labelled net (DIRECTION from the devices it drives or is driven by, USE POWER / GROUND for supply names, PORT shapes on the routing layers) and OBS for the other metal.",
            json!({
                "cell": cell(),
                "path": {"type": "string", "description": "Output .lef file path."},
                "obs": {"type": "string", "enum": ["detailed", "block", "none"], "description": "Obstructions: merged shapes (default), per-layer blocks around the pins, or none."},
                "class": {"type": "string", "description": "Macro CLASS (default BLOCK)."},
            }),
            &["cell", "path"],
        ),
        tool(
            "import_lef",
            &format!(
                "Import the macros of a LEF file as layout cells of a new library: the SIZE box on the boundary layer, pin ports with a label of the pin name, and obstructions (layers matched by name; M1 / met1 / metal1 are the same). {NOT_SAVED}"
            ),
            json!({
                "path": {"type": "string", "description": "LEF file to read."},
                "lib": {"type": "string", "description": "Library name (defaults to the file name; made unique)."},
            }),
            &["path"],
        ),
        tool(
            "export_def",
            "Write a top cell as DEF: DIEAREA from its box, COMPONENTS from its instances (array elements expanded), PINS from its top-level labels and NETS from the extracted connectivity between the placed cells' pins. Optionally also writes the LEF abstracts of the placed cells.",
            json!({
                "cell": cell(),
                "path": {"type": "string", "description": "Output .def file path."},
                "lef_path": {"type": "string", "description": "Also write the LEF of the placed cells to this file."},
            }),
            &["cell", "path"],
        ),
        tool(
            "import_def",
            &format!(
                "Import a DEF file as a new top cell placing the project cells named by its components' macros (import their LEF first), with its pins as labelled shapes and its die area on the boundary layer. {NOT_SAVED}"
            ),
            json!({
                "path": {"type": "string", "description": "DEF file to read."},
                "lib": {"type": "string", "description": "Library of the new cell, searched first for macros (default work)."},
            }),
            &["path"],
        ),
        tool(
            "run_skill",
            &format!(
                "Run Cadence SKILL (.il) code on the project, e.g. dbOpenCellViewByType / dbCreateRect / dbCreateInst (coordinates in µm). Give 'code' or 'file'. Returns what the script printed, the value of its last form and the cells whose layout it created or changed (lib/cell). On an error the edits made before it are kept. Sandboxed: 'file', load(), outfile() and infile() only reach files under the project directory (relative paths are taken from there; an unsaved project has none), getShellEnvVar() is unavailable, and a run stops after 120 s. {NOT_SAVED}"
            ),
            json!({
                "code": {"type": "string", "description": "SKILL source to run."},
                "file": {"type": "string", "description": "SKILL file (.il) to run instead of 'code'; base directory of relative load() paths."},
                "cell": {"type": "string", "description": "Cell returned by geGetEditCellView() (cell or lib/cell)."},
            }),
            &[],
        ),
        tool(
            "gui_status",
            "Whether the IC Studio desktop app is connected (live mode: every tool runs on the project open in its window and the user sees edits, DRC/LVS markers and waveforms there immediately) and which project is open.",
            json!({}),
            &[],
        ),
    ]
}

impl State {
    /// State around a project (scripting: the Python API).
    pub fn with_project(project: Project) -> Self {
        Self { project: Some(project), ..Default::default() }
    }

    /// The open project.
    pub fn current(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    /// The open project, to change it directly (marks it as changed).
    pub fn current_mut(&mut self) -> Option<&mut Project> {
        self.dirty = true;
        self.project.as_mut()
    }

    /// State around the project open in the desktop app (live mode).
    pub fn in_gui(project: Option<Project>, dirty: bool) -> Self {
        Self { project, dirty, in_gui: true, verified: None }
    }

    /// The reports behind the last run_drc / extract / run_lvs call (live mode only).
    pub fn take_verified(&mut self) -> Option<Verified> {
        self.verified.take()
    }

    fn keep(&mut self, v: Verified) {
        if self.in_gui {
            self.verified = Some(v);
        }
    }

    /// The project and whether it has unsaved changes.
    pub fn into_parts(self) -> (Option<Project>, bool) {
        (self.project, self.dirty)
    }

    /// Run tool `name`; `None` if there is no such tool.
    pub fn call(&mut self, name: &str, args: &Map<String, Value>) -> Option<Result<Value, String>> {
        let a = &Args(args);
        Some(match name {
            "open_project" => self.open_project(a),
            "new_project" => self.new_project(a),
            "save_project" => self.save_project(),
            "load_pdk" => self.load_pdk(a),
            "project_info" => self.project_info(),
            "get_netlist" => self.get_netlist(a),
            "write_schematic" => self.write_schematic(a),
            "import_cdl" => self.import_cdl(a),
            "export_skill_schematic" => self.export_skill_schematic(a),
            "import_virtuoso" => self.import_virtuoso(a),
            "set_simulation" => self.set_simulation(a),
            "simulate" => self.simulate(a),
            "run_spice" => self.run_spice(a),
            "layout_summary" => self.layout_summary(a),
            "add_rect" => self.add_rect(a),
            "add_path" => self.add_path(a),
            "add_label" => self.add_label(a),
            "place_instance" => self.place_instance(a),
            "place_pcell" => self.place_pcell(a),
            "delete_items" => self.delete_items(a),
            "generate_layout" => self.generate_layout(a),
            "flylines" => self.flylines(a),
            "route" => self.route(a),
            "check_against_source" => self.check_against_source(a),
            "update_from_source" => self.update_from_source(a),
            "abut" => self.abut(a),
            "run_drc" => self.run_drc(a),
            "fix_drc" => self.fix_drc(a),
            "compact" => self.compact(a),
            "diff_layout" => self.diff_layout(a),
            "optimize" => self.optimize(a),
            "place_like" => self.place_like(a),
            "extract_corners" => self.extract_corners(a),
            "extract_mismatch" => self.extract_mismatch(a),
            "process_data" => process_data(a),
            "process_cross_section" => self.process_cross_section(a),
            "correlate_silicon" => self.correlate_silicon(a),
            "predict_yield" => self.predict_yield(a),
            "verify" => self.verify(a),
            "extract" => self.extract(a),
            "run_lvs" => self.run_lvs(a),
            "run_signoff" => self.run_signoff(a),
            "post_layout_parasitics" => self.post_layout_parasitics(a),
            "emir" => self.emir(a),
            "opc" => self.opc(a),
            "export_gds" => self.export_gds(a),
            "import_gds" => self.import_gds(a),
            "export_lef" => self.export_lef(a),
            "import_lef" => self.import_lef(a),
            "export_def" => self.export_def(a),
            "import_def" => self.import_def(a),
            "run_skill" => self.run_skill(a),
            "gui_status" => Ok(self.gui_status()),
            _ => return None,
        })
    }

    fn project(&self) -> Result<&Project, String> {
        self.project.as_ref().ok_or_else(|| "no project is open; call open_project or new_project first".into())
    }

    fn project_mut(&mut self) -> Result<&mut Project, String> {
        self.project.as_mut().ok_or_else(|| "no project is open; call open_project or new_project first".into())
    }

    fn cell_arg(&self, a: &Args) -> Result<(&Project, CellRef), String> {
        let p = self.project()?;
        Ok((p, resolve(p, a.str("cell")?)?))
    }

    /// The cell to edit, created with an empty layout if needed. Returns whether it was created.
    fn target(&mut self, cell: &str) -> Result<(CellRef, bool), String> {
        let (r, created) = self.target_cell(cell)?;
        self.project_mut()?.cell_mut(&r).expect("resolved").layout.get_or_insert_default();
        Ok((r, created))
    }

    /// The cell to edit, created (without views) if needed. Returns whether it was created.
    fn target_cell(&mut self, cell: &str) -> Result<(CellRef, bool), String> {
        let p = self.project_mut()?;
        let (r, created) = match find(p, cell) {
            Some(r) => (r, false),
            None => {
                let (lib, name) = match cell.split_once('/') {
                    Some((l, c)) => (l.to_string(), c),
                    None => {
                        let lib = p
                            .libraries
                            .iter()
                            .map(|l| l.name.clone())
                            .find(|n| n == "work")
                            .or_else(|| p.libraries.iter().map(|l| l.name.clone()).find(|n| n != PCELL_LIB))
                            .unwrap_or_else(|| "work".into());
                        (lib, cell)
                    }
                };
                if p.library_mut(&lib).is_none() {
                    p.add_library(&lib)?;
                }
                (p.add_cell(&lib, Cell::new(name))?, true)
            }
        };
        if r.lib == PCELL_LIB {
            return Err(format!("'{r}' is a generated PCell variant; edit a cell in another library"));
        }
        self.dirty = true;
        Ok((r, created))
    }

    fn layout_mut(&mut self, r: &CellRef) -> &mut Layout {
        self.project.as_mut().and_then(|p| p.cell_mut(r)).and_then(|c| c.layout.as_mut()).expect("target layout")
    }

    fn gui_status(&self) -> Value {
        let project = self.project.as_ref().map(
            |p| json!({"name": p.name, "root": p.root.as_ref().map(|r| r.display().to_string()), "unsaved_changes": self.dirty}),
        );
        let note = if self.in_gui {
            "Live: tools run on the project open in the IC Studio window and their results are shown there."
        } else {
            "Standalone: no IC Studio window is connected (or ICS_MCP_STANDALONE is set); tools run on this server's own project. Start ic-studio to work live."
        };
        json!({"live": self.in_gui, "pid": std::process::id(), "project": project, "note": note})
    }

    fn open_project(&mut self, a: &Args) -> Result<Value, String> {
        let p = Project::open(Path::new(a.str("path")?)).map_err(|e| e.to_string())?;
        let discarded = self.dirty;
        self.project = Some(p);
        self.dirty = false;
        let mut info = self.project_info()?;
        if discarded {
            info["warning"] = json!("unsaved changes to the previously open project were discarded");
        }
        Ok(info)
    }

    fn new_project(&mut self, a: &Args) -> Result<Value, String> {
        let dir = PathBuf::from(a.str("path")?);
        if dir.join(MANIFEST).exists() {
            return Err(format!("{} already holds a project; use open_project", dir.display()));
        }
        let name = dir.file_name().map_or("project".into(), |s| s.to_string_lossy().to_string());
        let mut p = match a.opt_str("template")?.unwrap_or("demo") {
            "empty" => Project::new(&name),
            "demo" => {
                let mut p = ics_core::examples::demo_project();
                p.name = name;
                p
            }
            t => return Err(format!("unknown template '{t}' (use empty or demo)")),
        };
        p.save_as(&dir).map_err(|e| e.to_string())?;
        self.project = Some(p);
        self.dirty = false;
        self.project_info()
    }

    fn save_project(&mut self) -> Result<Value, String> {
        let p = self.project()?;
        p.save().map_err(|e| e.to_string())?;
        let root = p.root.as_ref().map(|r| r.display().to_string());
        self.dirty = false;
        Ok(json!({"saved": root}))
    }

    fn load_pdk(&mut self, a: &Args) -> Result<Value, String> {
        let pdk = ics_core::pdk::load(std::path::Path::new(a.str("path")?))?;
        let p = self.project_mut()?;
        let warnings: Vec<String> = pdk.warnings.into_iter().chain(p.set_tech(pdk.tech)?).collect();
        self.dirty = true;
        let mut info = self.project_info()?;
        info["warnings"] = json!(warnings);
        Ok(info)
    }

    fn project_info(&self) -> Result<Value, String> {
        let p = self.project()?;
        let t = &p.tech;
        let libraries: Vec<Value> = p
            .libraries
            .iter()
            .map(|l| {
                let cells: Vec<Value> = l
                    .cells
                    .iter()
                    .map(|c| {
                        let mut views = vec![];
                        if c.schematic.is_some() {
                            views.push("schematic");
                        }
                        if c.layout.is_some() {
                            views.push("layout");
                        }
                        json!({"name": c.name, "views": views, "description": c.description})
                    })
                    .collect();
                json!({"name": l.name, "cells": cells})
            })
            .collect();
        Ok(json!({
            "name": p.name,
            "description": p.description,
            "root": p.root.as_ref().map(|r| r.display().to_string()),
            "unsaved_changes": self.dirty,
            "tech": {
                "name": t.name,
                "dbu_per_um": t.dbu_per_um,
                "grid_um": um(t, t.grid),
                "layers": t.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
                "routing_layers": t.layers.iter().filter(|l| l.routing).map(|l| l.name.as_str()).collect::<Vec<_>>(),
                "vias": t.vias.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
                "devices": t.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
                "model_corners": t.models.as_ref().map(|m| m.corners.clone()),
            },
            "libraries": libraries,
        }))
    }

    fn get_netlist(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let c = p.cell(&r).expect("resolved");
        let sch = c.schematic.as_ref().ok_or("cell has no schematic view")?;
        let format: ics_core::netlist::Format = a.opt_str("format")?.unwrap_or("spice").parse()?;
        let (deck, warnings) = ics_core::netlist::write(sch, &p.tech, &c.sim, &r.to_string(), format);
        Ok(json!({"cell": r.to_string(), "deck": deck, "warnings": warnings}))
    }

    fn import_cdl(&mut self, a: &Args) -> Result<Value, String> {
        let text = match (a.opt_str("cdl")?, a.opt_str("path")?) {
            (Some(t), _) => t.to_string(),
            (None, Some(p)) => std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?,
            _ => return Err("give the CDL as cdl or path".into()),
        };
        let imp = ics_core::cadence::import_cdl(&text, &self.project()?.tech, a.opt_str("top")?)?;
        let cell = match a.opt_str("cell")? {
            Some(c) => c.to_string(),
            None => format!("work/{}", if imp.flat.top.is_empty() { "cdl" } else { &imp.flat.top }),
        };
        if let Some(r) = find(self.project()?, &cell)
            && self.project()?.cell(&r).is_some_and(|c| c.schematic.is_some())
            && !a.opt_bool("overwrite")?.unwrap_or(false)
        {
            return Err(format!("'{r}' already has a schematic; pass overwrite: true to replace it"));
        }
        let (r, created) = self.target_cell(&cell)?;
        let p = self.project_mut()?;
        p.cell_mut(&r).expect("resolved").schematic = Some(imp.imported.schematic);
        self.dirty = true;
        Ok(json!({
            "cell": r.to_string(), "created": created, "top": imp.flat.top, "devices": imp.flat.devices,
            "pins": imp.flat.pins.iter().map(|(n, d)| json!({"name": n, "direction": d})).collect::<Vec<_>>(),
            "subckts": imp.subckts, "warnings": imp.imported.warnings,
        }))
    }

    fn import_virtuoso(&mut self, a: &Args) -> Result<Value, String> {
        let text = match (a.opt_str("dump")?, a.opt_str("path")?) {
            (Some(t), _) => t.to_string(),
            (None, Some(p)) => ics_core::virtuoso_batch::read_dump(std::path::Path::new(p), a.opt_str("lib")?)?,
            _ => return Err("give the dump as dump or path".into()),
        };
        let map = match a.opt_str("map")? {
            Some(t) => ics_core::cadence::SymbolMap::from_toml(t)?,
            None => ics_core::cadence::SymbolMap::analog_lib(),
        };
        let scale = a.0.get("scale").and_then(Value::as_f64).unwrap_or(0.1875);
        let d = ics_core::virtuoso_dump::Dump::parse(&text)?;
        let imp = ics_core::virtuoso_dump::import_dump(&d, &self.project()?.tech, &map, scale)?;
        let overwrite = a.opt_bool("overwrite")?.unwrap_or(false);
        let mut warnings = imp.warnings.clone();
        let done = ics_core::virtuoso_dump::apply(self.project_mut()?, &imp, overwrite, &mut warnings)?;
        self.dirty = true;
        let cells: Vec<Value> = done
            .iter()
            .map(|d| {
                let c = imp.cells.iter().find(|c| c.cell == d.cell).expect("imported");
                json!({"cell": d.cell.to_string(), "schematic": d.schematic, "layout": d.layout,
                       "devices": c.devices, "virtuoso_placement": c.placed})
            })
            .collect();
        Ok(json!({"cells": cells, "warnings": warnings}))
    }

    fn verify(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::verify;
        let p = self.project()?;
        let mut tests = p.settings.verification.resolve(p);
        if let Some(only) = a.0.get("tests").and_then(Value::as_array) {
            let only: Vec<&str> = only.iter().filter_map(Value::as_str).collect();
            tests.retain(|(n, r)| only.iter().any(|t| *t == n || *t == r.to_string() || *t == r.cell));
        }
        if tests.is_empty() {
            return Err("no tests: give cells specs in their sweep setup (sim.sweep.specs)".into());
        }
        let mut opts = ics_sim::SimOptions::default();
        if let Some(exe) = &p.trusted_ngspice() {
            opts.ngspice_exe = exe.clone();
        }
        let run = ics_sim::verify::run(p, &tests, &opts, &|_, _, _| {});
        let root = p.root.clone();
        let before = root.as_deref().and_then(|r| verify::list_history(r).pop()).and_then(|f| verify::load_run(&f).ok());
        let changes: Vec<Value> = before
            .as_ref()
            .map(|b| run.compare(b))
            .unwrap_or_default()
            .iter()
            .map(|c| {
                json!({"test": c.test, "spec": c.spec, "before": c.before.map(|s| s.label()), "after": c.after.label(),
                       "worst_before": c.worst_before, "worst_after": c.worst_after, "regression": c.is_regression()})
            })
            .collect();
        let saved = match (a.opt_bool("save")?.unwrap_or(false), &root) {
            (true, Some(r)) => Some(verify::save_history(r, &run)?.display().to_string()),
            _ => None,
        };
        let n = run.counts();
        let tests: Vec<Value> = run
            .tests
            .iter()
            .map(|t| {
                let specs: Vec<Value> = t
                    .specs
                    .iter()
                    .map(|s| {
                        json!({"name": s.name, "limits": s.limits(), "status": s.status().label(), "worst": s.worst,
                               "at": s.worst_point.and_then(|k| t.points.get(k)), "margin": s.margin, "range": s.range,
                               "passed": s.passed, "points": s.values.len(), "errors": s.errors,
                               "first_error": (!s.first_error.is_empty()).then_some(&s.first_error)})
                    })
                    .collect();
                json!({"name": t.name, "cell": t.cell, "status": t.status().label(), "points": t.points.len(),
                       "error": t.error, "specs": specs})
            })
            .collect();
        Ok(json!({"status": run.status().label(), "pass": n.pass, "fail": n.fail, "error": n.error, "info": n.info,
                  "tests": tests, "changes": changes, "saved": saved}))
    }

    fn export_skill_schematic(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let sch = p.cell(&r).and_then(|c| c.schematic.as_ref()).ok_or_else(|| format!("{r} has no schematic"))?;
        let map = match a.opt_str("map")? {
            Some(t) => ics_core::cadence::SymbolMap::from_toml(t)?,
            None => ics_core::cadence::SymbolMap::analog_lib(),
        };
        let scale = a.0.get("scale").and_then(Value::as_f64).unwrap_or(0.1875);
        let lib = a.opt_str("lib")?.unwrap_or(&r.lib).to_string();
        let (il, warnings) = ics_core::cadence::skill_schematic(sch, &lib, &r.cell, &map, scale);
        if let Some(path) = a.opt_str("path")? {
            std::fs::write(path, &il).map_err(|e| format!("{path}: {e}"))?;
        }
        Ok(json!({"cell": r.to_string(), "lib": lib, "script": il, "warnings": warnings}))
    }

    fn write_schematic(&mut self, a: &Args) -> Result<Value, String> {
        let cell = a.str("cell")?;
        let mut imp = ics_core::spice_import::import(a.str("netlist")?, &self.project()?.tech)?;
        if let Some(pins) = a.get("pins") {
            let pins = pins.as_object().ok_or("argument 'pins' must be an object of net: direction")?;
            let list: Vec<(String, ics_core::PortDir)> = pins
                .iter()
                .map(|(n, d)| {
                    d.as_str().map(|d| (n.clone(), ics_core::PortDir::parse(d))).ok_or("pin directions must be strings")
                })
                .collect::<Result<_, _>>()?;
            let marked = imp.schematic.mark_ports(&list);
            if marked < list.len() {
                imp.warnings.push(format!("{} pin(s) name no net of the netlist", list.len() - marked));
            }
        }
        if let Some(r) = find(self.project()?, cell)
            && self.project()?.cell(&r).is_some_and(|c| c.schematic.is_some())
            && !a.opt_bool("overwrite")?.unwrap_or(false)
        {
            return Err(format!("'{r}' already has a schematic; pass overwrite: true to replace it"));
        }
        let (r, created) = self.target_cell(cell)?;
        let p = self.project_mut()?;
        let tech = p.tech.clone();
        let c = p.cell_mut(&r).expect("resolved");
        if let Some(d) = a.opt_str("description")? {
            c.description = d.to_string();
        }
        imp.apply_to(&mut c.sim);
        let nl = ics_core::netlist::netlist_for(&imp.schematic, &tech, &c.sim.corner);
        let nets = nl.conn.nets();
        let devices = imp.schematic.instances.iter().filter(|i| i.kind.spice_prefix().is_some()).count();
        let deck = nl.deck(&r.to_string(), &c.sim);
        let warnings: Vec<String> = imp.warnings.iter().chain(&nl.warnings).cloned().collect();
        let analyses: Vec<&str> = c.sim.enabled().map(|a| a.kind_name()).collect();
        let out = json!({
            "cell": r.to_string(), "created": created, "devices": devices, "nets": nets,
            "analyses": analyses, "warnings": warnings, "deck": deck,
        });
        c.schematic = Some(imp.schematic);
        Ok(out)
    }

    fn set_simulation(&mut self, a: &Args) -> Result<Value, String> {
        let r = resolve(self.project()?, a.str("cell")?)?;
        let analyses = match a.get("analyses") {
            Some(v) => {
                let items = v.as_array().ok_or("argument 'analyses' must be an array")?;
                Some(items.iter().map(parse_analysis).collect::<Result<Vec<_>, _>>()?)
            }
            None => None,
        };
        let backend = backend(a)?;
        let (temp, parasitics) = (a.opt_num("temp")?, a.opt_bool("parasitics")?);
        let (extra, corner) = (a.opt_str("extra")?.map(str::to_string), a.opt_str("corner")?.map(str::to_string));
        let sim = &mut self.project_mut()?.cell_mut(&r).expect("resolved").sim;
        if let Some(list) = &analyses {
            ics_core::spice_import::set_analyses(sim, list);
        }
        if let Some(b) = backend {
            sim.backend = b;
        }
        if let Some(t) = temp {
            sim.temp = t;
        }
        if let Some(on) = parasitics {
            sim.parasitics = on;
        }
        if let Some(e) = extra {
            sim.extra = e;
        }
        if let Some(k) = corner {
            sim.corner = k;
        }
        let setup = serde_json::to_value(&*sim).map_err(|e| e.to_string())?;
        self.dirty = true;
        Ok(json!({"cell": r.to_string(), "setup": setup}))
    }

    fn simulate(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let sim = simulate_cell(p, &r, a.opt_bool("parasitics")?.unwrap_or(false), backend(a)?)?;
        let mut out = results_json(&sim.results);
        out["cell"] = json!(r.to_string());
        out["netlist_warnings"] = json!(sim.warnings);
        if let Some(s) = sim.parasitics {
            out["parasitics"] = json!(s);
        }
        Ok(out)
    }

    fn run_spice(&self, a: &Args) -> Result<Value, String> {
        let opts = sim_options(self.project.as_ref(), backend(a)?.unwrap_or_default());
        let res = ics_sim::simulate(a.str("deck")?, &[], &opts).map_err(|e| e.to_string())?;
        Ok(results_json(&res))
    }

    fn layout_summary(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let lay = layout_of(p, &r)?;
        let idx = Hier::default().get_for(&r, lay, p);
        let mut per_layer: BTreeMap<String, usize> = BTreeMap::new();
        let mut labels = vec![];
        for s in &lay.shapes {
            *per_layer.entry(layer_name(t, s.layer)).or_default() += 1;
            if let Geometry::Label { text, at, .. } = &s.geom
                && labels.len() < MAX_LISTED
            {
                labels.push(json!({"id": s.id, "text": text, "layer": layer_name(t, s.layer), "at": pt_um(t, *at)}));
            }
        }
        let instances: Vec<Value> = lay
            .instances
            .iter()
            .take(MAX_LISTED)
            .map(|i| {
                let mut v = json!({
                    "id": i.id, "name": i.name, "cell": i.cell.to_string(), "at": pt_um(t, (i.trans.x, i.trans.y)),
                    "rot": i.trans.rot as u32 * 90, "mirror": i.trans.mirror,
                });
                if let Some(arr) = i.array {
                    v["array"] =
                        json!({"cols": arr.cols, "rows": arr.rows, "col_step": pt_um(t, arr.col), "row_step": pt_um(t, arr.row)});
                }
                v
            })
            .collect();
        let mut out = json!({
            "cell": r.to_string(),
            "bbox_um": idx.bbox.map(|b| rect_um(t, b)),
            "shape_count": lay.shapes.len(),
            "flat_shape_count": idx.flat_shapes,
            "shapes_per_layer": per_layer,
            "labels": labels,
            "instance_count": lay.instances.len(),
            "instances": instances,
        });
        if a.opt_bool("list_shapes")?.unwrap_or(false) {
            let shapes: Vec<Value> = lay
                .shapes
                .iter()
                .take(MAX_LISTED)
                .map(|s| {
                    let pts = |p: &[(i64, i64)]| p.iter().map(|&q| pt_um(t, q)).collect::<Vec<_>>();
                    let mut v = json!({"id": s.id, "layer": layer_name(t, s.layer)});
                    match &s.geom {
                        Geometry::Rect(b) => v["rect"] = rect_um(t, *b),
                        Geometry::Polygon { points } => v["polygon"] = json!(pts(points)),
                        Geometry::Path { points, width } => {
                            v["path"] = json!(pts(points));
                            v["width"] = json!(um(t, *width));
                        }
                        Geometry::Label { text, at, .. } => {
                            v["label"] = json!(text);
                            v["at"] = pt_um(t, *at);
                        }
                    }
                    v
                })
                .collect();
            out["shapes"] = json!(shapes);
        }
        Ok(out)
    }

    fn add_shape(&mut self, a: &Args, geom: impl FnOnce(&Tech) -> Result<Geometry, String>) -> Result<Value, String> {
        let t = &self.project()?.tech;
        let layer = layer_index(t, a.str("layer")?)?;
        let geom = geom(t)?;
        let bbox = rect_um(t, geom.bbox());
        let (r, created) = self.target(a.str("cell")?)?;
        let id = self.layout_mut(&r).add(layer, geom);
        Ok(json!({"id": id, "cell": r.to_string(), "created_cell": created, "bbox_um": bbox}))
    }

    fn add_rect(&mut self, a: &Args) -> Result<Value, String> {
        let c = [a.num("x0")?, a.num("y0")?, a.num("x1")?, a.num("y1")?];
        self.add_shape(a, |t| {
            let r = IRect::new(dbu(t, c[0]), dbu(t, c[1]), dbu(t, c[2]), dbu(t, c[3]));
            if r.width() == 0 || r.height() == 0 {
                return Err("rectangle has zero width or height after snapping to the grid".into());
            }
            Ok(Geometry::Rect(r))
        })
    }

    fn add_path(&mut self, a: &Args) -> Result<Value, String> {
        let raw = a.get("points").and_then(Value::as_array).ok_or("argument 'points' must be an array of [x, y]")?;
        let pts: Vec<(f64, f64)> = raw
            .iter()
            .map(|p| match p.as_array().map(|v| v.iter().map(Value::as_f64).collect::<Vec<_>>()).as_deref() {
                Some([Some(x), Some(y)]) => Ok((*x, *y)),
                _ => Err(format!("invalid point {p}; expected [x, y]")),
            })
            .collect::<Result<_, _>>()?;
        if pts.len() < 2 {
            return Err("a path needs at least 2 points".into());
        }
        let width = a.num("width")?;
        self.add_shape(a, |t| {
            let w = dbu(t, width);
            if w <= 0 {
                return Err("width must be at least one grid step".into());
            }
            let mut points: Vec<(i64, i64)> = pts.iter().map(|&(x, y)| (dbu(t, x), dbu(t, y))).collect();
            points.dedup();
            if points.len() < 2 {
                return Err("path collapses to a point after snapping to the grid".into());
            }
            Ok(Geometry::Path { points, width: w })
        })
    }

    fn add_label(&mut self, a: &Args) -> Result<Value, String> {
        let text = a.str("text")?.to_string();
        if text.trim().is_empty() {
            return Err("label text is empty".into());
        }
        let (x, y, size) = (a.num("x")?, a.num("y")?, a.opt_num("size")?.unwrap_or(0.2));
        self.add_shape(a, |t| {
            let size = dbu(t, size).max(t.grid.max(1));
            Ok(Geometry::Label { text, at: (dbu(t, x), dbu(t, y)), size })
        })
    }

    fn place(&mut self, a: &Args, child: CellRef) -> Result<Value, String> {
        let p = self.project()?;
        layout_of(p, &child)?;
        let t = &p.tech;
        let (x, y) = (dbu(t, a.num("x")?), dbu(t, a.num("y")?));
        let rot = a.opt_num("rot")?.unwrap_or(0.0);
        if rot.fract() != 0.0 || (rot as i64).rem_euclid(90) != 0 {
            return Err("rot must be 0, 90, 180 or 270".into());
        }
        let trans = Trans::with(x, y, ((rot as i64).rem_euclid(360) / 90) as u8, a.opt_bool("mirror")?.unwrap_or(false));
        let name = a.opt_str("name")?.map(str::to_string);
        let cell = a.str("cell")?;
        if let Some(r) = find(p, cell) {
            if would_cycle(p, &r, &child) {
                return Err(format!("placing '{child}' in '{r}' would create a hierarchy cycle"));
            }
            if let (Some(n), Some(l)) = (&name, p.layout(&r))
                && l.instances.iter().any(|i| i.name.as_ref() == Some(n))
            {
                return Err(format!("an instance named '{n}' already exists in '{r}'"));
            }
        }
        let (r, created) = self.target(cell)?;
        let l = self.layout_mut(&r);
        let id = l.add_instance(child.clone(), trans, None);
        l.instance_mut(id).expect("just added").name = name;
        Ok(json!({"id": id, "cell": r.to_string(), "created_cell": created, "child": child.to_string()}))
    }

    fn place_instance(&mut self, a: &Args) -> Result<Value, String> {
        let child = resolve(self.project()?, a.str("child")?)?;
        self.place(a, child)
    }

    fn place_pcell(&mut self, a: &Args) -> Result<Value, String> {
        let empty = Map::new();
        let params = match a.get("params") {
            None => &empty,
            Some(v) => v.as_object().ok_or("argument 'params' must be an object")?,
        };
        let p = self.project_mut()?;
        let pc = pcell_from(a.str("kind")?, &Args(params), &p.tech)?;
        let summary = pc.summary(p.tech.dbu_per_um);
        let child = p.ensure_pcell(&pc)?;
        self.dirty = true;
        let mut out = self.place(a, child)?;
        out["device"] = json!(summary);
        Ok(out)
    }

    fn delete_items(&mut self, a: &Args) -> Result<Value, String> {
        let ids: Vec<u64> = a
            .get("ids")
            .and_then(Value::as_array)
            .and_then(|v| v.iter().map(Value::as_u64).collect())
            .ok_or("argument 'ids' must be an array of integers")?;
        let (_, r) = self.cell_arg(a)?;
        let l = self.project_mut()?.cell_mut(&r).and_then(|c| c.layout.as_mut()).ok_or("cell has no layout view")?;
        let (found, missing): (Vec<u64>, Vec<u64>) = ids.into_iter().partition(|&id| l.ids().any(|i| i == id));
        for &id in &found {
            l.remove(id);
        }
        if !found.is_empty() {
            self.dirty = true;
        }
        Ok(json!({"cell": r.to_string(), "deleted": found, "not_found": missing}))
    }

    fn generate_layout(&mut self, a: &Args) -> Result<Value, String> {
        let (_, r) = self.cell_arg(a)?;
        if r.lib == PCELL_LIB {
            return Err("PCell variants have no schematic".into());
        }
        let p = self.project_mut()?;
        let sch = p.cell(&r).and_then(|c| c.schematic.clone()).ok_or("cell has no schematic view to generate from")?;
        let plan = sdl::plan(&sch, &p.tech);
        let original = p.cell_mut(&r).expect("resolved").layout.take();
        let mut layout = original.clone().unwrap_or_default();
        let res = sdl::apply(p, &mut layout, &plan);
        let pins = if res.is_ok() { ics_core::eco::place_pins(p, &mut layout, &sch) } else { vec![] };
        p.cell_mut(&r).expect("resolved").layout = if res.is_ok() { Some(layout) } else { original };
        let placed = res?;
        let flylines = sdl::ratsnest(&sch, layout_of(p, &r)?, p, &p.tech).len();
        self.dirty = true;
        Ok(json!({
            "cell": r.to_string(),
            "placed": placed,
            "skipped": plan.skipped,
            "matched": plan.notes,
            "pins": pins,
            "unrouted_flylines": flylines,
        }))
    }

    fn route(&mut self, a: &Args) -> Result<Value, String> {
        let (_, r) = self.cell_arg(a)?;
        let p = self.project_mut()?;
        let sch = p.cell(&r).and_then(|c| c.schematic.clone()).ok_or("cell has no schematic view")?;
        let mut layout = p.cell_mut(&r).expect("resolved").layout.take().ok_or("cell has no layout view")?;
        let res = ics_core::route::route(&sch, &mut layout, p, &p.tech);
        p.cell_mut(&r).expect("resolved").layout = Some(layout);
        let rep = res?;
        let left = sdl::ratsnest(&sch, layout_of(p, &r)?, p, &p.tech).len();
        let um = p.tech.dbu_per_um.max(1) as f64;
        if rep.connections > 0 {
            self.dirty = true;
        }
        Ok(json!({
            "cell": r.to_string(),
            "routed": rep.routed,
            "failed": rep.failed,
            "connections": rep.connections,
            "vias": rep.vias,
            "ripups": rep.ripups,
            "wire_length_um": sig(rep.wire_length as f64 / um),
            "unrouted_flylines": left,
        }))
    }

    fn check_against_source(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let sch = p.cell(&r).and_then(|c| c.schematic.as_ref()).ok_or("cell has no schematic view")?;
        let c = ics_core::eco::check(sch, layout_of(p, &r)?, p);
        let issues: Vec<Value> = c
            .issues
            .iter()
            .map(|i| {
                let mut v = json!({"kind": i.kind(), "message": i.message()});
                if let Some(at) = i.at() {
                    v["at"] = rect_um(t, at);
                }
                v
            })
            .collect();
        Ok(
            json!({"cell": r.to_string(), "clean": c.is_clean(), "summary": c.summary(), "devices": c.devices, "nets": c.nets, "issues": issues}),
        )
    }

    fn update_from_source(&mut self, a: &Args) -> Result<Value, String> {
        let (_, r) = self.cell_arg(a)?;
        let remove_extra = a.opt_bool("remove_extra")?.unwrap_or(false);
        let p = self.project_mut()?;
        let sch = p.cell(&r).and_then(|c| c.schematic.clone()).ok_or("cell has no schematic view")?;
        let mut layout = p.cell_mut(&r).expect("resolved").layout.take().unwrap_or_default();
        let res = ics_core::eco::update(p, &mut layout, &sch, ics_core::eco::UpdateOptions { remove_extra });
        p.cell_mut(&r).expect("resolved").layout = Some(layout);
        let rep = res?;
        if rep.changed() {
            self.dirty = true;
        }
        Ok(json!({
            "cell": r.to_string(),
            "placed": rep.placed,
            "updated": rep.updated,
            "removed": rep.removed,
            "pins": rep.pins,
            "remaining": rep.remaining,
        }))
    }

    fn abut(&mut self, a: &Args) -> Result<Value, String> {
        let (_, r) = self.cell_arg(a)?;
        let names: Vec<String> = a
            .get("instances")
            .and_then(Value::as_array)
            .ok_or("argument 'instances' must be an array of instance names")?
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or("instance names must be strings"))
            .collect::<Result<_, _>>()?;
        let p = self.project_mut()?;
        let lay = p.cell(&r).and_then(|c| c.layout.as_ref()).ok_or("cell has no layout view")?;
        let ids = names
            .iter()
            .map(|n| {
                lay.instances
                    .iter()
                    .find(|i| i.name.as_deref() == Some(n.as_str()))
                    .map(|i| i.id)
                    .ok_or(format!("no instance named '{n}'"))
            })
            .collect::<Result<Vec<u64>, String>>()?;
        let mut next = lay.clone();
        let sch = p.cell(&r).and_then(|c| c.schematic.as_ref());
        let checked = sch.is_some();
        let notes = ics_core::abut::abut_chain(&mut next, &ids, p, sch)?;
        p.cell_mut(&r).expect("resolved").layout = Some(next);
        self.dirty = true;
        Ok(json!({"cell": r.to_string(), "abutted": notes, "nets_checked": checked}))
    }

    fn flylines(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let sch = p.cell(&r).and_then(|c| c.schematic.as_ref()).ok_or("cell has no schematic view")?;
        let fl = sdl::ratsnest(sch, layout_of(p, &r)?, p, t);
        let lines: Vec<Value> = fl.iter().map(|f| json!({"net": f.net, "a": pt_um(t, f.a), "b": pt_um(t, f.b)})).collect();
        Ok(json!({"cell": r.to_string(), "count": fl.len(), "flylines": lines}))
    }

    fn run_drc(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let lay = layout_of(p, &r)?;
        let max = a.opt_num("max_markers")?.map_or(20, |n| n.max(0.0) as usize);
        let parsed = parse_any(&t.deck_text(), t).map_err(|errs| {
            let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
            format!("technology rule deck has errors: {}", msgs.join("; "))
        })?;
        let (deck, deck_notes) = (parsed.deck, parsed.notes);
        let rep = run_drc_deck(&deck, lay, p, t);
        let mut total = 0;
        let mut rules = vec![];
        for rule in &deck.rules {
            let vs: Vec<_> = rep.violations.iter().filter(|v| v.rule == rule.name).collect();
            let Some(first) = vs.first() else { continue };
            let count = rep.capped.iter().find(|(n, _)| *n == rule.name).map_or(vs.len(), |c| c.1);
            total += count;
            let markers: Vec<Value> = vs.iter().take(max).map(|v| rect_um(t, v.marker)).collect();
            rules.push(json!({"rule": rule.name, "description": first.description, "count": count, "markers_um": markers}));
        }
        let mut out = json!({
            "cell": r.to_string(),
            "clean": total == 0,
            "violations": total,
            "by_rule": rules,
            "rules_run": rep.rules_run,
            "shapes_checked": rep.shapes,
            "any_angle_shapes": rep.any_angle,
            "millis": sig(rep.millis),
        });
        if !deck_notes.is_empty() {
            out["deck_notes"] = json!(deck_notes.iter().take(MAX_LISTED).collect::<Vec<_>>());
        }
        self.keep(Verified::Drc(rep));
        Ok(out)
    }

    fn fix_drc(&mut self, a: &Args) -> Result<Value, String> {
        let (_, r) = self.cell_arg(a)?;
        let max = a.opt_num("max_fixes")?.map_or(200, |n| n.max(1.0) as usize);
        let p = self.project_mut()?;
        let deck = parse_any(&p.tech.deck_text(), &p.tech).map_err(|errs| {
            let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
            format!("technology rule deck has errors: {}", msgs.join("; "))
        })?;
        let mut layout = p.cell_mut(&r).expect("resolved").layout.take().ok_or("cell has no layout view")?;
        let done = ics_core::drc_fix::fix_all(&deck.deck, &mut layout, p, &p.tech, max);
        p.cell_mut(&r).expect("resolved").layout = Some(layout);
        let left = run_drc_deck(&deck.deck, layout_of(p, &r)?, p, &p.tech).violations.len();
        if !done.is_empty() {
            self.dirty = true;
        }
        Ok(json!({"cell": r.to_string(), "fixed": done.len(), "fixes": done, "violations_left": left}))
    }

    fn compact(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::compact::Dir;
        let (_, r) = self.cell_arg(a)?;
        let dirs: Vec<Dir> = match a.0.get("directions").and_then(Value::as_array) {
            Some(v) => v
                .iter()
                .map(|d| match d.as_str() {
                    Some("left") => Ok(Dir::Left),
                    Some("right") => Ok(Dir::Right),
                    Some("down") => Ok(Dir::Down),
                    Some("up") => Ok(Dir::Up),
                    _ => Err(format!("unknown direction {d}")),
                })
                .collect::<Result<_, _>>()?,
            None => vec![Dir::Left, Dir::Down],
        };
        let infer = a.0.get("infer_constraints").and_then(Value::as_bool).unwrap_or(true);
        let p = self.project_mut()?;
        let deck = parse_any(&p.tech.deck_text(), &p.tech).ok().map(|d| d.deck);
        let mut layout = p.cell_mut(&r).expect("resolved").layout.take().ok_or("cell has no layout view")?;
        let inferred = match p.cell(&r).and_then(|c| c.schematic.as_ref()) {
            Some(sch) if infer => ics_core::constraints::infer(&mut layout, p, sch),
            _ => 0,
        };
        let before_layout = layout.clone();
        let (mut moved, mut first, mut last) = (0, None, None);
        for d in dirs {
            let rep = ics_core::compact::compact(&mut layout, p, &p.tech, deck.as_ref(), d, &Default::default());
            moved += rep.moved;
            first = first.or(rep.before);
            last = rep.after.or(last);
        }
        let changed = layout != before_layout || inferred > 0;
        p.cell_mut(&r).expect("resolved").layout = Some(layout);
        if changed {
            self.dirty = true;
        }
        let t = &self.project()?.tech;
        Ok(json!({
            "cell": r.to_string(),
            "constraints_inferred": inferred,
            "moves": moved,
            "extent_before_um": first.map(|b| rect_um(t, b)),
            "extent_after_um": last.map(|b| rect_um(t, b)),
        }))
    }

    fn optimize(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::optimize::{Goal, OptVar};
        use ics_core::sweep::Spec;
        let (_, r) = self.cell_arg(a)?;
        let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let p = self.project_mut()?;
        let c = p.cell_mut(&r).expect("resolved");
        if let Some(vs) = a.0.get("vars").and_then(Value::as_array) {
            c.sim.optimize.vars = vs
                .iter()
                .map(|v| OptVar {
                    target: get(v, "target"),
                    min: get(v, "min"),
                    max: get(v, "max"),
                    log: v.get("log").and_then(Value::as_bool).unwrap_or(false),
                    step: get(v, "step"),
                })
                .collect();
        }
        if let Some(ss) = a.0.get("specs").and_then(Value::as_array) {
            c.sim.sweep.specs = ss
                .iter()
                .map(|s| Spec {
                    name: get(s, "name"),
                    analysis: get(s, "analysis"),
                    expr: get(s, "expr"),
                    min: get(s, "min"),
                    max: get(s, "max"),
                })
                .collect();
        }
        if let Some(n) = a.0.get("max_evals").and_then(Value::as_u64) {
            c.sim.optimize.max_evals = n as usize;
        }
        if let Some(b) = a.0.get("post_layout").and_then(Value::as_bool) {
            c.sim.optimize.post_layout = b;
        }
        match a.0.get("goal").and_then(Value::as_str) {
            Some("min_area") => c.sim.optimize.goal = Goal::MinArea,
            Some("margin") => c.sim.optimize.goal = Goal::Margin,
            _ => {}
        }
        self.dirty = true;
        let p = self.project()?;
        let mut opts = ics_sim::SimOptions::default();
        if let Some(exe) = &p.trusted_ngspice() {
            opts.ngspice_exe = exe.clone();
        }
        let rep = ics_sim::optimize::run(p, &r, &opts, &mut |_, _| true)?;
        let best = rep.best().ok_or("nothing evaluated")?.clone();
        let c = p.cell(&r).expect("resolved");
        let setup = c.sim.optimize.clone();
        let specs: Vec<Value> = c
            .sim
            .sweep
            .specs
            .iter()
            .zip(&best.specs)
            .map(|(s, v)| match v {
                Ok(x) => json!({"name": s.name, "value": sig(*x), "pass": s.check(*x)}),
                Err(e) => json!({"name": s.name, "error": e}),
            })
            .collect();
        let vars: Vec<Value> =
            setup.vars.iter().zip(&best.values).map(|(v, x)| json!({"target": v.target, "value": sig(*x)})).collect();
        let apply = a.0.get("apply").and_then(Value::as_bool).unwrap_or(false);
        if apply {
            let sch = c.schematic.clone().ok_or("cell has no schematic")?;
            let new = ics_sim::optimize::apply_values(&sch, &setup, &best.values);
            self.project_mut()?.cell_mut(&r).expect("resolved").schematic = Some(new);
        }
        Ok(json!({
            "cell": r.to_string(),
            "passed": best.passed,
            "evaluations": rep.evals.len(),
            "vars": vars,
            "specs": specs,
            "note": best.note,
            "applied": apply,
        }))
    }

    fn predict_yield(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::sweep::{Dist, McVar, Spec};
        let (_, r) = self.cell_arg(a)?;
        let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let p = self.project_mut()?;
        let c = p.cell_mut(&r).expect("resolved");
        let sw = &mut c.sim.sweep;
        if let Some(vs) = a.0.get("vars").and_then(Value::as_array) {
            sw.monte_carlo.vars = vs
                .iter()
                .map(|v| McVar {
                    target: get(v, "target"),
                    nominal: get(v, "nominal"),
                    sigma: get(v, "sigma"),
                    dist: if get(v, "dist") == "uniform" { Dist::Uniform } else { Dist::Gauss },
                    mismatch: v.get("mismatch").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect();
        }
        if let Some(ss) = a.0.get("specs").and_then(Value::as_array) {
            sw.specs = ss
                .iter()
                .map(|s| Spec {
                    name: get(s, "name"),
                    analysis: get(s, "analysis"),
                    expr: get(s, "expr"),
                    min: get(s, "min"),
                    max: get(s, "max"),
                })
                .collect();
        }
        if let Some(n) = a.0.get("runs").and_then(Value::as_u64) {
            sw.monte_carlo.runs = n as u32;
        }
        if let Some(t) = a.0.get("temps").and_then(Value::as_str) {
            sw.temps = t.to_string();
        }
        if let Some(b) = a.0.get("process").and_then(Value::as_bool) {
            sw.monte_carlo.process = b;
        }
        if sw.monte_carlo.runs == 0 {
            sw.monte_carlo.runs = 100;
        }
        if (sw.monte_carlo.vars.is_empty() && !sw.monte_carlo.process) || sw.specs.is_empty() {
            return Err(
                "give the varied parameters (vars or process) and the specs, or save them in the cell's sweep setup".into()
            );
        }
        let specs = sw.specs.clone();
        self.dirty = true;
        let samples = a.0.get("samples").and_then(Value::as_u64).unwrap_or(100_000) as usize;
        let p = self.project()?;
        let mut opts = ics_sim::SimOptions::default();
        if let Some(exe) = &p.trusted_ngspice() {
            opts.ngspice_exe = exe.clone();
        }
        let run = ics_sim::sweep::run(p, &r, &opts, &|_, _| {})?;
        let plan = run.plan.clone().ok_or("no Monte Carlo runs")?;
        let conds: Vec<Value> = run
            .predict(&specs, samples, 1)
            .iter()
            .map(|c| {
                let (lo, hi) = ics_core::predict::wilson(c.passed, c.runs);
                let sj: Vec<Value> = specs
                    .iter()
                    .zip(&c.prediction.specs)
                    .map(|(s, sp)| match sp {
                        Ok(sp) => {
                            let mut sens: Vec<(usize, f64)> = sp.sensitivity.iter().copied().enumerate().collect();
                            sens.sort_by(|a, b| b.1.total_cmp(&a.1));
                            let sens: Vec<Value> = sens
                                .iter()
                                .take(8)
                                .map(|(i, v)| json!({"input": plan.inputs[*i], "share": sig(*v), "slope_per_sigma": sig(sp.model.slope(*i))}))
                                .collect();
                            json!({"name": s.name, "mean": sig(sp.mean), "sigma": sig(sp.std), "yield": sp.yield_.map(sig),
                                "cpk": sp.cpk.map(sig), "q2": sig(sp.model.q2), "model_error": sig(sp.model.rmse), "sensitivity": sens})
                        }
                        Err(e) => json!({"name": s.name, "error": e}),
                    })
                    .collect();
                json!({"condition": c.condition, "runs": c.runs, "passed": c.passed, "simulated_ci95": [sig(lo), sig(hi)],
                    "predicted_yield": c.prediction.joint.map(sig), "specs": sj})
            })
            .collect();
        Ok(json!({"cell": r.to_string(), "points": run.points.len(), "notes": run.notes, "conditions": conds}))
    }

    fn correlate_silicon(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::silicon::{FitParam, Measured, Trace};
        let (_, r) = self.cell_arg(a)?;
        let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let p = self.project_mut()?;
        let si = &mut p.cell_mut(&r).expect("resolved").sim.silicon;
        if let Some(d) = a.0.get("data").and_then(Value::as_str) {
            si.data = d.to_string();
        }
        if let Some(x) = a.0.get("x").and_then(Value::as_str) {
            si.x = x.to_string();
        }
        if let Some(x) = a.0.get("analysis").and_then(Value::as_str) {
            si.analysis = x.to_string();
        }
        if let Some(ts) = a.0.get("traces").and_then(Value::as_array) {
            si.traces = ts
                .iter()
                .map(|t| Trace { column: get(t, "column"), expr: get(t, "expr"), bias: get(t, "bias"), log: flag(t, "log") })
                .collect();
        }
        if let Some(ps) = a.0.get("params").and_then(Value::as_array) {
            si.params = ps
                .iter()
                .map(|q| FitParam { target: get(q, "target"), min: get(q, "min"), max: get(q, "max"), log: flag(q, "log") })
                .collect();
        }
        let setup = si.clone();
        self.dirty = true;
        let p = self.project()?;
        let mut opts = ics_sim::SimOptions::default();
        if let Some(exe) = &p.trusted_ngspice() {
            opts.ngspice_exe = exe.clone();
        }
        let cor = match a.0.get("csv").and_then(Value::as_str) {
            Some(text) => ics_sim::silicon::Correlation::with_data(p, &r, setup, Measured::parse(text)?, &opts)?,
            None => ics_sim::silicon::Correlation::new(p, &r, &opts)?,
        };
        let start_values = cor.start_values();
        let iterations = a.0.get("iterations").and_then(Value::as_u64).unwrap_or(40) as usize;
        let (start, best, std_errors) = if a.0.get("fit").and_then(Value::as_bool).unwrap_or(false) {
            let rep = cor.fit(iterations, &mut |_, _| true)?;
            (rep.start, Some(rep.best), rep.std_errors)
        } else {
            (cor.evaluate(&start_values), None, vec![])
        };
        let traces = |e: &ics_sim::silicon::Evaluation| -> Vec<Value> {
            e.traces
                .iter()
                .zip(&cor.setup.traces)
                .map(|(t, s)| match t {
                    Ok(t) => json!({"column": t.column, "error": sig(t.error), "points": t.x.len()}),
                    Err(e) => json!({"column": s.column, "failed": e}),
                })
                .collect()
        };
        let params: Vec<Value> = cor
            .params()
            .iter()
            .enumerate()
            .map(|(i, q)| {
                json!({"target": q.target, "start": sig(start_values[i]), "fitted": best.as_ref().map(|b| sig(b.values[i])),
                "std_error": std_errors.get(i).filter(|e| e.is_finite()).map(|e| sig(*e))})
            })
            .collect();
        let cards = match &best {
            Some(b) => cor.fitted_cards(&b.values)?,
            None => String::new(),
        };
        Ok(json!({
            "cell": r.to_string(),
            "error_start": sig(start.error()),
            "traces_start": traces(&start),
            "error_fitted": best.as_ref().map(|b| sig(b.error())),
            "traces_fitted": best.as_ref().map(traces),
            "params": params,
            "model_cards": cards,
        }))
    }

    fn process_cross_section(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::process::{Cut, GENERIC180, ProcessFlow, default_cut, emulate_cell};
        let (p, r) = self.cell_arg(a)?;
        let text = match (a.0.get("flow").and_then(Value::as_str), a.0.get("flow_path").and_then(Value::as_str)) {
            (Some(t), _) => t.to_string(),
            (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
            _ => GENERIC180.to_string(),
        };
        let flow = ProcessFlow::from_toml(&text).map_err(|e| format!("process flow: {e}"))?;
        let cut = match a.0.get("cut").and_then(Value::as_array) {
            Some(v) => {
                let n: Vec<f64> = v.iter().filter_map(Value::as_f64).collect();
                let [x0, y0, x1, y1] = n[..] else { return Err("cut: give [x0, y0, x1, y1] in µm".into()) };
                Cut { x0, y0, x1, y1 }
            }
            None => default_cut(p, &r)?,
        };
        let steps = a.0.get("steps").and_then(Value::as_u64).map(|n| n as usize);
        let s = emulate_cell(p, &r, &flow, &cut, steps)?;
        let len = cut.length();
        let at: Vec<f64> = match a.0.get("at").and_then(Value::as_array) {
            Some(v) => v.iter().filter_map(Value::as_f64).collect(),
            None => (1..=5).map(|k| len * k as f64 / 6.0).collect(),
        };
        if let Some(path) = a.0.get("svg_path").and_then(Value::as_str) {
            std::fs::write(path, s.svg(&flow, (1200.0 / len).clamp(50.0, 2000.0), &format!("{r} {}", flow.name)))
                .map_err(|e| format!("{path}: {e}"))?;
        }
        let n = steps.unwrap_or(flow.steps.len()).min(flow.steps.len());
        let columns: Vec<Value> = at
            .iter()
            .map(|x| {
                json!({"x": sig(*x), "surface": sig(s.surface_at(*x)),
                    "stack": s.stack_at(*x).iter().map(|(m, t)| json!({"material": m, "thickness": sig(*t)})).collect::<Vec<_>>(),
                    "doping": s.doped_layers(*x).iter().map(|l| json!({"material": l.material, "type": if l.n_type { "n" } else { "p" },
                        "top": sig(l.top), "bottom": sig(l.bottom), "peak": sig(l.peak),
                        "sheet_resistance": l.sheet_resistance.is_finite().then(|| sig(l.sheet_resistance))})).collect::<Vec<_>>(),
                    "junctions": s.junctions(*x).iter().map(|z| sig(*z)).collect::<Vec<_>>()})
            })
            .collect();
        Ok(json!({
            "cell": r.to_string(),
            "flow": flow.name,
            "cut": [cut.x0, cut.y0, cut.x1, cut.y1],
            "length": sig(len),
            "steps_run": n,
            "last_step": if n > 0 { flow.steps[n - 1].label() } else { "substrate".into() },
            "materials": s.materials,
            "columns": columns,
        }))
    }

    fn extract_mismatch(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::mismatch::{GENERIC180_PAIRS, MismatchModel, PairData, Pelgrom, fit};
        let (text, name) = match (a.0.get("csv").and_then(Value::as_str), a.0.get("path").and_then(Value::as_str)) {
            (Some(t), _) => (t.to_string(), "pairs".to_string()),
            (None, Some(p)) => (std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?, p.to_string()),
            _ => (GENERIC180_PAIRS.to_string(), "generic180 pairs (built in)".to_string()),
        };
        let d = PairData::parse(&text)?;
        let fits = fit(&d)?;
        let pel = |p: &Pelgrom| {
            json!({"a": sig(p.a), "a_err": sig(p.a_err), "floor": sig(p.floor),
                "sizes": p.sizes.iter().map(|s| json!({"w": s.w, "l": s.l, "n": s.n, "mean": sig(s.mean), "sigma": sig(s.sigma), "fitted": sig(s.fitted)})).collect::<Vec<_>>()})
        };
        let devices: Vec<Value> = fits
            .iter()
            .map(|f| json!({"device": f.device, "pmos": f.pmos, "vt": pel(&f.vt), "beta": f.beta.as_ref().map(pel)}))
            .collect();
        let apply = a.0.get("apply").and_then(Value::as_bool).unwrap_or(false);
        if apply {
            self.project_mut().map_err(|_| "apply needs an open project")?.tech.mismatch =
                Some(MismatchModel::from_fits(&fits, &name));
            self.dirty = true;
        }
        Ok(json!({"data": name, "pairs": d.rows.len(), "devices": devices, "applied": apply}))
    }

    fn extract_corners(&mut self, a: &Args) -> Result<Value, String> {
        use ics_core::process_data::{GENERIC180_SPEC, ProcessSpec, WatData};
        let read = |text: &str, path: &str| -> Result<Option<String>, String> {
            match (a.0.get(text).and_then(Value::as_str), a.0.get(path).and_then(Value::as_str)) {
                (Some(t), _) => Ok(Some(t.to_string())),
                (None, Some(p)) => std::fs::read_to_string(p).map(Some).map_err(|e| format!("{p}: {e}")),
                _ => Ok(None),
            }
        };
        let d = WatData::parse(&read("csv", "path")?.ok_or("give the data as csv or path")?)?;
        let spec = ProcessSpec::from_toml(&read("spec", "spec_path")?.unwrap_or_else(|| GENERIC180_SPEC.to_string()))?;
        let generic;
        let p = match self.project() {
            Ok(p) => p,
            Err(_) => {
                generic = ics_core::examples::demo_project();
                &generic
            }
        };
        let ex = ics_sim::corners::Extraction::new(p, &spec, &d, &ics_sim::SimOptions::default())?;
        let fits = ex.run(40, &mut |_| {});
        let var = fits.first().map(|tt| ex.variation(tt, &format!("{} WAT", spec.name)));
        let (var, var_error) = match var {
            Some(Ok(v)) => (Some(v), None),
            Some(Err(e)) => (None, Some(e)),
            None => (None, None),
        };
        let library = ex.library(&fits, var.as_ref().map(|v| &v.variation), &format!("{} corners", spec.name))?;
        let variation = var.as_ref().map(|v| {
            let pv = &v.variation;
            json!({
                "params": (0..pv.params.len()).map(|i| json!({"name": pv.params[i], "mean": sig(pv.mean[i]), "sigma": sig(pv.sigma(i))})).collect::<Vec<_>>(),
                "correlation": (0..pv.params.len()).map(|i| (0..pv.params.len()).map(|j| sig(pv.correlation(i, j))).collect::<Vec<_>>()).collect::<Vec<_>>(),
                "reproduced": v.params.iter().map(|(n, d, g)| json!({"name": n, "data_sigma": sig(*d), "sigma": sig(*g)})).collect::<Vec<_>>(),
            })
        });
        let apply = a.0.get("apply").and_then(Value::as_bool).unwrap_or(false);
        if apply {
            let v = var
                .as_ref()
                .ok_or_else(|| format!("no process variation to apply: {}", var_error.clone().unwrap_or_default()))?;
            self.project_mut().map_err(|_| "apply needs an open project")?.tech.variation = Some(v.variation.clone());
            self.dirty = true;
        }
        if let Some(path) = a.0.get("lib_path").and_then(Value::as_str) {
            std::fs::write(path, &library).map_err(|e| format!("{path}: {e}"))?;
        }
        let corners: Vec<Value> = fits
            .iter()
            .map(|f| {
                json!({"corner": f.name, "error_sigma": sig(f.error_sigma),
                    "knobs": ex.knobs.iter().zip(&f.values).map(|(k, v)| json!({"target": k.target, "value": sig(*v)})).collect::<Vec<_>>(),
                    "params": f.params.iter().map(|(n, t, g)| json!({"name": n, "target": sig(*t), "simulated": g.as_ref().ok().map(|v| sig(*v))})).collect::<Vec<_>>()})
            })
            .collect();
        Ok(
            json!({"spec": spec.name, "corners": corners, "variation": variation, "variation_error": var_error, "applied": apply, "library": library}),
        )
    }

    fn place_like(&mut self, a: &Args) -> Result<Value, String> {
        let (p, target) = self.cell_arg(a)?;
        let reference = resolve(p, a.str("reference")?)?;
        let route = a.0.get("route").and_then(Value::as_bool).unwrap_or(true);
        let p = self.project_mut()?;
        let rep = ics_core::by_example::place_like(p, &reference, &target)?;
        let mut out = json!({"cell": target.to_string(), "reference": reference.to_string(), "placed": rep.placed, "taps": rep.taps, "shapes": rep.shapes, "notes": rep.notes});
        if route && let Some(sch) = p.cell(&target).and_then(|c| c.schematic.clone()) {
            let mut lay = p.cell_mut(&target).expect("resolved").layout.take().unwrap_or_default();
            let res = ics_core::route::route(&sch, &mut lay, p, &p.tech);
            p.cell_mut(&target).expect("resolved").layout = Some(lay);
            let r = res?;
            out["routed"] = json!(r.routed);
            out["failed"] = json!(r.failed);
        }
        self.dirty = true;
        Ok(out)
    }

    fn diff_layout(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let root = p.root.clone().ok_or("the project has not been saved")?;
        let rev = a.0.get("rev").and_then(Value::as_str).unwrap_or("HEAD");
        let old = if rev == "saved" {
            Project::open(&root).map_err(|e| e.to_string())?
        } else {
            ics_core::layout_diff::open_revision(&root, rev)?
        };
        let empty = ics_core::Layout::default();
        let d = ics_core::layout_diff::diff(old.layout(&r).unwrap_or(&empty), &old, layout_of(p, &r)?, p);
        let t = &p.tech;
        let um2 = |v: i128| sig(v as f64 / (t.dbu_per_um.max(1) as f64).powi(2));
        let layers: Vec<Value> = d
            .layers
            .iter()
            .map(|l| {
                let boxes = |reg: &ics_core::region::Region| -> Vec<Value> {
                    reg.parts().iter().take(MAX_LISTED).filter_map(|p| p.bbox()).map(|b| rect_um(t, b)).collect()
                };
                json!({
                    "layer": t.layers.get(l.layer).map_or(format!("#{}", l.layer), |d| d.name.clone()),
                    "added_um2": um2(l.added.area()),
                    "removed_um2": um2(l.removed.area()),
                    "added": boxes(&l.added),
                    "removed": boxes(&l.removed),
                })
            })
            .collect();
        Ok(json!({
            "cell": r.to_string(),
            "against": rev,
            "identical": d.is_empty(),
            "layers": layers,
            "placed": d.placed,
            "removed_placements": d.unplaced,
            "moved": d.moved,
        }))
    }

    fn run_signoff(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let runsets: Vec<Value> = t
            .signoff
            .iter()
            .map(|rs| json!({"name": rs.name, "tool": rs.tool.label(), "check": rs.check.label(), "deck": rs.deck}))
            .collect();
        let Some(name) = a.opt_str("runset")? else {
            return Ok(json!({"runsets": runsets}));
        };
        let rs = ics_core::signoff::find(t, name)?;
        let max = a.opt_num("max_markers")?.map_or(20, |n| n.max(0.0) as usize);
        let rep = ics_core::signoff::run(p, &r, rs, &ics_core::signoff::Tools::default())?;
        let mut out = json!({
            "cell": r.to_string(),
            "runset": rep.runset,
            "tool": rep.tool.label(),
            "summary": rep.summary(),
            "work_dir": rep.dir.display().to_string(),
            "deck_notes": rep.notes.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "millis": sig(rep.millis),
        });
        if let Some(d) = &rep.drc {
            let mut rules: Vec<Value> = vec![];
            let mut names: Vec<&str> = d.violations.iter().map(|v| v.rule.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            for name in names {
                let vs: Vec<_> = d.violations.iter().filter(|v| v.rule == name).collect();
                let markers: Vec<Value> = vs.iter().take(max).map(|v| rect_um(t, v.marker)).collect();
                rules.push(json!({"rule": name, "description": vs[0].description, "count": vs.len(), "markers_um": markers}));
            }
            out["clean"] = json!(d.violations.is_empty());
            out["violations"] = json!(d.violations.len());
            out["by_rule"] = json!(rules);
        }
        if let Some(l) = &rep.lvs {
            out["matched"] = json!(l.matched);
            out["messages"] = json!(l.messages.iter().take(MAX_LISTED).collect::<Vec<_>>());
        }
        self.keep(Verified::Signoff(Ok(rep)));
        Ok(out)
    }

    fn extract(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let x = extract(layout_of(p, &r)?, p, &p.tech);
        let mut out = extraction_json(&x, &p.tech, &r.to_string());
        out["cell"] = json!(r.to_string());
        self.keep(Verified::Extract(x));
        Ok(out)
    }

    fn run_lvs(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let c = p.cell(&r).expect("resolved");
        let sch = c.schematic.as_ref().ok_or("cell has no schematic view")?;
        let x = extract(layout_of(p, &r)?, p, t);
        let rep = compare(sch, &x, &LvsOptions::for_tech(&p.tech));
        let errors: Vec<Value> = rep
            .errors
            .iter()
            .map(|e| json!({"kind": format!("{:?}", e.kind), "message": e.message, "marker_um": e.marker.map(|m| rect_um(t, m))}))
            .collect();
        let out = json!({
            "cell": r.to_string(),
            "matched": rep.matched,
            "schematic_devices": rep.schematic_devices,
            "layout_devices": rep.layout_devices,
            "device_pairs": rep.devices,
            "net_pairs": rep.nets,
            "differences": errors.len(),
            "errors": errors,
            "warnings": rep.warnings.iter().map(|w| json!({"message": w.message, "marker_um": w.marker.map(|m| rect_um(t, m))})).collect::<Vec<_>>(),
        });
        self.keep(Verified::Lvs(x, rep));
        Ok(out)
    }

    fn post_layout_parasitics(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let pl = ics_core::pex::post_layout(p, &r)?;
        let x = extract(layout_of(p, &r)?, p, &p.tech);
        let caps: Vec<Value> = ics_core::pex::net_caps(&x, &p.tech)
            .iter()
            .take(50)
            .map(|c| json!({"layout_net": c.net, "fF": sig(c.farads * 1e15)}))
            .collect();
        let coupling: Vec<Value> = ics_core::pex::coupling_caps(&x, &p.tech)
            .iter()
            .take(50)
            .map(|c| json!({"layout_nets": [c.a, c.b], "fF": sig(c.farads * 1e15)}))
            .collect();
        Ok(json!({
            "cell": r.to_string(),
            "summary": pl.summary,
            "spice": pl.caps,
            "junctions": pl.junctions,
            "largest_net_caps": caps,
            "largest_coupling_caps": coupling,
        }))
    }

    fn emir(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        layout_of(p, &r)?;
        let currents = match a.get("currents") {
            Some(v) => v
                .as_object()
                .and_then(|m| m.iter().map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect::<Option<BTreeMap<_, _>>>())
                .ok_or("argument 'currents' must map device names to numbers")?,
            None => op_device_currents(p, &r)?,
        };
        let rep = ics_core::emir::analyze_cell(p, &r, a.str("net")?, &currents)?;
        let max = a.opt_num("max_items")?.map_or(20, |v| v.max(0.0) as usize);
        let mut out = emir_json(&rep, &p.tech, max);
        out["cell"] = json!(r.to_string());
        self.keep(Verified::Emir(Ok(rep)));
        Ok(out)
    }

    fn opc(&mut self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let tech = &p.tech;
        let layer = layer_index(tech, a.str("layer")?)?;
        let o = OpcOverrides {
            sigma: a.opt_num("sigma")?,
            threshold: a.opt_num("threshold")?,
            fragment: a.opt_num("fragment")?,
            max_out: a.opt_num("max_out")?,
            max_in: a.opt_num("max_in")?,
            iterations: a.opt_num("iterations")?.map(|v| v.max(1.0) as u32),
            gain: a.opt_num("gain")?,
        };
        let params = opc_params(tech, layer, &o);
        let top = layout_of(p, &r)?;
        let rep = ics_core::opc::correct(top, p, tech, &params)?;
        let apply = a.opt_bool("apply")?.unwrap_or(false);
        let max = a.opt_num("max_items")?.map_or(20, |v| v.max(0.0) as usize);
        let mut res = opc_json(&rep, &params, tech, max);
        res["cell"] = json!(r.to_string());
        if apply {
            let p = self.project_mut()?;
            let (out, added) = ics_core::opc::mask_layer(&mut p.tech, layer)?;
            let name = layer_name(&p.tech, out);
            let lay = p.cell_mut(&r).and_then(|c| c.layout.as_mut()).ok_or("cell has no layout view")?;
            let (removed, shapes) = ics_core::opc::write_mask(lay, &rep.mask, out);
            self.dirty = true;
            res["applied"] = json!({"layer": name, "layer_added": added, "shapes": shapes, "replaced_shapes": removed});
        }
        Ok(res)
    }

    fn export_gds(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        layout_of(p, &r)?;
        let path = PathBuf::from(a.str("path")?);
        let format = ics_core::gds::StreamFormat::from_path(&path);
        let bytes = ics_core::gds::write_stream_hier(
            format,
            &r.lib,
            p,
            std::slice::from_ref(&r),
            &ics_core::stream_map::export_tech(&p.tech),
        );
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let cells = descendants(p, &r).iter().filter(|c| p.layout(c).is_some()).count();
        Ok(json!({"path": path.display().to_string(), "bytes": bytes.len(), "cells": cells}))
    }

    fn import_gds(&mut self, a: &Args) -> Result<Value, String> {
        let path = PathBuf::from(a.str("path")?);
        let hint = match a.opt_str("lib")? {
            Some(l) => l.to_string(),
            None => path.file_stem().map_or("gds".into(), |s| s.to_string_lossy().to_string()),
        };
        let map = match a.opt_str("layermap")? {
            Some(m) => Some(std::fs::read_to_string(m).map_err(|e| format!("{m}: {e}"))?),
            None => None,
        };
        let p = self.project_mut()?;
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut warnings = vec![];
        let mut added_layers = vec![];
        if let Some(text) = &map {
            let r = ics_core::stream_map::apply_layer_map(&mut p.tech, text);
            warnings.extend(r.warnings.into_iter().map(|w| format!("layer map: {w}")));
            added_layers = r.added_layers;
        }
        let imp = ics_core::gds::read_stream_with(
            &data,
            &p.tech,
            "gds",
            ics_core::gds::ReadOptions { use_stream_map: true, ..Default::default() },
        )?;
        let skipped = imp.skipped_unknown_layers;
        warnings.extend(imp.warnings.iter().cloned());
        let (lib, tops) = p.add_gds_library(&hint, imp)?;
        let cells = p.libraries.iter().find(|l| l.name == lib).map_or(0, |l| l.cells.len());
        self.dirty = true;
        Ok(json!({
            "library": lib,
            "cells": cells,
            "top_cells": tops.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            "skipped_unknown_layer_shapes": skipped,
            "added_layers": added_layers,
            "warnings": warnings,
        }))
    }

    fn export_lef(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let path = PathBuf::from(a.str("path")?);
        let mut opts = ics_core::lefdef::AbstractOptions::default();
        if let Some(o) = a.opt_str("obs")? {
            opts.obs = o.parse()?;
        }
        if let Some(c) = a.opt_str("class")? {
            opts.class = c.to_string();
        }
        let (m, warnings) = ics_core::lefdef::generate_abstract(&r.cell, layout_of(p, &r)?, p, &p.tech, &opts);
        let text = ics_core::lefdef::write_lef(&p.tech, std::slice::from_ref(&m), true);
        std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        let pins: Vec<Value> = m
            .pins
            .iter()
            .map(|p| json!({"name": p.name, "direction": p.direction, "use": p.usage, "shapes": p.ports.len()}))
            .collect();
        Ok(json!({
            "path": path.display().to_string(),
            "size_um": pt_um(&p.tech, m.size),
            "pins": pins,
            "obs_shapes": m.obs.len(),
            "warnings": warnings,
        }))
    }

    fn import_lef(&mut self, a: &Args) -> Result<Value, String> {
        let path = PathBuf::from(a.str("path")?);
        let hint = match a.opt_str("lib")? {
            Some(l) => l.to_string(),
            None => path.file_stem().map_or("lef".into(), |s| s.to_string_lossy().to_string()),
        };
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let imp = ics_core::lefdef::import_lef(self.project_mut()?, &text, &hint)?;
        self.dirty = true;
        Ok(json!({
            "library": imp.lib,
            "cells": imp.cells.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            "warnings": imp.warnings,
        }))
    }

    fn export_def(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let path = PathBuf::from(a.str("path")?);
        let (text, d, mut warnings) = ics_core::lefdef::export_def(p, &r)?;
        std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut out = json!({
            "path": path.display().to_string(),
            "components": d.components.len(),
            "pins": d.pins.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            "nets": d.nets.len(),
        });
        if let Some(lp) = a.opt_str("lef_path")? {
            let macros = ics_core::lefdef::def_macros(p, &r);
            let (lef, w) = ics_core::lefdef::export_lef(p, &macros, &Default::default())?;
            std::fs::write(lp, lef).map_err(|e| format!("{lp}: {e}"))?;
            warnings.extend(w);
            out["lef_path"] = json!(lp);
            out["macros"] = json!(macros.iter().map(|c| c.to_string()).collect::<Vec<_>>());
        }
        out["warnings"] = json!(warnings);
        Ok(out)
    }

    fn import_def(&mut self, a: &Args) -> Result<Value, String> {
        let path = PathBuf::from(a.str("path")?);
        let lib = a.opt_str("lib")?.map(str::to_string);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let imp = ics_core::lefdef::import_def(self.project_mut()?, &text, lib.as_deref())?;
        self.dirty = true;
        Ok(json!({
            "cell": imp.cell.to_string(),
            "placed": imp.placed,
            "unplaced": imp.unplaced,
            "missing_macros": imp.missing,
            "pins": imp.pins,
            "nets": imp.nets,
            "warnings": imp.warnings,
        }))
    }

    fn run_skill(&mut self, a: &Args) -> Result<Value, String> {
        let (code, file) = (a.opt_str("code")?, a.opt_str("file")?);
        let current = match a.opt_str("cell")? {
            Some(c) => Some(find(self.project()?, c).ok_or_else(|| format!("no cell '{c}'"))?),
            None => None,
        };
        let p = self.project_mut()?;
        // Agent code is sandboxed: files only under the project directory, no environment.
        let mut session = ics_core::skill::Session::sandboxed(ics_core::skill::Sandbox {
            roots: p.root.iter().cloned().collect(),
            time_limit: Some(std::time::Duration::from_secs(120)),
        });
        let res = match (code, file) {
            (Some(code), None) => session.eval(p, code, None, current.as_ref()),
            (None, Some(file)) => session.load(p, Path::new(file), current.as_ref()),
            _ => return Err("give exactly one of 'code' or 'file'".into()),
        };
        let names = |m: &[CellRef]| m.iter().map(CellRef::to_string).collect::<Vec<_>>();
        match res {
            Ok(r) => {
                self.dirty |= !r.modified.is_empty();
                Ok(json!({"output": r.output, "value": r.value, "modified": names(&r.modified)}))
            }
            Err(e) => {
                self.dirty |= !e.modified.is_empty();
                let mut msg = format!("SKILL error: {e}");
                if !e.modified.is_empty() {
                    msg += &format!("\nmodified (edits kept): {}", names(&e.modified).join(", "));
                }
                if !e.output.is_empty() {
                    msg += &format!("\noutput:\n{}", e.output);
                }
                Err(msg)
            }
        }
    }
}

fn process_data(a: &Args) -> Result<Value, String> {
    use ics_core::process_data::{GENERIC180_SPEC, ProcessSpec, WatData, correlation, summarize};
    let read = |text: &str, path: &str| -> Result<Option<String>, String> {
        match (a.0.get(text).and_then(Value::as_str), a.0.get(path).and_then(Value::as_str)) {
            (Some(t), _) => Ok(Some(t.to_string())),
            (None, Some(p)) => std::fs::read_to_string(p).map(Some).map_err(|e| format!("{p}: {e}")),
            _ => Ok(None),
        }
    };
    let csv = read("csv", "path")?.ok_or("give the data as csv or path")?;
    let d = WatData::parse(&csv)?;
    let spec = ProcessSpec::from_toml(&read("spec", "spec_path")?.unwrap_or_else(|| GENERIC180_SPEC.to_string()))?;
    let wafers = d.wafers();
    let params: Vec<Value> = summarize(&d, &spec)
        .iter()
        .map(|s| {
            json!({"name": s.name, "n": s.n, "mean": sig(s.mean), "sigma": sig(s.std), "cp": s.cp.map(sig), "cpk": s.cpk.map(sig),
                "out_of_spec": s.out_of_spec, "control": {"center": sig(s.center), "ucl": sig(s.ucl), "lcl": sig(s.lcl)},
                "alarms": s.violations.iter().map(|v| json!({"rule": v.rule, "lot": wafers[v.wafer].0, "wafer": wafers[v.wafer].1,
                    "message": v.message})).collect::<Vec<_>>(),
                "corners": s.corners.iter().map(|c| json!({"corner": c.0, "value": c.1, "z": sig(c.2)})).collect::<Vec<_>>(),
                "corners_cover_3sigma": s.corners_cover})
        })
        .collect();
    let mut cors = vec![];
    for i in 0..d.params.len() {
        for j in i + 1..d.params.len() {
            let r = correlation(&d, i, j);
            if r.abs() >= 0.7 {
                cors.push(json!({"a": d.params[i], "b": d.params[j], "r": sig(r)}));
            }
        }
    }
    Ok(json!({"spec": spec.name, "sites": d.sites.len(), "wafers": wafers.len(), "params": params, "correlations": cors}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(p: &Project) -> Vec<CellRef> {
        let mut v: Vec<CellRef> = p.all_cells().map(|(r, _)| r).collect();
        v.sort();
        v
    }

    #[test]
    fn snapshot_keeps_only_descendants() {
        let p = ics_core::examples::demo_project();
        let top = find(&p, "inv_tiles").unwrap();
        let snap = snapshot(&p, Some(&top), None);
        let mut want = descendants(&p, &top);
        want.retain(|c| p.cell(c).is_some());
        want.sort();
        assert!(want.len() > 1, "{want:?}");
        assert_eq!(cells(&snap), want);
        assert!(find(&snap, "rc_lowpass").is_none());
        assert_eq!(snap.layout(&top), p.layout(&top));
        assert_eq!(snap.tech.name, p.tech.name);

        let empty = Layout::default();
        let alone = snapshot(&p, Some(&top), Some(&empty));
        assert_eq!(cells(&alone), vec![top.clone()]);
        assert_eq!(alone.layout(&top), Some(&empty));

        assert!(snapshot(&p, None, None).libraries.is_empty());
    }
}
