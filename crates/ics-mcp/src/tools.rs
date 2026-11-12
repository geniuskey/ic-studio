//! MCP tools over one in-memory IC Studio project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ics_core::drc::{parse_deck, run as run_drc_deck};
use ics_core::extract::{Extraction, extract};
use ics_core::hier::{Hier, LayoutSource, descendants, would_cycle};
use ics_core::layout::{Geometry, Layout, Trans};
use ics_core::lvs::{LvsOptions, compare};
use ics_core::pcell::{PCELL_LIB, PCell};
use ics_core::project::MANIFEST;
use ics_core::{Backend, Cell, CellRef, IRect, Project, Tech, sdl};
use ics_sim::{Dataset, SimOptions, SimResults};
use serde_json::{Map, Value, json};

const MAX_LISTED: usize = 500;

#[derive(Default)]
pub struct State {
    project: Option<Project>,
    dirty: bool,
    in_gui: bool,
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
        Some(b) => Err(format!("unknown backend '{b}' (use builtin or ngspice)")),
    }
}

fn sim_options(p: Option<&Project>, backend: Backend) -> SimOptions {
    let mut opts = SimOptions { backend, ..Default::default() };
    if let Some(exe) = p.and_then(|p| p.settings.ngspice_path.clone()) {
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
        "log_tail": tail,
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
    json!({
        "net_count": x.nets.len(),
        "device_count": x.devices.len(),
        "nets": nets,
        "devices": devices,
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
            PCell::Mos { pmos, w: len("w", if pmos { 2.0 } else { 1.0 })?, l: len("l", 0.18)?, fingers: fingers as u32 }
        }
        "poly_res" => PCell::PolyRes { w: len("w", 0.4)?, l: len("l", 5.0)? },
        "mom_cap" => PCell::MomCap { w: len("w", 5.0)?, h: len("h", 5.0)? },
        k => return Err(format!("unknown pcell kind '{k}' (use nmos, pmos, poly_res or mom_cap)")),
    })
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

pub fn definitions() -> Vec<Value> {
    let cell = cell_prop;
    let edit_cell = || json!({"type": "string", "description": "Target cell name or 'lib/cell'. Created (in library 'work' unless a lib is given) if it does not exist; a layout view is added if missing."});
    let backend = json!({"type": "string", "enum": ["builtin", "ngspice"], "description": "Simulator: the built-in engine (default unless the cell's setup says otherwise) or an external ngspice."});
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
            "project_info",
            "Overview of the current project: libraries and cells with their views (schematic, layout), technology name, database units per µm, grid, layer names and via names. Call this first to learn cell and layer names.",
            json!({}),
            &[],
        ),
        tool(
            "get_netlist",
            "SPICE deck generated from a cell's schematic, including the analyses of its saved simulation setup.",
            json!({"cell": cell()}),
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
                "kind": {"type": "string", "enum": ["nmos", "pmos", "poly_res", "mom_cap"]},
                "params": {
                    "type": "object",
                    "description": "Sizes in µm. nmos/pmos: w (per finger, default 1 / 2), l (default 0.18), fingers (default 1). poly_res: w (0.4), l (5). mom_cap: w (5), h (5).",
                    "properties": {"w": {"type": "number"}, "l": {"type": "number"}, "h": {"type": "number"}, "fingers": {"type": "integer", "minimum": 1}},
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
                "Schematic-driven layout: place every schematic MOS device that is not yet in the layout as a PCell instance named after it (PMOS row above NMOS row, DRC-clean spacing). Sources, R and C are skipped. Wiring is not done; use flylines to see what to connect. {NOT_SAVED}"
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
            "run_drc",
            "Run design rule checks on a cell layout (flattened hierarchy) with the technology rule deck. Violations are grouped by rule with up to max_markers marker boxes (µm) each.",
            json!({"cell": cell(), "max_markers": {"type": "integer", "minimum": 0, "description": "Markers listed per rule (default 20)."}}),
            &["cell"],
        ),
        tool(
            "extract",
            "Extract connectivity and transistors from a cell layout: nets (name, bbox µm), MOS devices (W/L µm, terminal nets) and the extracted circuit as SPICE.",
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
            "post_layout_parasitics",
            "Wire capacitance per net extracted from the layout and mapped to schematic nets via LVS: the SPICE capacitor lines that simulate adds with parasitics=true, a summary, and the largest net capacitances.",
            json!({"cell": cell()}),
            &["cell"],
        ),
        tool(
            "export_gds",
            "Write a cell layout and every cell it places to a GDSII file.",
            json!({"cell": cell(), "path": {"type": "string", "description": "Output .gds file path."}}),
            &["cell", "path"],
        ),
        tool(
            "import_gds",
            &format!(
                "Import a GDSII file as a new library (cells mapped to technology layers by GDS layer/datatype; unknown layers are skipped). {NOT_SAVED}"
            ),
            json!({
                "path": {"type": "string", "description": "GDSII file to read."},
                "lib": {"type": "string", "description": "Library name (defaults to the file name; made unique)."},
            }),
            &["path"],
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
    /// State around the project open in the desktop app (live mode).
    pub fn in_gui(project: Option<Project>, dirty: bool) -> Self {
        Self { project, dirty, in_gui: true }
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
            "project_info" => self.project_info(),
            "get_netlist" => self.get_netlist(a),
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
            "run_drc" => self.run_drc(a),
            "extract" => self.extract(a),
            "run_lvs" => self.run_lvs(a),
            "post_layout_parasitics" => self.post_layout_parasitics(a),
            "export_gds" => self.export_gds(a),
            "import_gds" => self.import_gds(a),
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
        p.cell_mut(&r).expect("resolved").layout.get_or_insert_default();
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
            },
            "libraries": libraries,
        }))
    }

    fn get_netlist(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let c = p.cell(&r).expect("resolved");
        let sch = c.schematic.as_ref().ok_or("cell has no schematic view")?;
        let nl = ics_core::netlist::netlist(sch);
        Ok(json!({"cell": r.to_string(), "deck": nl.deck(&r.to_string(), &c.sim), "warnings": nl.warnings}))
    }

    fn simulate(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let c = p.cell(&r).expect("resolved");
        let sch = c.schematic.as_ref().ok_or("cell has no schematic view")?;
        let nl = ics_core::netlist::netlist(sch);
        let mut setup = c.sim.clone();
        let mut parasitics = None;
        if a.opt_bool("parasitics")?.unwrap_or(false) || setup.parasitics {
            let (lines, summary) = ics_core::pex::post_layout_lines(p, &r)?;
            setup.extra = format!("{}\n* --- layout parasitics ---\n{lines}", setup.extra.trim_end());
            parasitics = Some(summary);
        }
        let deck = nl.deck(&r.to_string(), &setup);
        let opts = sim_options(Some(p), backend(a)?.unwrap_or(c.sim.backend));
        let analyses: Vec<_> = c.sim.enabled().cloned().collect();
        let res = ics_sim::simulate(&deck, &analyses, &opts).map_err(|e| e.to_string())?;
        let mut out = results_json(&res);
        out["cell"] = json!(r.to_string());
        out["netlist_warnings"] = json!(nl.warnings);
        if let Some(s) = parasitics {
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
        p.cell_mut(&r).expect("resolved").layout = if res.is_ok() { Some(layout) } else { original };
        let placed = res?;
        let flylines = sdl::ratsnest(&sch, layout_of(p, &r)?, p, &p.tech).len();
        self.dirty = true;
        Ok(json!({"cell": r.to_string(), "placed": placed, "skipped": plan.skipped, "unrouted_flylines": flylines}))
    }

    fn flylines(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let sch = p.cell(&r).and_then(|c| c.schematic.as_ref()).ok_or("cell has no schematic view")?;
        let fl = sdl::ratsnest(sch, layout_of(p, &r)?, p, t);
        let lines: Vec<Value> = fl.iter().map(|f| json!({"net": f.net, "a": pt_um(t, f.a), "b": pt_um(t, f.b)})).collect();
        Ok(json!({"cell": r.to_string(), "count": fl.len(), "flylines": lines}))
    }

    fn run_drc(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let lay = layout_of(p, &r)?;
        let max = a.opt_num("max_markers")?.map_or(20, |n| n.max(0.0) as usize);
        let deck = parse_deck(&t.deck_text(), t).map_err(|errs| {
            let msgs: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
            format!("technology rule deck has errors: {}", msgs.join("; "))
        })?;
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
        Ok(json!({
            "cell": r.to_string(),
            "clean": total == 0,
            "violations": total,
            "by_rule": rules,
            "rules_run": rep.rules_run,
            "shapes_checked": rep.shapes,
            "non_manhattan_skipped": rep.skipped,
            "millis": sig(rep.millis),
        }))
    }

    fn extract(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let x = extract(layout_of(p, &r)?, p, &p.tech);
        let mut out = extraction_json(&x, &p.tech, &r.to_string());
        out["cell"] = json!(r.to_string());
        Ok(out)
    }

    fn run_lvs(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let t = &p.tech;
        let c = p.cell(&r).expect("resolved");
        let sch = c.schematic.as_ref().ok_or("cell has no schematic view")?;
        let x = extract(layout_of(p, &r)?, p, t);
        let rep = compare(sch, &x, &LvsOptions::default());
        let errors: Vec<Value> = rep
            .errors
            .iter()
            .map(|e| json!({"kind": format!("{:?}", e.kind), "message": e.message, "marker_um": e.marker.map(|m| rect_um(t, m))}))
            .collect();
        Ok(json!({
            "cell": r.to_string(),
            "matched": rep.matched,
            "schematic_devices": rep.schematic_devices,
            "layout_devices": rep.layout_devices,
            "device_pairs": rep.devices,
            "net_pairs": rep.nets,
            "differences": errors.len(),
            "errors": errors,
        }))
    }

    fn post_layout_parasitics(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        let (lines, summary) = ics_core::pex::post_layout_lines(p, &r)?;
        let x = extract(layout_of(p, &r)?, p, &p.tech);
        let caps: Vec<Value> = ics_core::pex::net_caps(&x, &p.tech)
            .iter()
            .take(50)
            .map(|c| json!({"layout_net": c.net, "fF": sig(c.farads * 1e15)}))
            .collect();
        Ok(json!({"cell": r.to_string(), "summary": summary, "spice": lines, "largest_net_caps": caps}))
    }

    fn export_gds(&self, a: &Args) -> Result<Value, String> {
        let (p, r) = self.cell_arg(a)?;
        layout_of(p, &r)?;
        let path = PathBuf::from(a.str("path")?);
        let bytes = ics_core::gds::write_gds_hier(&r.lib, p, std::slice::from_ref(&r), &p.tech);
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
        let p = self.project_mut()?;
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let imp = ics_core::gds::read_gds(&data, &p.tech, "gds")?;
        let (skipped, warnings) = (imp.skipped_unknown_layers, imp.warnings.clone());
        let (lib, tops) = p.add_gds_library(&hint, imp)?;
        let cells = p.libraries.iter().find(|l| l.name == lib).map_or(0, |l| l.cells.len());
        self.dirty = true;
        Ok(json!({
            "library": lib,
            "cells": cells,
            "top_cells": tops.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            "skipped_unknown_layer_shapes": skipped,
            "warnings": warnings,
        }))
    }
}
