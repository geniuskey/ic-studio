//! `ics` — command line interface to IC Studio projects (automation / CI / scripting).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ics_core::hier::{LayoutSource, descendants};
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
        /// Add layout parasitic capacitances (also enabled by the saved setup).
        #[arg(long)]
        parasitics: bool,
    },
    /// Simulate a raw SPICE file using its own dot analyses.
    Spice {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "builtin")]
        backend: BackendArg,
        #[arg(long)]
        json: bool,
    },
    /// Export a cell layout and every cell it places to GDSII.
    Gds {
        project: PathBuf,
        cell: String,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Run DRC on a cell layout with the technology deck (or `--deck FILE`).
    Drc {
        project: PathBuf,
        cell: String,
        /// Rule deck file to use instead of the technology deck.
        #[arg(long)]
        deck: Option<PathBuf>,
    },
    /// Extract nets and transistors from a cell layout and print them as SPICE.
    Extract { project: PathBuf, cell: String },
    /// Compare a cell's layout with its schematic (non-zero exit on differences).
    Lvs { project: PathBuf, cell: String },
    /// Import a GDSII file as a new library. Layers the technology lacks are added to it.
    ImportGds {
        project: PathBuf,
        file: PathBuf,
        /// Library name (defaults to the file name; made unique).
        #[arg(long)]
        lib: Option<String>,
        /// Skip shapes on layers the technology does not define instead of adding the layers.
        #[arg(long)]
        skip_unknown_layers: bool,
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
    if rep.skipped > 0 {
        eprintln!("warning: {} non-Manhattan shape(s) were not checked", rep.skipped);
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
            let nl = ics_core::netlist::netlist(sch);
            for w in &nl.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", nl.deck(&r.to_string(), &c.sim));
        }
        Cmd::Sim { project, cell, backend: b, json, parasitics } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let nl = ics_core::netlist::netlist(sch);
            let mut setup = c.sim.clone();
            if parasitics || setup.parasitics {
                let (lines, summary) = ics_core::pex::post_layout_lines(&p, &r)?;
                eprintln!("{summary}");
                setup.extra = format!(
                    "{}
* --- layout parasitics ---
{lines}",
                    setup.extra.trim_end()
                );
            }
            let deck = nl.deck(&r.to_string(), &setup);
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
            p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let bytes = ics_core::gds::write_gds_hier(&r.lib, &p, std::slice::from_ref(&r), &p.tech);
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
        Cmd::Drc { project, cell, deck } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let lay = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let (text, origin) = match &deck {
                Some(f) => (std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?, f.display().to_string()),
                None => (p.tech.deck_text(), "technology deck".to_string()),
            };
            let d = ics_core::drc::parse_deck(&text, &p.tech).map_err(|errs| {
                errs.iter().map(|e| format!("{origin}:{}: {}", e.line, e.message)).collect::<Vec<_>>().join("\n")
            })?;
            let rep = ics_core::drc::run(&d, lay, &p, &p.tech);
            print_drc(&rep, &d, p.tech.dbu_per_um);
            if !rep.violations.is_empty() {
                return Err("DRC failed".into());
            }
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
            println!("* {} nets, {} transistors", x.nets.len(), x.devices.len());
        }
        Cmd::Lvs { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let lay = c.layout.as_ref().ok_or("cell has no layout")?;
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let x = ics_core::extract::extract(lay, &p, &p.tech);
            let rep = ics_core::lvs::compare(sch, &x, &ics_core::lvs::LvsOptions::default());
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
        Cmd::ImportGds { project, file, lib, skip_unknown_layers } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let opts = ics_core::gds::ReadOptions { add_unknown_layers: !skip_unknown_layers };
            let imp = ics_core::gds::read_gds_with(&data, &p.tech, "gds", opts)?;
            let (skipped, warnings) = (imp.skipped_unknown_layers, imp.warnings.clone());
            let layers_before = p.tech.layers.len();
            let hint = lib.unwrap_or_else(|| file.file_stem().map_or("gds".into(), |s| s.to_string_lossy().to_string()));
            let (name, tops) = p.add_gds_library(&hint, imp)?;
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
