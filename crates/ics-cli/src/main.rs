//! `ics` — command line interface to IC Studio projects (automation / CI / scripting).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ics_core::units::format_eng;
use ics_core::{Backend, CellRef, Project};
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
}

#[derive(Clone, Copy, ValueEnum)]
enum Template {
    Empty,
    Demo,
}

#[derive(Subcommand)]
enum Cmd {
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
    /// Simulate a cell with its saved simulation setup.
    Sim {
        project: PathBuf,
        cell: String,
        #[arg(long, value_enum)]
        backend: Option<BackendArg>,
        /// Emit results as JSON on stdout.
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
    },
    /// Export a cell layout to GDSII.
    Gds {
        project: PathBuf,
        cell: String,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Run the basic built-in DRC on a cell layout.
    Drc { project: PathBuf, cell: String },
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
        println!("{}", serde_json::json!({"backend": res.backend, "elapsed_ms": res.elapsed_ms, "datasets": sets}));
        return;
    }
    eprintln!("backend: {}  ({:.1} ms)", res.backend, res.elapsed_ms);
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
                    println!("    {:<20} [{}] {}", c.name, views.join(", "), c.description);
                }
            }
        }
        Cmd::Netlist { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let nl = ics_core::netlist::netlist(sch);
            for w in &nl.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", nl.deck(&r.to_string(), &c.sim));
        }
        Cmd::Sim { project, cell, backend: b, json } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let nl = ics_core::netlist::netlist(sch);
            let deck = nl.deck(&r.to_string(), &c.sim);
            let mut opts = SimOptions { backend: c.sim.backend, ..Default::default() };
            if let Some(b) = b {
                opts.backend = backend(b);
            }
            if let Some(exe) = &p.settings.ngspice_path {
                opts.ngspice_exe = exe.clone();
            }
            let analyses: Vec<_> = c.sim.enabled().cloned().collect();
            let res = ics_sim::simulate(&deck, &analyses, &opts).map_err(|e| e.to_string())?;
            print_results(&res, json);
        }
        Cmd::Spice { file, backend: b, json } => {
            let deck = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            let opts = SimOptions { backend: backend(b), ..Default::default() };
            let res = ics_sim::simulate(&deck, &[], &opts).map_err(|e| e.to_string())?;
            print_results(&res, json);
        }
        Cmd::Gds { project, cell, out } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let lay = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let bytes = ics_core::gds::write_gds(&r.lib, &[(&r.cell, lay)], &p.tech);
            std::fs::write(&out, &bytes).map_err(|e| e.to_string())?;
            println!("wrote {} ({} shapes, {} bytes)", out.display(), lay.shapes.len(), bytes.len());
        }
        Cmd::Drc { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let lay = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let v = ics_core::layout::run_basic_drc(lay, &p.tech);
            for x in &v {
                println!("{}  at ({}, {})-({}, {})", x.rule, x.area.x0, x.area.y0, x.area.x1, x.area.y1);
            }
            println!("{} violation(s)", v.len());
            if !v.is_empty() {
                return Err("DRC failed".into());
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
