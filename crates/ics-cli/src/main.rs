//! `ics` — command line interface to IC Studio projects (automation / CI / scripting).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ics_core::hier::{LayoutSource, descendants};
use ics_core::units::format_eng;
use ics_core::{Backend, CellRef, LibStorage, Project};
use ics_sim::{SimOptions, SimResults};

#[derive(Parser)]
#[command(name = "ics", version, about = "IC Studio command line interface")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum BackendArg {
    Builtin,
    Ngspice,
    Auto,
}

#[derive(Clone, Copy, ValueEnum)]
enum StorageArg {
    Gds,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum Template {
    Empty,
    Demo,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the MCP server for AI agents on stdio (`claude mcp add ic-studio -- ics mcp`).
    Mcp,
    /// Create a new project directory.
    New {
        dir: PathBuf,
        #[arg(long, value_enum, default_value = "demo")]
        template: Template,
    },
    /// List libraries, cells and views of a project.
    Info { project: PathBuf },
    /// Print the SPICE deck of a cell (`lib/cell` or just `cell`).
    Netlist { project: PathBuf, cell: String },
    /// Create or replace a cell's schematic from a SPICE netlist file; its analysis lines set the
    /// cell's simulation setup. Saves the project.
    Schematic {
        project: PathBuf,
        cell: String,
        netlist: PathBuf,
        /// Replace an existing schematic.
        #[arg(long)]
        overwrite: bool,
    },
    /// Simulate a cell with its saved simulation setup.
    Sim {
        project: PathBuf,
        cell: String,
        #[arg(long, value_enum)]
        backend: Option<BackendArg>,
        /// Emit results as JSON on stdout.
        #[arg(long)]
        json: bool,
        /// Add layout parasitic capacitances (also enabled by the saved setup).
        #[arg(long)]
        parasitics: bool,
        /// Run on both the built-in engine and ngspice and report how far they disagree.
        #[arg(long)]
        compare: bool,
        /// Largest relative error `--compare` accepts (non-zero exit above it).
        #[arg(long, default_value_t = 0.05)]
        tol: f64,
    },
    /// Monte Carlo on the built-in engine: process shifts and Pelgrom mismatch of every MOS,
    /// with statistics of waveform-calculator measurements.
    Mc {
        project: PathBuf,
        cell: String,
        #[arg(long, default_value_t = 100)]
        runs: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Measurement `analysis:expression`, e.g. `ac:bw(v(out))` or `op:v(out)` (repeatable).
        #[arg(long = "measure", required = true)]
        measures: Vec<String>,
        /// Threshold mismatch coefficient in V·m (5n = 5 mV·µm).
        #[arg(long, default_value = "5n")]
        avt: String,
        /// Current factor mismatch coefficient in m (10n = 1 %·µm).
        #[arg(long, default_value = "10n")]
        abeta: String,
        /// Global threshold sigma per polarity (V).
        #[arg(long, default_value = "20m")]
        vto_sigma: String,
        /// Global relative kp sigma per polarity.
        #[arg(long, default_value = "0.03")]
        kp_sigma: String,
        #[arg(long)]
        parasitics: bool,
        #[arg(long)]
        json: bool,
    },
    /// Simulate a raw SPICE file using its own dot analyses.
    Spice {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "builtin")]
        backend: BackendArg,
        #[arg(long)]
        json: bool,
        /// Run on both the built-in engine and ngspice and report how far they disagree.
        #[arg(long)]
        compare: bool,
        /// Largest relative error `--compare` accepts (non-zero exit above it).
        #[arg(long, default_value_t = 0.05)]
        tol: f64,
    },
    /// Export a cell layout and every cell it places to GDSII, or to OASIS when the output
    /// file ends in `.oas` / `.oasis`.
    Gds {
        project: PathBuf,
        cell: String,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// LEF abstracts: export cells as LEF macros, import LEF macros as layout cells.
    Lef {
        #[command(subcommand)]
        cmd: LefCmd,
    },
    /// DEF placements: export a top cell's components, pins and nets, import a DEF as a top cell.
    Def {
        #[command(subcommand)]
        cmd: DefCmd,
    },
    /// Route a cell's flylines on the metal stack and save the project (non-zero exit when some
    /// nets stay unrouted).
    Route {
        project: PathBuf,
        cell: String,
        /// First place the schematic devices missing from the layout (schematic-driven layout).
        #[arg(long)]
        place: bool,
    },
    /// Run DRC on a cell layout with the technology deck (or `--deck FILE`).
    Drc {
        project: PathBuf,
        cell: String,
        /// Rule deck file to use instead of the technology deck.
        #[arg(long)]
        deck: Option<PathBuf>,
        /// Apply verified quick fixes (top-level geometry only) and save the project first.
        #[arg(long)]
        fix: bool,
    },
    /// Compact a cell's layout toward the given directions (fixed items stay, constraints hold,
    /// no new DRC violations) and save the project.
    Compact {
        project: PathBuf,
        cell: String,
        /// Directions in order: left, right, down, up.
        #[arg(long, value_delimiter = ',', default_value = "left,down")]
        dir: Vec<String>,
        /// Do not infer symmetry constraints from the schematic's matched devices first.
        #[arg(long)]
        no_infer: bool,
    },
    /// Compare a cell's layout geometry (flattened, XOR per layer) with a git revision of the
    /// project or with another project directory. Non-zero exit when they differ.
    Diff {
        project: PathBuf,
        cell: String,
        /// Git revision to compare with.
        #[arg(long, default_value = "HEAD")]
        rev: String,
        /// Another project directory to compare with instead of a revision.
        #[arg(long)]
        against: Option<PathBuf>,
    },
    /// Size the cell's devices to meet its specs (`sim.sweep.specs`) with the variables of its
    /// optimizer setup (`sim.optimize`); `--apply` writes the best values into the schematic.
    Optimize {
        project: PathBuf,
        cell: String,
        /// Evaluate every candidate after layout generation, routing and extraction.
        #[arg(long)]
        post_layout: bool,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Place a cell's devices like another cell's layout with the same circuit topology (layout
    /// by example), optionally route it, and save.
    PlaceLike {
        project: PathBuf,
        cell: String,
        /// Reference cell with a finished layout.
        #[arg(long)]
        from: String,
        #[arg(long)]
        route: bool,
    },
    /// Extract nets, transistors, resistors and capacitors from a cell layout and print them as SPICE.
    Extract { project: PathBuf, cell: String },
    /// Compare a cell's layout with its schematic (non-zero exit on differences).
    Lvs { project: PathBuf, cell: String },
    /// IR drop and electromigration of one net of a cell layout (non-zero exit on EM
    /// violations). Pins labelled with the net name are the pads; device currents come from
    /// `--currents` or else from an operating-point simulation of the schematic.
    Emir {
        project: PathBuf,
        cell: String,
        #[arg(long)]
        net: String,
        /// JSON object of amps per layout device name (as `ics extract` prints them).
        #[arg(long)]
        currents: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Run an external sign-off DRC / LVS runset of the technology (KLayout, Calibre or netgen)
    /// on a cell; without a runset, list them. Non-zero exit on violations or a mismatch.
    Signoff {
        project: PathBuf,
        cell: String,
        /// Runset name (or a unique part of it).
        runset: Option<String>,
    },
    /// Read a GDSII or OASIS file and print hierarchy statistics and timings (nothing is written).
    GdsInfo { file: PathBuf },
    /// Import a GDSII or OASIS file as a new library stored as one GDS file. Layers the technology
    /// lacks are added to it.
    ImportGds {
        project: PathBuf,
        file: PathBuf,
        /// Library name (defaults to the file name; made unique).
        #[arg(long)]
        lib: Option<String>,
        /// Skip shapes on layers the technology does not define instead of adding the layers.
        #[arg(long)]
        skip_unknown_layers: bool,
        /// Store the library as one JSON directory per cell instead of a GDS file.
        #[arg(long)]
        json: bool,
    },
    /// Change how a library is stored on disk.
    LibStorage {
        project: PathBuf,
        lib: String,
        #[arg(value_enum)]
        storage: StorageArg,
    },
    /// Replace the technology with the layers of a KLayout layer properties file (.lyp).
    /// Layout shapes keep their GDS layer/datatype; layers missing from the file are kept.
    ImportTech {
        project: PathBuf,
        lyp: PathBuf,
        /// KLayout technology file (.lyt) supplying the name and database unit.
        #[arg(long)]
        lyt: Option<PathBuf>,
    },
    /// Replace the technology with a PDK file (TOML: layers, vias, devices, models; see
    /// `pdks/sky130A.toml`). Variables like ${PDK_ROOT} in its paths come from the environment.
    LoadPdk { project: PathBuf, pdk: PathBuf },
    /// Run a Cadence SKILL (.il) script on the project and save it. Prints what the script
    /// printed; non-zero exit on a SKILL error (the project is then not saved).
    Skill {
        project: PathBuf,
        file: PathBuf,
        /// Do not save the project afterwards.
        #[arg(long)]
        no_save: bool,
        /// Print {output, value, modified} as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ObsArg {
    Detailed,
    Block,
    None,
}

#[derive(Subcommand)]
enum LefCmd {
    /// Write the abstracts of cells (pins from labelled nets, OBS from the other metal) with a
    /// technology header; to stdout without `--out`.
    Export {
        project: PathBuf,
        #[arg(required = true)]
        cells: Vec<String>,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Obstructions: merged shapes, per-layer blocks around the pins, or none.
        #[arg(long, value_enum, default_value = "detailed")]
        obs: ObsArg,
        /// Macro CLASS.
        #[arg(long, default_value = "BLOCK")]
        class: String,
    },
    /// Import the macros of a LEF file as layout cells of a new library and save the project.
    Import {
        project: PathBuf,
        file: PathBuf,
        /// Library name (defaults to the file name; made unique).
        #[arg(long)]
        lib: Option<String>,
    },
}

#[derive(Subcommand)]
enum DefCmd {
    /// Write a top cell as DEF (die area, components, pins, nets); to stdout without `--out`.
    Export {
        project: PathBuf,
        cell: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Also write the LEF abstracts of the placed cells to this file.
        #[arg(long)]
        lef: Option<PathBuf>,
    },
    /// Import a DEF file as a new top cell placing the project cells named by its components'
    /// macros, and save the project.
    Import {
        project: PathBuf,
        file: PathBuf,
        /// Library of the new cell, searched first for macros (default `work`).
        #[arg(long)]
        lib: Option<String>,
    },
}

fn write_out(out: Option<&PathBuf>, text: &str) -> Result<(), String> {
    match out {
        Some(f) => std::fs::write(f, text).map_err(|e| format!("{}: {e}", f.display())),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

fn resolve(p: &Project, cell: &str) -> Result<CellRef, String> {
    if let Some((l, c)) = cell.split_once('/') {
        let r = CellRef::new(l, c);
        return p.cell(&r).map(|_| r.clone()).ok_or(format!("no cell '{cell}'"));
    }
    p.all_cells().find(|(_, c)| c.name == cell).map(|(r, _)| r).ok_or(format!("no cell '{cell}'"))
}

fn backend(b: BackendArg) -> Backend {
    match b {
        BackendArg::Builtin => Backend::Builtin,
        BackendArg::Ngspice => Backend::Ngspice,
        BackendArg::Auto => Backend::Auto,
    }
}

fn print_drc(rep: &ics_core::drc::DrcReport, deck: &ics_core::drc::Deck, dbu_per_um: i64) {
    const SHOWN: usize = 50;
    let um = |v: i64| v as f64 / dbu_per_um.max(1) as f64;
    let mut total = 0;
    for rule in &deck.rules {
        let vs: Vec<_> = rep.violations.iter().filter(|v| v.rule == rule.name).collect();
        let Some(first) = vs.first() else { continue };
        let count = rep.capped.iter().find(|(n, _)| *n == rule.name).map_or(vs.len(), |c| c.1);
        total += count;
        println!("{}: {} ({count})", rule.name, first.description);
        for v in vs.iter().take(SHOWN) {
            let m = v.marker;
            println!("  ({}, {}) - ({}, {}) µm", um(m.x0), um(m.y0), um(m.x1), um(m.y1));
        }
        if count > SHOWN {
            println!("  ... {} more", count - SHOWN);
        }
    }
    println!("{total} violation(s); {} rules on {} shapes in {:.0} ms", rep.rules_run, rep.shapes, rep.millis);
}

fn print_correlation(deck: &str, analyses: &[ics_core::Analysis], opts: &SimOptions, tol: f64) -> Result<(), String> {
    let (_, _, c) = ics_sim::correlate(deck, analyses, opts).map_err(|e| e.to_string())?;
    print!("{}", c.table());
    match c.worst() {
        Some(w) if w.rel > tol => {
            Err(format!("{} {} differs by {:.2} % (tolerance {:.2} %)", w.analysis, w.name, w.rel * 100.0, tol * 100.0))
        }
        _ => Ok(()),
    }
}

fn print_results(res: &SimResults, json: bool) {
    if json {
        let sets: Vec<_> = res
            .datasets
            .iter()
            .map(|d| {
                let vecs: serde_json::Map<String, serde_json::Value> = d
                    .vectors
                    .iter()
                    .map(|v| {
                        let val = match &v.im {
                            Some(im) => serde_json::json!({"re": v.re, "im": im}),
                            None => serde_json::json!(v.re),
                        };
                        (v.name.clone(), val)
                    })
                    .collect();
                serde_json::json!({"analysis": d.analysis, "title": d.title, "vectors": vecs})
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({"backend": res.backend, "elapsed_ms": res.elapsed_ms, "warnings": res.warnings, "datasets": sets})
        );
        return;
    }
    eprintln!("backend: {}  ({:.1} ms)", res.backend, res.elapsed_ms);
    for w in &res.warnings {
        eprintln!("warning: {w}");
    }
    for d in &res.datasets {
        println!("== {} ({} points)", d.title, d.len());
        if d.analysis == "op" {
            for v in &d.vectors {
                println!("  {:<16} {}", v.name, format_eng(v.re[0], v.kind.unit()));
            }
            for dev in &d.devices {
                let vals: Vec<String> = dev.values.iter().map(|(k, v)| format!("{k}={}", format_eng(*v, ""))).collect();
                println!("  {:<8} {:<11} {}", dev.name, dev.region.clone().unwrap_or_default(), vals.join(" "));
            }
        } else {
            let names: Vec<&str> = d.vectors.iter().map(|v| v.name.as_str()).collect();
            println!("  vectors: {}", names.join(", "));
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Mcp => ics_mcp::server::serve_stdio(),
        Cmd::New { dir, template } => {
            let name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or("project".into());
            let mut p = match template {
                Template::Empty => Project::new(&name),
                Template::Demo => {
                    let mut p = ics_core::examples::demo_project();
                    p.name = name;
                    p
                }
            };
            p.save_as(&dir).map_err(|e| e.to_string())?;
            println!("created {}", dir.display());
        }
        Cmd::Info { project } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            println!("{} — {}  (tech: {})", p.name, p.description, p.tech.name);
            for l in &p.libraries {
                println!("  {}/", l.name);
                for c in &l.cells {
                    let mut views = vec![];
                    if c.schematic.is_some() {
                        views.push("schematic");
                    }
                    if c.layout.is_some() {
                        views.push("layout");
                    }
                    let insts = c.layout.as_ref().map_or(0, |l| l.instances.len());
                    let insts = if insts > 0 { format!(" ({insts} instances)") } else { String::new() };
                    println!("    {:<20} [{}]{insts} {}", c.name, views.join(", "), c.description);
                }
            }
        }
        Cmd::Netlist { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let nl = ics_core::netlist::netlist_for(sch, &p.tech, &c.sim.corner);
            for w in &nl.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", nl.deck(&r.to_string(), &c.sim));
        }
        Cmd::Schematic { project, cell, netlist, overwrite } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&netlist).map_err(|e| format!("{}: {e}", netlist.display()))?;
            let imp = ics_core::spice_import::import(&text, &p.tech)?;
            let r = match resolve(&p, &cell) {
                Ok(r) => r,
                Err(_) => {
                    let (lib, name) = cell.split_once('/').unwrap_or(("work", cell.as_str()));
                    if p.library_mut(lib).is_none() {
                        p.add_library(lib)?;
                    }
                    p.add_cell(lib, ics_core::Cell::new(name))?
                }
            };
            let c = p.cell_mut(&r).unwrap();
            if c.schematic.is_some() && !overwrite {
                return Err(format!("{r} already has a schematic; pass --overwrite to replace it"));
            }
            imp.apply_to(&mut c.sim);
            c.schematic = Some(imp.schematic);
            for w in &imp.warnings {
                eprintln!("warning: {w}");
            }
            let c = p.cell(&r).unwrap();
            let nl = ics_core::netlist::netlist_for(c.schematic.as_ref().unwrap(), &p.tech, &c.sim.corner);
            for w in &nl.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", nl.deck(&r.to_string(), &c.sim));
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::Mc { project, cell, runs, seed, measures, avt, abeta, vto_sigma, kp_sigma, parasitics, json } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let mut deck = ics_core::netlist::netlist_for(sch, &p.tech, &c.sim.corner).deck(&r.to_string(), &c.sim);
            if parasitics || c.sim.parasitics {
                deck = ics_core::pex::post_layout(&p, &r)?.apply(&deck);
            }
            let num = |label: &str, v: &str| ics_core::units::parse_spice(v).ok_or(format!("--{label}: invalid number '{v}'"));
            let spec = ics_sim::montecarlo::McSpec {
                runs,
                seed,
                avt: num("avt", &avt)?,
                abeta: num("abeta", &abeta)?,
                vto_sigma: num("vto-sigma", &vto_sigma)?,
                kp_sigma: num("kp-sigma", &kp_sigma)?,
            };
            let ms = measures
                .iter()
                .map(|m| {
                    let (a, e) = m.split_once(':').ok_or(format!("--measure '{m}': write analysis:expression"))?;
                    Ok(ics_sim::montecarlo::Measure { analysis: a.trim().to_ascii_lowercase(), expr: e.trim().to_string() })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let analyses: Vec<_> = c.sim.enabled().cloned().collect();
            let res = ics_sim::montecarlo::monte_carlo(&deck, &analyses, &ms, &spec).map_err(|e| e.to_string())?;
            if json {
                let stats: Vec<_> = ms
                    .iter()
                    .zip(&res.stats)
                    .zip(&res.values)
                    .map(|((m, s), v)| {
                        serde_json::json!({"analysis": m.analysis, "expr": m.expr, "n": s.n, "mean": s.mean, "sigma": s.sigma,
                            "min": s.min, "max": s.max, "values": v.iter().map(|p| p.1).collect::<Vec<_>>()})
                    })
                    .collect();
                println!("{}", serde_json::json!({"runs": runs, "failed": res.failed.len(), "measures": stats}));
            } else {
                println!("Monte Carlo: {runs} runs, {} failed", res.failed.len());
                for (m, s) in ms.iter().zip(&res.stats) {
                    println!(
                        "  {:<5} {:<24} n={:<4} mean {}  sigma {}  min {}  max {}",
                        m.analysis,
                        m.expr,
                        s.n,
                        format_eng(s.mean, ""),
                        format_eng(s.sigma, ""),
                        format_eng(s.min, ""),
                        format_eng(s.max, "")
                    );
                }
                for (run, e) in res.failed.iter().take(5) {
                    eprintln!("run {run} failed: {e}");
                }
            }
        }
        Cmd::Sim { project, cell, backend: b, json, parasitics, compare, tol } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let nl = ics_core::netlist::netlist_for(sch, &p.tech, &c.sim.corner);
            let mut deck = nl.deck(&r.to_string(), &c.sim);
            if parasitics || c.sim.parasitics {
                let pl = ics_core::pex::post_layout(&p, &r)?;
                eprintln!("{}", pl.summary);
                deck = pl.apply(&deck);
            }
            let mut opts = SimOptions { backend: c.sim.backend, ..Default::default() };
            if let Some(b) = b {
                opts.backend = backend(b);
            }
            if let Some(exe) = &p.settings.ngspice_path {
                opts.ngspice_exe = exe.clone();
            }
            let analyses: Vec<_> = c.sim.enabled().cloned().collect();
            if compare {
                return print_correlation(&deck, &analyses, &opts, tol);
            }
            let res = ics_sim::simulate(&deck, &analyses, &opts).map_err(|e| e.to_string())?;
            print_results(&res, json);
        }
        Cmd::Spice { file, backend: b, json, compare, tol } => {
            let deck = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            let opts = SimOptions { backend: backend(b), ..Default::default() };
            if compare {
                return print_correlation(&deck, &[], &opts, tol);
            }
            let res = ics_sim::simulate(&deck, &[], &opts).map_err(|e| e.to_string())?;
            print_results(&res, json);
        }
        Cmd::Gds { project, cell, out } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let format = ics_core::gds::StreamFormat::from_path(&out);
            let bytes = ics_core::gds::write_stream_hier(
                format,
                &r.lib,
                &p,
                std::slice::from_ref(&r),
                &ics_core::stream_map::export_tech(&p.tech),
            );
            std::fs::write(&out, &bytes).map_err(|e| e.to_string())?;
            let layouts: Vec<_> = descendants(&p, &r).iter().filter_map(|c| p.layout(c)).collect();
            let shapes: usize = layouts.iter().map(|l| l.shapes.len()).sum();
            let insts: usize = layouts.iter().map(|l| l.instances.len()).sum();
            println!(
                "wrote {} ({} cells, {shapes} shapes, {insts} instances, {} bytes)",
                out.display(),
                layouts.len(),
                bytes.len()
            );
        }
        Cmd::Route { project, cell, place } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let sch = p.cell(&r).unwrap().schematic.clone().ok_or("cell has no schematic")?;
            let mut lay = p.cell_mut(&r).unwrap().layout.take().unwrap_or_default();
            if place {
                let plan = ics_core::sdl::plan(&sch, &p.tech);
                for s in &plan.skipped {
                    eprintln!("not placed: {s}");
                }
                let placed = ics_core::sdl::apply(&mut p, &mut lay, &plan)?;
                println!("placed {} device(s)", placed.len());
            }
            let rep = ics_core::route::route(&sch, &mut lay, &p, &p.tech)?;
            p.cell_mut(&r).unwrap().layout = Some(lay);
            p.save().map_err(|e| e.to_string())?;
            println!("{}", rep.summary());
            let left = ics_core::sdl::ratsnest(&sch, p.layout(&r).unwrap(), &p, &p.tech).len();
            println!("{left} flyline(s) left");
            if !rep.failed.is_empty() {
                return Err("some nets could not be routed".into());
            }
        }
        Cmd::Drc { project, cell, deck, fix } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let parsed = match &deck {
                Some(f) => ics_core::drc::load_deck(f, &p.tech)?,
                None => ics_core::drc::parse_any(&p.tech.deck_text(), &p.tech).map_err(|errs| {
                    errs.iter().map(|e| format!("technology deck:{}: {}", e.line, e.message)).collect::<Vec<_>>().join("\n")
                })?,
            };
            for n in &parsed.notes {
                eprintln!("note: {n}");
            }
            let d = parsed.deck;
            if fix {
                let mut lay = p.cell_mut(&r).unwrap().layout.take().unwrap_or_default();
                let done = ics_core::drc_fix::fix_all(&d, &mut lay, &p, &p.tech, 1000);
                p.cell_mut(&r).unwrap().layout = Some(lay);
                for t in &done {
                    println!("fixed: {t}");
                }
                if !done.is_empty() {
                    p.save().map_err(|e| e.to_string())?;
                }
            }
            let lay = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let rep = ics_core::drc::run(&d, lay, &p, &p.tech);
            print_drc(&rep, &d, p.tech.dbu_per_um);
            if !rep.violations.is_empty() {
                return Err("DRC failed".into());
            }
        }
        Cmd::Compact { project, cell, dir, no_infer } => {
            use ics_core::compact::Dir;
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let dirs = dir
                .iter()
                .map(|d| match d.as_str() {
                    "left" => Ok(Dir::Left),
                    "right" => Ok(Dir::Right),
                    "down" => Ok(Dir::Down),
                    "up" => Ok(Dir::Up),
                    _ => Err(format!("unknown direction '{d}' (left, right, down, up)")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let deck = ics_core::drc::parse_any(&p.tech.deck_text(), &p.tech).ok().map(|d| d.deck);
            let mut lay = p.cell_mut(&r).unwrap().layout.take().ok_or("cell has no layout")?;
            if !no_infer && let Some(sch) = p.cell(&r).and_then(|c| c.schematic.as_ref()) {
                let n = ics_core::constraints::infer(&mut lay, &p, sch);
                if n > 0 {
                    println!("inferred {n} symmetry constraint(s)");
                }
            }
            let um = |v: i64| v as f64 / p.tech.dbu_per_um.max(1) as f64;
            for d in dirs {
                let rep = ics_core::compact::compact(&mut lay, &p, &p.tech, deck.as_ref(), d, &Default::default());
                if let (Some(b), Some(a)) = (rep.before, rep.after) {
                    println!(
                        "{d:?}: {} move(s), extent {} x {} -> {} x {} um",
                        rep.moved,
                        um(b.width()),
                        um(b.height()),
                        um(a.width()),
                        um(a.height())
                    );
                }
            }
            p.cell_mut(&r).unwrap().layout = Some(lay);
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::Diff { project, cell, rev, against } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let (old, what) = match &against {
                Some(dir) => (Project::open(dir).map_err(|e| e.to_string())?, dir.display().to_string()),
                None => (ics_core::layout_diff::open_revision(&project, &rev)?, rev.clone()),
            };
            let empty = ics_core::Layout::default();
            let d = ics_core::layout_diff::diff(old.layout(&r).unwrap_or(&empty), &old, p.layout(&r).unwrap_or(&empty), &p);
            let names = |l: usize| p.tech.layers.get(l).map_or(format!("#{l}"), |d| d.name.clone());
            for line in d.summary(&names, p.tech.dbu_per_um) {
                println!("{line}");
            }
            if !d.is_empty() {
                return Err(format!("{r} differs from {what}"));
            }
            println!("{r}: no layout differences against {what}");
        }
        Cmd::Optimize { project, cell, post_layout, apply, json } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            if post_layout {
                p.cell_mut(&r).unwrap().sim.optimize.post_layout = true;
            }
            let mut opts = SimOptions::default();
            if let Some(exe) = &p.settings.ngspice_path {
                opts.ngspice_exe = exe.clone();
            }
            let mut progress = |e: &ics_sim::optimize::OptEval, k: usize| {
                if !json {
                    let vals: Vec<String> = e.values.iter().map(|v| format_eng(*v, "")).collect();
                    eprintln!(
                        "#{:<3} {:<40} objective {:.4}{}",
                        k + 1,
                        vals.join(" "),
                        e.objective,
                        if e.passed { "  pass" } else { "" }
                    );
                }
                true
            };
            let rep = ics_sim::optimize::run(&p, &r, &opts, &mut progress)?;
            let c = p.cell(&r).unwrap();
            let setup = c.sim.optimize.clone();
            let best = rep.best().ok_or("nothing evaluated")?.clone();
            if json {
                let specs: Vec<serde_json::Value> = c
                    .sim
                    .sweep
                    .specs
                    .iter()
                    .zip(&best.specs)
                    .map(|(s, v)| serde_json::json!({"name": s.name, "value": v.as_ref().ok(), "pass": v.as_ref().ok().and_then(|x| s.check(*x))}))
                    .collect();
                let vars: Vec<serde_json::Value> = setup
                    .vars
                    .iter()
                    .zip(&best.values)
                    .map(|(v, x)| serde_json::json!({"target": v.target, "value": x}))
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({"passed": best.passed, "evaluations": rep.evals.len(), "vars": vars, "specs": specs})
                );
            } else {
                for (v, x) in setup.vars.iter().zip(&best.values) {
                    println!("{} = {}", v.target, format_eng(*x, ""));
                }
                for (s, v) in c.sim.sweep.specs.iter().zip(&best.specs) {
                    match v {
                        Ok(x) => println!("{}: {}", s.name, format_eng(*x, "")),
                        Err(e) => println!("{}: error: {e}", s.name),
                    }
                }
            }
            if apply {
                let sch = c.schematic.clone().ok_or("cell has no schematic")?;
                p.cell_mut(&r).unwrap().schematic = Some(ics_sim::optimize::apply_values(&sch, &setup, &best.values));
                p.save().map_err(|e| e.to_string())?;
            }
            if !best.passed {
                return Err("no evaluated point meets every spec".into());
            }
        }
        Cmd::PlaceLike { project, cell, from, route } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let (target, reference) = (resolve(&p, &cell)?, resolve(&p, &from)?);
            let rep = ics_core::by_example::place_like(&mut p, &reference, &target)?;
            println!("placed {} device(s), {} tap(s), {} well / implant shape(s)", rep.placed, rep.taps, rep.shapes);
            for n in &rep.notes {
                eprintln!("note: {n}");
            }
            if route {
                let sch = p.cell(&target).unwrap().schematic.clone().ok_or("cell has no schematic")?;
                let mut lay = p.cell_mut(&target).unwrap().layout.take().unwrap_or_default();
                let res = ics_core::route::route(&sch, &mut lay, &p, &p.tech);
                p.cell_mut(&target).unwrap().layout = Some(lay);
                println!("{}", res?.summary());
            }
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::Extract { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let lay = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let x = ics_core::extract::extract(lay, &p, &p.tech);
            for w in &x.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", x.spice(&r.to_string(), p.tech.dbu_per_um));
            println!("* {} nets, {} transistors, {} resistors/capacitors", x.nets.len(), x.devices.len(), x.passives.len());
        }
        Cmd::Lvs { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let lay = c.layout.as_ref().ok_or("cell has no layout")?;
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let x = ics_core::extract::extract(lay, &p, &p.tech);
            let rep = ics_core::lvs::compare(sch, &x, &ics_core::lvs::LvsOptions::for_tech(&p.tech));
            println!(
                "{} schematic / {} layout devices, {} matched",
                rep.schematic_devices,
                rep.layout_devices,
                rep.devices.len()
            );
            for (s, l) in &rep.devices {
                println!("  device {s} = {l}");
            }
            for (s, l) in &rep.nets {
                println!("  net {s} = {l}");
            }
            for e in &rep.errors {
                println!("{:?}: {}", e.kind, e.message);
            }
            if !rep.matched {
                return Err(format!("LVS: {} difference(s)", rep.errors.len()));
            }
            println!("LVS: layout matches schematic");
        }
        Cmd::Emir { project, cell, net, currents, json } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let currents = match currents {
                Some(f) => {
                    let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                    serde_json::from_str(&text).map_err(|e| format!("{}: expected {{\"M1\": 1e-3, ...}}: {e}", f.display()))?
                }
                None => ics_mcp::tools::op_device_currents(&p, &r)?,
            };
            let rep = ics_core::emir::analyze_cell(&p, &r, &net, &currents)?;
            if json {
                println!("{}", ics_mcp::tools::emir_json(&rep, &p.tech, 50));
            } else {
                for w in &rep.warnings {
                    eprintln!("warning: {w}");
                }
                let um = |v: i64| v as f64 / p.tech.dbu_per_um.max(1) as f64;
                for l in &rep.loads {
                    println!(
                        "  {:<8} {:>10}  drop {}",
                        l.device,
                        format_eng(l.amps, "A"),
                        format_eng(rep.nodes[l.node].drop, "V")
                    );
                }
                for v in rep.violations.iter().take(50) {
                    let unit = if matches!(rep.branches[v.branch].kind, ics_core::emir::BranchKind::Via { .. }) {
                        "A/cut"
                    } else {
                        "A/µm"
                    };
                    let a = v.at;
                    println!(
                        "  EM {}: {} > {} {unit} at ({}, {}) - ({}, {}) µm",
                        v.layer,
                        format_eng(v.density, ""),
                        format_eng(v.limit, ""),
                        um(a.x0),
                        um(a.y0),
                        um(a.x1),
                        um(a.y1)
                    );
                }
                if rep.violations.len() > 50 {
                    println!("  ... {} more", rep.violations.len() - 50);
                }
                println!("{}", rep.summary(&p.tech));
            }
            if !rep.violations.is_empty() {
                return Err(format!("{} EM violation(s)", rep.violations.len()));
            }
        }
        Cmd::Signoff { project, cell, runset } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let Some(name) = runset else {
                if p.tech.signoff.is_empty() {
                    println!("technology {} has no sign-off runsets", p.tech.name);
                }
                for rs in &p.tech.signoff {
                    println!("{}: {} {} ({})", rs.name, rs.tool.label(), rs.check.label(), rs.deck);
                }
                return Ok(());
            };
            let rs = ics_core::signoff::find(&p.tech, &name)?;
            let rep = ics_core::signoff::run(&p, &r, rs, &ics_core::signoff::Tools::default())?;
            for n in &rep.notes {
                eprintln!("note: {n}");
            }
            let um = |v: i64| v as f64 / p.tech.dbu_per_um.max(1) as f64;
            let mut failed = false;
            if let Some(d) = &rep.drc {
                for v in d.violations.iter().take(200) {
                    let m = v.marker;
                    println!("{}: {} at ({}, {})-({}, {}) µm", v.rule, v.description, um(m.x0), um(m.y0), um(m.x1), um(m.y1));
                }
                if d.violations.len() > 200 {
                    println!("... {} more", d.violations.len() - 200);
                }
                failed = !d.violations.is_empty();
            }
            if let Some(l) = &rep.lvs {
                for m in &l.messages {
                    println!("{m}");
                }
                failed = l.matched != Some(true);
            }
            println!("reports in {}", rep.dir.display());
            if failed {
                return Err(rep.summary());
            }
            println!("{}", rep.summary());
        }
        Cmd::GdsInfo { file } => {
            let t0 = std::time::Instant::now();
            let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let t_io = t0.elapsed();
            let tech = ics_core::Tech::generic();
            let opts = ics_core::gds::ReadOptions { add_unknown_layers: true, ..Default::default() };
            let imp = ics_core::gds::read_stream_with(&data, &tech, "gds", opts)?;
            drop(data);
            let t_parse = t0.elapsed() - t_io;
            let shapes: usize = imp.cells.iter().map(|(_, l)| l.shapes.len()).sum();
            let insts: usize = imp.cells.iter().map(|(_, l)| l.instances.len()).sum();
            let tops = imp.top_cells();
            let src: std::collections::HashMap<ics_core::CellRef, ics_core::Layout> =
                imp.cells.into_iter().map(|(n, l)| (ics_core::CellRef::new("gds", n), l)).collect();
            let t1 = std::time::Instant::now();
            let mut hier = ics_core::hier::Hier::default();
            let mut flat = 0u64;
            for (k, t) in tops.iter().enumerate() {
                if let Some(i) = hier.get(&ics_core::CellRef::new("gds", t), &src) {
                    flat += i.flat_shapes;
                    if let Some(b) = i.bbox
                        && k < 10
                    {
                        println!("top {t}: {:.1} x {:.1} µm", b.width() as f64 / 1000.0, b.height() as f64 / 1000.0);
                    }
                }
            }
            if tops.len() > 10 {
                println!("... {} more top cells", tops.len() - 10);
            }
            println!("{} cells, {shapes} shapes, {insts} instances, {flat} flattened shapes", src.len());
            println!(
                "read {:.2} s, parse {:.2} s, index {:.2} s",
                t_io.as_secs_f64(),
                t_parse.as_secs_f64(),
                t1.elapsed().as_secs_f64()
            );
        }
        Cmd::Lef { cmd: LefCmd::Export { project, cells, out, obs, class } } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let refs = cells.iter().map(|c| resolve(&p, c)).collect::<Result<Vec<_>, _>>()?;
            let obs = match obs {
                ObsArg::Detailed => ics_core::lefdef::ObsMode::Detailed,
                ObsArg::Block => ics_core::lefdef::ObsMode::Block,
                ObsArg::None => ics_core::lefdef::ObsMode::None,
            };
            let (text, warnings) = ics_core::lefdef::export_lef(&p, &refs, &ics_core::lefdef::AbstractOptions { obs, class })?;
            write_out(out.as_ref(), &text)?;
            if let Some(f) = &out {
                eprintln!("wrote {} ({} macro(s))", f.display(), refs.len());
            }
            for w in &warnings {
                eprintln!("warning: {w}");
            }
        }
        Cmd::Lef { cmd: LefCmd::Import { project, file, lib } } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let hint = lib.unwrap_or_else(|| file.file_stem().map_or("lef".into(), |s| s.to_string_lossy().to_string()));
            let imp = ics_core::lefdef::import_lef(&mut p, &text, &hint)?;
            p.save().map_err(|e| e.to_string())?;
            let cells: Vec<&str> = imp.cells.iter().map(|c| c.cell.as_str()).collect();
            println!("library {}: {} cells ({})", imp.lib, cells.len(), cells.join(", "));
            for w in &imp.warnings {
                eprintln!("warning: {w}");
            }
        }
        Cmd::Def { cmd: DefCmd::Export { project, cell, out, lef } } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let (text, d, mut warnings) = ics_core::lefdef::export_def(&p, &r)?;
            write_out(out.as_ref(), &text)?;
            if let Some(f) = &out {
                eprintln!(
                    "wrote {} ({} components, {} pins, {} nets)",
                    f.display(),
                    d.components.len(),
                    d.pins.len(),
                    d.nets.len()
                );
            }
            if let Some(f) = &lef {
                let macros = ics_core::lefdef::def_macros(&p, &r);
                let (text, w) = ics_core::lefdef::export_lef(&p, &macros, &Default::default())?;
                std::fs::write(f, text).map_err(|e| format!("{}: {e}", f.display()))?;
                eprintln!("wrote {} ({} macro(s))", f.display(), macros.len());
                warnings.extend(w);
            }
            for w in &warnings {
                eprintln!("warning: {w}");
            }
        }
        Cmd::Def { cmd: DefCmd::Import { project, file, lib } } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let imp = ics_core::lefdef::import_def(&mut p, &text, lib.as_deref())?;
            p.save().map_err(|e| e.to_string())?;
            println!("cell {}: {} components placed, {} pins, {} nets", imp.cell, imp.placed, imp.pins, imp.nets);
            if imp.unplaced > 0 {
                eprintln!("warning: {} unplaced component(s) skipped", imp.unplaced);
            }
            for w in &imp.warnings {
                eprintln!("warning: {w}");
            }
        }
        Cmd::ImportGds { project, file, lib, skip_unknown_layers, json } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let opts = ics_core::gds::ReadOptions { add_unknown_layers: !skip_unknown_layers, use_stream_map: true };
            let imp = ics_core::gds::read_stream_with(&data, &p.tech, "gds", opts)?;
            let (skipped, warnings) = (imp.skipped_unknown_layers, imp.warnings.clone());
            let layers_before = p.tech.layers.len();
            let hint = lib.unwrap_or_else(|| file.file_stem().map_or("gds".into(), |s| s.to_string_lossy().to_string()));
            let (name, tops) = p.add_gds_library(&hint, imp)?;
            if json {
                p.set_library_storage(&name, LibStorage::Json)?;
            }
            p.save().map_err(|e| e.to_string())?;
            let n = p.libraries.iter().find(|l| l.name == name).map_or(0, |l| l.cells.len());
            println!("library {name}: {n} cells");
            let tops: Vec<&str> = tops.iter().map(|t| t.cell.as_str()).collect();
            println!("top cells: {}", tops.join(", "));
            let added: Vec<&str> = p.tech.layers[layers_before..].iter().map(|l| l.name.as_str()).collect();
            if !added.is_empty() {
                println!("added {} layer(s) to the technology: {}", added.len(), added.join(", "));
            }
            if skipped > 0 {
                eprintln!("warning: {skipped} shape(s) on layers unknown to the technology were skipped");
            }
            for w in &warnings {
                eprintln!("warning: {w}");
            }
        }
        Cmd::LibStorage { project, lib, storage } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let storage = match storage {
                StorageArg::Gds => LibStorage::Gds,
                StorageArg::Json => LibStorage::Json,
            };
            p.set_library_storage(&lib, storage)?;
            p.save().map_err(|e| e.to_string())?;
            println!("library {lib}: stored as {}", if storage == LibStorage::Gds { "GDS" } else { "JSON" });
        }
        Cmd::LoadPdk { project, pdk } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let loaded = ics_core::pdk::load(&pdk)?;
            let warnings = p.set_tech(loaded.tech)?;
            p.save().map_err(|e| e.to_string())?;
            let t = &p.tech;
            println!("PDK {}: {} layers, {} vias, {} devices", t.name, t.layers.len(), t.vias.len(), t.devices.len());
            if let Some(m) = &t.models {
                println!("models: {}", m.spice_line(""));
            }
            for w in loaded.warnings.iter().chain(&warnings) {
                eprintln!("warning: {w}");
            }
        }
        Cmd::ImportTech { project, lyp, lyt } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let read = |f: &PathBuf| std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()));
            let name = lyp.file_stem().map_or("tech".into(), |s| s.to_string_lossy().to_string());
            let mut tech = ics_core::tech_import::from_lyp(&read(&lyp)?, &name).map_err(|e| format!("{}: {e}", lyp.display()))?;
            if let Some(f) = &lyt {
                ics_core::tech_import::apply_lyt(&mut tech, &read(f)?).map_err(|e| format!("{}: {e}", f.display()))?;
            }
            let warnings = p.set_tech(tech)?;
            p.save().map_err(|e| e.to_string())?;
            println!("technology {}: {} layers, {} dbu per µm", p.tech.name, p.tech.layers.len(), p.tech.dbu_per_um);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
        }
        Cmd::Skill { project, file, no_save, json } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let res = match ics_core::skill::run_file(&mut p, &file) {
                Ok(r) => r,
                Err(e) => {
                    print!("{}", e.output);
                    return Err(e.to_string());
                }
            };
            if !no_save {
                p.save().map_err(|e| e.to_string())?;
            }
            let modified: Vec<String> = res.modified.iter().map(CellRef::to_string).collect();
            if json {
                println!("{}", serde_json::json!({"output": res.output, "value": res.value, "modified": modified}));
            } else {
                print!("{}", res.output);
                if !res.output.is_empty() && !res.output.ends_with('\n') {
                    println!();
                }
                if !modified.is_empty() {
                    eprintln!("modified {} cell(s): {}", modified.len(), modified.join(", "));
                }
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
