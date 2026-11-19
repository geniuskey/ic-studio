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
enum DeckCmd {
    /// Print where the deck comes from and its rule count.
    Show,
    /// Print the deck text.
    Print,
    /// Copy a deck file (IC Studio or SVRF) into the technology (unlinks a shared deck).
    Import { file: PathBuf },
    /// Write the technology's deck to a file.
    Export { file: PathBuf },
    /// Link the technology to a shared deck file.
    Link { file: PathBuf },
    /// Stop using the shared deck file; the project stores its deck itself.
    Unlink,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExtTemplate {
    /// A Python program with one tool and a menu command.
    Python,
    /// A SKILL script as a menu command, no program.
    Skill,
}

#[derive(Subcommand)]
enum ExtCmd {
    /// List the installed extensions (and folders that failed to load).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Install from a folder, a git repository URL or an id of the extension index.
    Install {
        source: String,
        /// Sub-folder of the repository holding extension.toml.
        #[arg(long)]
        path: Option<String>,
        /// Git tag or branch.
        #[arg(long)]
        rev: Option<String>,
        /// Replace an installed extension of the same id.
        #[arg(long)]
        force: bool,
    },
    /// Install an extension again from where it came from.
    Update {
        id: String,
    },
    /// Delete an installed extension.
    Remove {
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    /// Search the extension index (all entries without a query).
    Search {
        query: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Start a new extension in a folder.
    New {
        dir: PathBuf,
        /// Extension id (default: the folder name).
        #[arg(long)]
        id: Option<String>,
        #[arg(long, value_enum, default_value = "python")]
        template: ExtTemplate,
    },
    /// Check an extension folder's extension.toml without installing it.
    Check {
        dir: PathBuf,
    },
    /// Run a tool (an extension's `<id>__<tool>` or a built-in one) on a project; saves the
    /// project when the tool changed it.
    Call {
        project: PathBuf,
        tool: String,
        /// Arguments as a JSON object.
        #[arg(default_value = "{}")]
        arguments: String,
        #[arg(long)]
        no_save: bool,
    },
    /// Run an extension's menu command on a cell of a project.
    Run {
        project: PathBuf,
        /// `<extension id>/<command id>`.
        command: String,
        /// The cell `${cell}` stands for.
        #[arg(long)]
        cell: Option<String>,
        #[arg(long)]
        no_save: bool,
    },
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the MCP server for AI agents on stdio (`claude mcp add ic-studio -- ics mcp`).
    Mcp,
    /// Manage extensions: install, list, create and run them.
    Ext {
        #[command(subcommand)]
        cmd: ExtCmd,
    },
    /// Create a new project directory.
    New {
        dir: PathBuf,
        #[arg(long, value_enum, default_value = "demo")]
        template: Template,
    },
    /// List libraries, cells and views of a project.
    Info { project: PathBuf },
    /// Print the netlist of a cell (`lib/cell` or just `cell`): a SPICE deck, the same deck in
    /// Spectre, or CDL for LVS.
    Netlist {
        project: PathBuf,
        cell: String,
        #[arg(long, default_value = "spice")]
        format: ics_core::netlist::Format,
    },
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
    /// Import a hierarchical CDL netlist (Virtuoso auCdl: .SUBCKT, *.PININFO, X ... / cell,
    /// subcircuit parameters, ! globals) as a flat schematic of one cell. Saves the project.
    ImportCdl {
        project: PathBuf,
        cdl: PathBuf,
        /// Subcircuit to import (default: the top one).
        #[arg(long)]
        top: Option<String>,
        /// Cell to create or replace (default: work/<top>).
        #[arg(long)]
        cell: Option<String>,
        /// Replace an existing schematic.
        #[arg(long)]
        overwrite: bool,
        /// Import every subcircuit as its own cell (each flattened from itself).
        #[arg(long)]
        all: bool,
        /// Keep the hierarchy: a cell per subcircuit (in the library of --cell, default work),
        /// subcircuit instances as cell instances, a variant cell per other parameter values.
        #[arg(long, conflicts_with = "all")]
        hier: bool,
    },
    /// Write a cell's schematic as a SKILL script that recreates it in Virtuoso (load it in the
    /// CIW), with analogLib symbols or the symbols of a map file.
    ExportSkill {
        project: PathBuf,
        cell: String,
        /// Output file (.il).
        #[arg(short, long)]
        out: PathBuf,
        /// Virtuoso library to write the schematic into (default: the cell's library).
        #[arg(long)]
        lib: Option<String>,
        /// Symbol map (TOML [[symbol]] entries: kind, model, lib, cell, terms, params).
        #[arg(long)]
        map: Option<PathBuf>,
        /// Virtuoso user units per schematic grid point.
        #[arg(long, default_value_t = 0.1875)]
        scale: f64,
    },
    /// Write icsDump.il, the SKILL script that dumps a Virtuoso library for import-virtuoso
    /// (in the CIW: load("icsDump.il") then icsDumpLib("myLib" "/tmp/myLib.icsdump")).
    DumpSkill {
        /// Output file (.il; default: standard output).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Import a Virtuoso library dump written by icsDumpLib: schematics (flattened, Virtuoso
    /// placement kept when possible) and layouts (with their masters). Saves the project.
    ImportVirtuoso {
        project: PathBuf,
        /// A dump file, or a cds.lib / a directory with one / a library directory: then Virtuoso
        /// (ICS_VIRTUOSO or `virtuoso` on the PATH) dumps the library in batch mode.
        dump: PathBuf,
        /// Library to import from a cds.lib.
        #[arg(long)]
        lib: Option<String>,
        /// Symbol map (TOML [[symbol]] entries; default: analogLib).
        #[arg(long)]
        map: Option<PathBuf>,
        /// Virtuoso user units per schematic grid point.
        #[arg(long, default_value_t = 0.1875)]
        scale: f64,
        /// Replace existing schematics and layouts.
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
        /// Global process variation (JSON from `ics corners --variation`). Without it, the
        /// technology's variation is used if it has one, else --vto-sigma / --kp-sigma.
        #[arg(long)]
        variation: Option<PathBuf>,
        /// Use the generic sigmas and Avt / Abeta even when the technology has a measured
        /// variation or mismatch.
        #[arg(long)]
        generic: bool,
        #[arg(long)]
        parasitics: bool,
        #[arg(long)]
        json: bool,
    },
    /// Simulate a SPICE or Spectre (`.scs`, `simulator lang=spectre`) file using its own analyses.
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
    /// Check a schematic-driven layout against its schematic (missing, extra and changed
    /// devices, shorts, opens); `--update` places new devices and resizes changed ones, then
    /// saves. Non-zero exit when differences remain.
    Eco {
        project: PathBuf,
        cell: String,
        /// Update the layout from the schematic and save the project.
        #[arg(long)]
        update: bool,
        /// With --update: also delete layout devices that are no longer in the schematic.
        #[arg(long)]
        remove_extra: bool,
        /// Ignore opens (unrouted nets) for the exit status.
        #[arg(long)]
        allow_opens: bool,
    },
    /// Export a PCell Designer master as a SKILL pcDefinePCell for Cadence Virtuoso.
    PcellSkill {
        project: PathBuf,
        /// The master cell (made with the PCell Designer).
        cell: String,
        /// Virtuoso library of the PCell (default: the master's library).
        #[arg(long)]
        lib: Option<String>,
        /// Output file (.il; default: standard output).
        #[arg(short, long)]
        out: Option<PathBuf>,
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
    /// Manage the technology's DRC deck: show where it comes from, import or export a deck file,
    /// or link a shared deck file common to several projects (re-read on open, written when the
    /// deck is changed). Changes save the project.
    Deck {
        project: PathBuf,
        #[command(subcommand)]
        action: DeckCmd,
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
    /// Run the project's verification plan (ADE Assembler style): every test's corner / sweep /
    /// Monte Carlo setup (`sim.sweep`), each spec summarised over its points (worst value,
    /// margin, pass count). Without a plan, every cell with specs is a test. The run is saved
    /// in results/verify/ and compared with the previous one. Non-zero exit on a failing or
    /// unmeasurable spec.
    Verify {
        project: PathBuf,
        /// Run only these tests (names or cells; repeatable).
        #[arg(long)]
        test: Vec<String>,
        /// Add a cell to the plan as a test (repeatable), save the project and exit.
        #[arg(long)]
        add: Vec<String>,
        /// List the tests and exit.
        #[arg(long)]
        list: bool,
        /// Write JUnit XML (for CI test reports).
        #[arg(long)]
        junit: Option<PathBuf>,
        /// Compare with this saved run instead of the latest one.
        #[arg(long)]
        against: Option<PathBuf>,
        /// Do not save the run in the history.
        #[arg(long)]
        no_history: bool,
        /// Print the run as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run the cell's corner / Monte Carlo setup (`sim.sweep`) and predict the yield of each
    /// condition from a surrogate model of every spec, with the sensitivity of each spec to the
    /// varied parameters. Non-zero exit when a predicted yield is below `--target`.
    Yield {
        project: PathBuf,
        cell: String,
        /// Monte Carlo runs (overrides the setup's).
        #[arg(long)]
        runs: Option<u32>,
        /// Surrogate samples per condition.
        #[arg(long, default_value_t = 100_000)]
        samples: usize,
        /// Yield (0 to 1) every condition must reach.
        #[arg(long)]
        target: Option<f64>,
        /// Also draw the technology's measured process variation (`ics corners --apply`).
        #[arg(long)]
        process: bool,
        #[arg(long)]
        json: bool,
    },
    /// Silicon-to-simulation correlation: compare measured curves (CSV) with the cell's
    /// simulation and, with `--fit`, fit model, instance or design parameters to them. The
    /// setup comes from the cell (`sim.silicon`); the options below replace parts of it.
    Correlate {
        project: PathBuf,
        cell: String,
        /// Measured data, CSV with a header row.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Column of the x values (default: the first).
        #[arg(long)]
        x: Option<String>,
        /// Analysis giving the x axis: dc, ac or tran.
        #[arg(long)]
        analysis: Option<String>,
        /// `COLUMN=EXPR[@BIAS]`, e.g. `id_vg0.9=-i(vd)@VG.dc=0.9` (repeatable).
        #[arg(long = "trace")]
        traces: Vec<String>,
        /// Compare every trace on a log scale.
        #[arg(long)]
        log: bool,
        /// `TARGET=MIN:MAX[:log]`, e.g. `model:nmos.vto=0.3:0.7` or `M1.w=1u:4u` (repeatable).
        #[arg(long = "param")]
        params: Vec<String>,
        /// Fit the parameters to the measurements.
        #[arg(long)]
        fit: bool,
        #[arg(long, default_value_t = 40)]
        iterations: usize,
        /// Write the fitted model cards to this file.
        #[arg(long)]
        write_models: Option<PathBuf>,
        /// Write fitted instance parameters into the schematic; also saves the setup.
        #[arg(long)]
        apply: bool,
        /// Save the setup given by the options with the cell.
        #[arg(long)]
        save: bool,
        #[arg(long)]
        json: bool,
    },
    /// Process emulation: the cross-section of a cell's layout along a cut line, built by
    /// running a process flow (TOML; default: the generic 180 nm flow) step by step. Prints the
    /// material stack at points along the cut; `--svg` writes the picture.
    Xsection {
        project: PathBuf,
        cell: String,
        /// Process flow file (see `process/generic180.toml`).
        #[arg(long)]
        flow: Option<PathBuf>,
        /// Cut line `x0,y0,x1,y1` in µm (default: horizontal through the middle of the layout).
        #[arg(long)]
        cut: Option<String>,
        /// Stop after this many steps.
        #[arg(long)]
        steps: Option<usize>,
        /// Positions along the cut (µm) to print the stack at (repeatable; default: 5 points).
        #[arg(long = "at")]
        at: Vec<f64>,
        /// Write the cross-section as SVG.
        #[arg(long)]
        svg: Option<PathBuf>,
        /// Write the dopant depth profile at the first --at position (or the middle) as CSV.
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Analyse the column at this position (µm along the cut) as a pinned photodiode: the
        /// pinning voltage and storage capacitance of its buried n region.
        #[arg(long)]
        pinned_pd: Option<f64>,
        /// With --pinned-pd: write them into the technology's photodiode (`pinning`,
        /// `cap_area`) and save the project.
        #[arg(long, requires = "pinned_pd")]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Process data: WAT / PCM results (CSV with lot, wafer, site and parameter columns)
    /// against the process specification: capability (Cp, Cpk), out-of-spec sites, control
    /// chart rules on the wafer means, and whether the model corners cover the measured
    /// spread. Non-zero exit on out-of-spec sites or control-rule violations.
    ProcessData {
        data: PathBuf,
        /// Specification (TOML; default: the generic 180 nm WAT spec).
        #[arg(long)]
        spec: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Mismatch from silicon: fit Pelgrom's law (σ² = A² / (W L) + σ0²) to matched-pair data
    /// (CSV: device, w, l in µm, dvt in V, dbeta relative) per device: Avt and Abeta with
    /// their standard errors and the measured against fitted σ of every size.
    Mismatch {
        /// Matched-pair data (default: the synthetic generic 180 nm pairs).
        data: Option<PathBuf>,
        /// Project whose technology receives the coefficients with --apply.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Store the measured mismatch in the project's technology for Monte Carlo.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Corner models from silicon: fit model parameters so that the spec's simulated test
    /// structures reproduce the WAT data's means (tt) and its corners (ff, ss, fs, sf: primary
    /// parameters at ±zσ, the others following their correlation), and write a corner library.
    Corners {
        /// WAT / PCM data (CSV).
        data: PathBuf,
        /// Specification with `measure` per parameter and an `[extraction]` section (default:
        /// the generic 180 nm WAT spec).
        #[arg(long)]
        spec: Option<PathBuf>,
        /// Project whose technology models are the starting point (default: generic 180 nm).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Write the corner library (`.lib tt` ... sections, and `.lib mc`) to this file.
        #[arg(long)]
        lib: Option<PathBuf>,
        /// Write the global process variation (JSON, for `ics mc --variation`) to this file.
        #[arg(long)]
        variation: Option<PathBuf>,
        /// Store the process variation in the project's technology (needs --project); Monte
        /// Carlo then draws it in place of the generic sigmas.
        #[arg(long)]
        apply: bool,
        #[arg(long, default_value_t = 40)]
        iterations: usize,
        #[arg(long)]
        json: bool,
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
    /// Photodiodes (the technology's cross-section photodiode region) are listed as comments.
    Extract {
        project: PathBuf,
        cell: String,
        /// Also print the image sensor pixels found around the photodiodes: TG, RST, SF, SEL
        /// with W / L and junctions, the floating diffusion and the photodiode geometry.
        #[arg(long)]
        pixel: bool,
        /// With --pixel: simulate each pixel (up to 16) with charge injection for its FD and
        /// photodiode capacitance, conversion gain (µV/e-) and source follower gain.
        #[arg(long, requires = "pixel")]
        characterize: bool,
        /// Simulator for --characterize (auto: ngspice when the PDK models need it).
        #[arg(long, value_enum, default_value = "auto")]
        backend: BackendArg,
        /// Draw a schematic of the extraction (pixels in the textbook arrangement) with a test
        /// bench and transient setup, into the cell (or `--into`), and save the project.
        #[arg(long)]
        schematic: bool,
        /// Cell to write the schematic to (`lib/cell` or `cell`; created when missing).
        #[arg(long, requires = "schematic")]
        into: Option<String>,
        /// Replace an existing schematic.
        #[arg(long, requires = "schematic")]
        overwrite: bool,
        /// Draw the devices only, without test bench sources and analyses.
        #[arg(long, requires = "schematic")]
        no_testbench: bool,
        /// Test bench supply and gate high level (V).
        #[arg(long, default_value_t = 1.8)]
        vdd: f64,
        /// Test bench photocurrent per photodiode (A).
        #[arg(long, default_value_t = 10e-9)]
        iph: f64,
        /// Test bench temperature (°C): the simulations and the photodiode dark current.
        #[arg(long, default_value_t = 27.0)]
        temp: f64,
        /// Column line capacitance (F) of the readout noise analysis of --characterize.
        #[arg(long, default_value_t = 1e-12)]
        ccol: f64,
        /// Time (s) between the reset and the signal sample of correlated double sampling.
        #[arg(long, default_value_t = 2e-6)]
        cds_dt: f64,
    },
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
    /// Optical proximity correction of one layer of a cell layout: a Gaussian-blur / threshold
    /// litho model and iterative edge-fragment correction. Prints the edge placement error
    /// without and with correction; `--apply` writes the mask to `<layer>_opc` (added to the
    /// technology on first use), replacing the previous mask, and saves the project. The drawn
    /// layer is never changed.
    Opc {
        project: PathBuf,
        cell: String,
        /// Layer to correct, e.g. poly.
        #[arg(long)]
        layer: String,
        /// Write the mask to `<layer>_opc` and save the project.
        #[arg(long)]
        apply: bool,
        /// Blur sigma in µm (default 0.45 x min width).
        #[arg(long)]
        sigma: Option<f64>,
        /// Resist threshold, 0..1 (default 0.5).
        #[arg(long)]
        threshold: Option<f64>,
        /// Longest fragment in µm (default 2 x min width).
        #[arg(long)]
        fragment: Option<f64>,
        /// Furthest outward move in µm (default min spacing / 3).
        #[arg(long)]
        max_out: Option<f64>,
        /// Furthest inward move in µm (default min width / 4).
        #[arg(long)]
        max_in: Option<f64>,
        #[arg(long)]
        iterations: Option<u32>,
        #[arg(long)]
        gain: Option<f64>,
        #[arg(long)]
        json: bool,
    },
    /// Write the example projects, one per topic, to DIR/<topic> (existing ones are kept unless
    /// `--force`); without DIR, list the topics.
    Examples {
        dir: Option<PathBuf>,
        /// Overwrite example projects that already exist.
        #[arg(long)]
        force: bool,
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
        /// Virtuoso layer map file (`RX drawing 2 0` per line) applied before reading: its rows
        /// join the technology's stream map and layers it names that the technology lacks are added.
        #[arg(long)]
        layermap: Option<PathBuf>,
    },
    /// Change how a library is stored on disk.
    LibStorage {
        project: PathBuf,
        lib: String,
        #[arg(value_enum)]
        storage: StorageArg,
    },
    /// Replace the technology with one read from KLayout layer properties (`.lyp`) or a Cadence
    /// Virtuoso ASCII technology file (`.tf`, with `--drf` and `--layermap`). Layout shapes keep
    /// their GDS layer/datatype; layers missing from the file are kept.
    ImportTech {
        project: PathBuf,
        /// `.lyp` (KLayout) or `.tf` (Virtuoso) file.
        file: PathBuf,
        /// KLayout technology file (.lyt) supplying the name and database unit.
        #[arg(long)]
        lyt: Option<PathBuf>,
        /// Virtuoso display resource file (display.drf) for colours and fill patterns.
        #[arg(long)]
        drf: Option<PathBuf>,
        /// Stream layer map (Virtuoso layer map file) for the GDSII / OASIS numbers.
        #[arg(long)]
        layermap: Option<PathBuf>,
    },
    /// Show the results of a sign-off run made elsewhere on a cell: KLayout `.lyrdb` / `.lvsdb`,
    /// Calibre ASCII results database or LVS report, netgen report. Non-zero exit on violations
    /// or an LVS mismatch.
    Results { project: PathBuf, cell: String, file: PathBuf },
    /// Image sensor pixel optics: quantum efficiency, crosstalk and losses of a pixel stack
    /// (TOML, see `pixels/`), and with `--lux` the photocurrent, optionally written into a
    /// current source of a pixel circuit (`--set PROJECT CELL INSTANCE`).
    Pixel {
        /// Stack file; without one, the stack of `--cell` follows from the technology's cross
        /// section (`--project` and `--cell`).
        stack: Option<PathBuf>,
        /// Pixel cell for a stack built from the technology's cross section.
        #[arg(long)]
        cell: Option<String>,
        /// Print the stack built from the cross section (TOML) and stop.
        #[arg(long)]
        emit_stack: bool,
        /// Chief ray angle (degrees) and its azimuth from +x (degrees).
        #[arg(long, default_value_t = 0.0)]
        cra: f64,
        #[arg(long, default_value_t = 0.0)]
        azimuth: f64,
        /// Wavelength range and step (nm).
        #[arg(long, default_value = "380:1000:10")]
        range: String,
        /// Rays per pixel side (ray model).
        #[arg(long, default_value_t = 40)]
        rays: usize,
        /// Project whose layout fills the stack's `layout` entries (`[layout] cell = ...`).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Solver: `ray` (geometric rays, fast) or `rcwa` (rigorous coupled-wave analysis,
        /// includes diffraction; slower).
        #[arg(long, default_value = "ray")]
        method: String,
        /// Harmonics per direction for RCWA (odd; more is more accurate and slower).
        #[arg(long, default_value_t = 11)]
        harmonics: usize,
        /// Illuminance at the pixel (lux) for the photocurrent.
        #[arg(long)]
        lux: Option<f64>,
        /// Colour temperature of the (blackbody) illuminant (K).
        #[arg(long, default_value_t = 5000.0)]
        cct: f64,
        /// IR-cut filter edge (nm); 0 for none.
        #[arg(long, default_value_t = 650.0)]
        ir_cut: f64,
        /// Write the photocurrent of `--color` into the DC value of a current source:
        /// PROJECT CELL INSTANCE. Saves the project.
        #[arg(long, num_args = 3, value_names = ["PROJECT", "CELL", "INSTANCE"])]
        set: Option<Vec<String>>,
        /// Pixel colour for `--set`: R, Gr, Gb or B.
        #[arg(long, default_value = "Gr")]
        color: String,
        #[arg(long)]
        json: bool,
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
        Cmd::Ext { cmd } => ext(cmd)?,
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
        Cmd::Examples { dir, force } => match dir {
            None => {
                for t in &ics_core::examples::TOPICS {
                    println!("{:<18} {}", t.name, t.description);
                }
            }
            Some(dir) => {
                let written = ics_core::examples::write_topics(&dir, force).map_err(|e| e.to_string())?;
                for t in &ics_core::examples::TOPICS {
                    let state = if written.contains(&t.name) { "wrote" } else { "kept " };
                    println!("{state} {}", dir.join(t.name).display());
                }
            }
        },
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
        Cmd::Netlist { project, cell, format } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let circuit = p.circuit(&r)?;
            let sch = &*circuit;
            let (text, warnings) = ics_core::netlist::write(sch, &p.tech, &c.sim, &r.to_string(), format);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            print!("{text}");
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
            let circuit = p.circuit(&r)?;
            let nl = ics_core::netlist::netlist_for(&circuit, &p.tech, &c.sim.corner);
            for w in &nl.warnings {
                eprintln!("warning: {w}");
            }
            print!("{}", nl.deck(&r.to_string(), &c.sim));
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::ImportCdl { project, cdl, top, cell, overwrite, all, hier } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&cdl).map_err(|e| format!("{}: {e}", cdl.display()))?;
            if hier {
                let lib = cell.as_deref().and_then(|c| c.split_once('/')).map_or("work", |(l, _)| l).to_string();
                let h = ics_core::cadence::import_cdl_hier(&text, &p.tech, top.as_deref(), &lib)?;
                let cells = h.add_to(&mut p, &lib, overwrite)?;
                for (r, c) in cells.iter().zip(&h.cells) {
                    let sch = &c.imported.schematic;
                    let xs = sch.instances.iter().filter(|i| i.kind == ics_core::DeviceKind::Cell).count();
                    let devices = sch.instances.iter().filter(|i| i.kind.spice_prefix().is_some()).count() - xs;
                    println!("{r}: {devices} device(s), {xs} cell instance(s), {} pin(s)", sch.ports().len());
                }
                for w in h.warnings.iter().chain(h.cells.iter().flat_map(|c| &c.imported.warnings)) {
                    eprintln!("warning: {w}");
                }
                p.save().map_err(|e| e.to_string())?;
                return Ok(());
            }
            let parsed = ics_core::cadence::Cdl::parse(&text)?;
            let tops: Vec<Option<String>> =
                if all { parsed.subckts.iter().map(|s| Some(s.name.clone())).collect() } else { vec![top.clone()] };
            for t in tops {
                let imp = ics_core::cadence::import_cdl(&text, &p.tech, t.as_deref())?;
                let target = match (&cell, all) {
                    (Some(c), false) => c.clone(),
                    _ => format!("work/{}", if imp.flat.top.is_empty() { "cdl" } else { &imp.flat.top }),
                };
                let r = match resolve(&p, &target) {
                    Ok(r) => r,
                    Err(_) => {
                        let (lib, name) = target.split_once('/').unwrap_or(("work", target.as_str()));
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
                c.schematic = Some(imp.imported.schematic);
                let pins: Vec<String> =
                    imp.flat.pins.iter().map(|(n, d)| if d.is_empty() { n.clone() } else { format!("{n}:{d}") }).collect();
                println!("{r}: {} device(s) from {}, pins {}", imp.flat.devices, imp.flat.top, pins.join(" "));
                for w in &imp.imported.warnings {
                    eprintln!("warning: {w}");
                }
            }
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::DumpSkill { out } => match out {
            Some(f) => {
                std::fs::write(&f, ics_core::virtuoso_dump::DUMP_SKILL).map_err(|e| format!("{}: {e}", f.display()))?;
                eprintln!("wrote {}", f.display());
            }
            None => print!("{}", ics_core::virtuoso_dump::DUMP_SKILL),
        },
        Cmd::ImportVirtuoso { project, dump, lib, map, scale, overwrite } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let text = ics_core::virtuoso_batch::read_dump(&dump, lib.as_deref())?;
            let d = ics_core::virtuoso_dump::Dump::parse(&text).map_err(|e| format!("{}: {e}", dump.display()))?;
            let m = match &map {
                Some(f) => ics_core::cadence::SymbolMap::from_toml(
                    &std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
                )
                .map_err(|e| format!("{}: {e}", f.display()))?,
                None => ics_core::cadence::SymbolMap::analog_lib(),
            };
            let imp = ics_core::virtuoso_dump::import_dump(&d, &p.tech, &m, scale)?;
            let mut warnings = imp.warnings.clone();
            let done = ics_core::virtuoso_dump::apply(&mut p, &imp, overwrite, &mut warnings)?;
            for a in &done {
                let c = imp.cells.iter().find(|c| c.cell == a.cell).expect("imported");
                let mut views = vec![];
                if a.schematic {
                    views.push(format!(
                        "schematic ({} device(s){})",
                        c.devices,
                        if c.placed { ", Virtuoso placement" } else { "" }
                    ));
                }
                if a.layout {
                    let l = c.layout.as_ref().expect("has a layout");
                    views.push(format!("layout ({} shape(s), {} instance(s))", l.shapes.len(), l.instances.len()));
                }
                if !views.is_empty() {
                    println!("{}: {}", a.cell, views.join(", "));
                }
            }
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::ExportSkill { project, cell, out, lib, map, scale } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let sch = c.schematic.as_ref().ok_or("cell has no schematic")?;
            let m = match &map {
                Some(f) => ics_core::cadence::SymbolMap::from_toml(
                    &std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
                )
                .map_err(|e| format!("{}: {e}", f.display()))?,
                None => ics_core::cadence::SymbolMap::analog_lib(),
            };
            let (il, warnings) = ics_core::cadence::skill_schematic(sch, lib.as_deref().unwrap_or(&r.lib), &r.cell, &m, scale);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            std::fs::write(&out, il).map_err(|e| format!("{}: {e}", out.display()))?;
            eprintln!("wrote {}", out.display());
        }
        Cmd::Mc {
            project,
            cell,
            runs,
            seed,
            measures,
            avt,
            abeta,
            vto_sigma,
            kp_sigma,
            variation,
            generic,
            parasitics,
            json,
        } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let circuit = p.circuit(&r)?;
            let sch = &*circuit;
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
                process: match &variation {
                    Some(f) => {
                        let t = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
                        Some(serde_json::from_str(&t).map_err(|e| format!("{}: {e}", f.display()))?)
                    }
                    None if generic => None,
                    None => p.tech.variation.clone(),
                },
                mismatch: if generic { None } else { p.tech.mismatch.clone() },
            };
            if let Some(m) = &spec.mismatch
                && !json
            {
                eprintln!(
                    "mismatch: {} ({} device(s))",
                    if m.source.is_empty() { "measured" } else { &m.source },
                    m.devices.len()
                );
            }
            if let Some(v) = &spec.process
                && !json
            {
                eprintln!(
                    "process variation: {} ({} parameters)",
                    if v.source.is_empty() { "measured" } else { &v.source },
                    v.params.len()
                );
            }
            let ms = measures
                .iter()
                .map(|m| {
                    let (a, e) = m.split_once(':').ok_or(format!("--measure '{m}': write analysis:expression"))?;
                    Ok(ics_sim::montecarlo::Measure { analysis: a.trim().to_ascii_lowercase(), expr: e.trim().to_string() })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let analyses: Vec<_> = c.sim.enabled().cloned().collect();
            let res = ics_sim::montecarlo::monte_carlo(&deck, &analyses, &ms, &spec, None).map_err(|e| e.to_string())?;
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
            let circuit = p.circuit(&r)?;
            let sch = &*circuit;
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
            if let Some(exe) = &p.trusted_ngspice() {
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
            let mut deck = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            if ics_core::spectre::starts_in_spectre(Some(std::path::Path::new(&file)))
                && !ics_core::spectre::is_spectre(&deck, None)
            {
                // A `.scs` deck is Spectre from its first line.
                deck = format!("* {}\nsimulator lang=spectre\n{deck}", std::path::Path::new(&file).display());
            }
            let deck_dir = std::path::Path::new(&file).parent().map(|d| d.to_path_buf());
            let opts = SimOptions { backend: backend(b), deck_dir, ..Default::default() };
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
            let sch = p.circuit(&r)?.into_owned();
            let mut lay = p.cell_mut(&r).unwrap().layout.take().unwrap_or_default();
            if place {
                let plan = ics_core::sdl::plan(&sch, &p.tech);
                for s in &plan.skipped {
                    eprintln!("not placed: {s}");
                }
                let placed = ics_core::sdl::apply(&mut p, &mut lay, &plan)?;
                let pins = ics_core::eco::place_pins(&p, &mut lay, &sch);
                println!("placed {} device(s), {} pin(s)", placed.len(), pins.len());
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
        Cmd::Eco { project, cell, update, remove_extra, allow_opens } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let sch = p.circuit(&r)?.into_owned();
            if update {
                let mut lay = p.cell_mut(&r).unwrap().layout.take().unwrap_or_default();
                let res = ics_core::eco::update(&mut p, &mut lay, &sch, ics_core::eco::UpdateOptions { remove_extra });
                p.cell_mut(&r).unwrap().layout = Some(lay);
                let rep = res?;
                if rep.changed() {
                    p.save().map_err(|e| e.to_string())?;
                }
                println!("{}", rep.summary());
            }
            let lay = p.layout(&r).ok_or("cell has no layout")?;
            let c = ics_core::eco::check(&sch, lay, &p);
            for i in &c.issues {
                println!("[{}] {}", i.kind(), i.message());
            }
            println!("{}", c.summary());
            let bad = c.issues.iter().filter(|i| !(allow_opens && matches!(i, ics_core::eco::Issue::Open { .. }))).count();
            if bad > 0 {
                return Err(format!("{bad} difference(s) between layout and schematic"));
            }
        }
        Cmd::Deck { project, action } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            match action {
                DeckCmd::Show => {
                    match (p.drc_deck_path(), &p.tech.drc_deck) {
                        (Some(f), _) => {
                            println!("shared deck: {}", f.display());
                            if let Err(e) = p.reload_drc_deck() {
                                eprintln!("warning: {e} (using the deck derived from the layer rules)");
                            }
                        }
                        (None, Some(_)) => println!("deck stored in the project"),
                        (None, None) => println!("deck derived from the layer rules"),
                    }
                    let d = ics_core::drc::parse_any(&p.tech.deck_text(), &p.tech).map_err(|errs| {
                        errs.iter().map(|e| format!("deck:{}: {}", e.line, e.message)).collect::<Vec<_>>().join("\n")
                    })?;
                    println!("{} rule(s), {} derived layer(s)", d.deck.rules.len(), d.deck.derived.len());
                }
                DeckCmd::Print => print!("{}", p.tech.deck_text()),
                DeckCmd::Export { file } => {
                    std::fs::write(&file, p.tech.deck_text()).map_err(|e| format!("{}: {e}", file.display()))?;
                    println!("wrote {}", file.display());
                }
                DeckCmd::Import { file } => {
                    ics_core::drc::load_deck(&file, &p.tech)?;
                    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
                    let (text, _) = ics_core::drc::deck_file_text(&text, &file)?;
                    p.unlink_drc_deck();
                    p.tech.drc_deck = Some(text);
                    p.save().map_err(|e| e.to_string())?;
                    println!("imported {}", file.display());
                }
                DeckCmd::Link { file } => {
                    ics_core::drc::load_deck(&file, &p.tech)?;
                    p.link_drc_deck(&file)?;
                    p.save().map_err(|e| e.to_string())?;
                    println!("linked {}", p.drc_deck_path().unwrap().display());
                }
                DeckCmd::Unlink => match p.unlink_drc_deck() {
                    Some(f) => {
                        p.save().map_err(|e| e.to_string())?;
                        println!("unlinked {}", f.display());
                    }
                    None => println!("no shared deck linked"),
                },
            }
        }
        Cmd::PcellSkill { project, cell, lib, out } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let Some(ics_core::pcell::PCell::Template { def, .. }) = p.cell(&r).and_then(|c| c.pcell.as_ref()) else {
                return Err(format!("{r} is not a PCell Designer master"));
            };
            let (text, warnings) = def.to_skill(&p.tech, lib.as_deref().unwrap_or(&r.lib))?;
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            match out {
                Some(f) => {
                    std::fs::write(&f, &text).map_err(|e| format!("{}: {e}", f.display()))?;
                    eprintln!("wrote {}", f.display());
                }
                None => print!("{text}"),
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
        Cmd::Pixel {
            stack,
            cell,
            emit_stack,
            cra,
            azimuth,
            range,
            rays,
            project,
            method,
            harmonics,
            lux,
            cct,
            ir_cut,
            set,
            color,
            json,
        } => {
            use ics_optics::pixel::{COLORS, Method, Settings};
            let method = match method.to_ascii_lowercase().as_str() {
                "ray" => Method::Ray,
                "rcwa" => Method::Rcwa { harmonics },
                other => return Err(format!("unknown --method '{other}' (ray, rcwa)")),
            };
            let (text, stack) = match (&stack, &cell) {
                (Some(f), None) => (std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?, f.clone()),
                (None, Some(c)) => {
                    let pdir = project.as_ref().ok_or("--cell needs --project")?;
                    let p = Project::open(pdir).map_err(|e| e.to_string())?;
                    let xs = p.tech.cross_section.as_ref().ok_or(format!("technology {} has no cross section", p.tech.name))?;
                    let boundary = p.tech.layer_index("boundary").map(|_| "boundary");
                    let t = xs.pixel_stack_toml(&p.tech.name, c, boundary)?;
                    if emit_stack {
                        print!("{t}");
                        return Ok(());
                    }
                    (t, PathBuf::from(format!("{c} (cross section)")))
                }
                _ => return Err("give a stack file, or --project and --cell for one from the technology's cross section".into()),
            };
            let mut st = ics_optics::PixelStack::from_toml(&text).map_err(|e| format!("{}: {e}", stack.display()))?;
            if let Some(src) = st.layout.clone() {
                let pdir =
                    project.as_ref().ok_or(format!("{}: reads the layout of {}; give --project", stack.display(), src.cell))?;
                let p = Project::open(pdir).map_err(|e| e.to_string())?;
                let r = resolve(&p, &src.cell)?;
                let px = ics_core::pixel_layout::PixelLayout::new(&p, &r, src.boundary.as_deref())?;
                st.fill_from_layout(px.size_um(), |e| px.rects_um(e))?;
                eprintln!(
                    "{r}: {:.2} x {:.2} um pixel, photodiode {} rectangle(s)",
                    px.size_um().0,
                    px.size_um().1,
                    st.photodiode.rects.len()
                );
            }
            let r: Vec<f64> = range
                .split(':')
                .map(|v| v.parse::<f64>().map_err(|_| format!("bad --range '{range}'")))
                .collect::<Result<_, _>>()?;
            let [a, b, step] = r[..] else { return Err("--range is START:STOP:STEP in nm".into()) };
            if !(step > 0.0 && b >= a) {
                return Err("--range is START:STOP:STEP in nm".into());
            }
            let n = ((b - a) / step + 1e-9).floor() as usize;
            let wavelengths: Vec<f64> = (0..=n).map(|i| (a + i as f64 * step) / 1000.0).collect();
            let res = ics_optics::simulate(
                &st,
                &Settings { wavelengths: wavelengths.clone(), cra, azimuth, rays_per_side: rays, method },
            )?;
            let currents: Option<Vec<f64>> = lux.map(|lux| {
                let cut = (ir_cut > 0.0).then_some(ir_cut / 1000.0);
                let sp = ics_optics::light::Spectrum::blackbody(&wavelengths, cct, lux, cut);
                (0..4).map(|c| sp.photocurrent(&res.qe[c], st.pitch * st.pitch)).collect()
            });
            if json {
                let mut v = serde_json::to_value(&res).map_err(|e| e.to_string())?;
                if let Some(cur) = &currents {
                    v["photocurrent"] = serde_json::json!({"lux": lux, "cct": cct, "ir_cut_nm": ir_cut, "amps": cur});
                }
                println!("{v}");
            } else {
                match method {
                    Method::Ray => {
                        println!("{}: pitch {} um, CRA {cra} deg, ray model, {} rays per Bayer cell", st.name, st.pitch, res.rays)
                    }
                    Method::Rcwa { harmonics } => {
                        println!(
                            "{}: pitch {} um, CRA {cra} deg, RCWA with {harmonics} x {harmonics} harmonics",
                            st.name, st.pitch
                        )
                    }
                }
                let elec = st.silicon.is_some();
                println!(
                    "{:>6} {:>7} {:>7} {:>7} {:>7}   crosstalk (Gr){}",
                    "nm",
                    "QE R",
                    "QE Gr",
                    "QE Gb",
                    "QE B",
                    if elec { "  electrical crosstalk (Gr)" } else { "" }
                );
                let every = ((20.0 / step).round() as usize).max(1);
                for (i, l) in wavelengths.iter().enumerate().step_by(every) {
                    println!(
                        "{:>6.0} {:>7.3} {:>7.3} {:>7.3} {:>7.3}   {:.3}{}",
                        l * 1000.0,
                        res.qe[0][i],
                        res.qe[1][i],
                        res.qe[2][i],
                        res.qe[3][i],
                        res.crosstalk[1][i],
                        if elec { format!("          {:.3}", res.crosstalk_electrical[1][i]) } else { String::new() }
                    );
                }
                for (c, name) in COLORS.iter().enumerate() {
                    let (k, peak) = res.qe[c].iter().enumerate().fold((0, 0.0), |m, (k, &q)| if q > m.1 { (k, q) } else { m });
                    println!("peak {name:<2} {:.3} at {:.0} nm", peak, wavelengths[k] * 1000.0);
                }
                if let Some(cur) = &currents {
                    println!("photocurrent at {} lux, {cct} K, IR cut {ir_cut} nm:", lux.unwrap_or(0.0));
                    for (c, name) in COLORS.iter().enumerate() {
                        println!("  {name:<2} {:.4e} A", cur[c]);
                    }
                }
            }
            if let Some(set) = set {
                let cur = currents.ok_or("--set needs --lux")?;
                let c = COLORS
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&color))
                    .ok_or(format!("--color must be one of {COLORS:?}"))?;
                let mut p = Project::open(std::path::Path::new(&set[0])).map_err(|e| e.to_string())?;
                let r = resolve(&p, &set[1])?;
                let sch = p.cell_mut(&r).and_then(|c| c.schematic.as_mut()).ok_or("cell has no schematic")?;
                let inst = sch
                    .instances
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(&set[2]) && i.kind == ics_core::schematic::DeviceKind::ISource)
                    .ok_or(format!("no current source '{}' in {r}", set[2]))?;
                let value = format!("{:.6e}", cur[c]);
                inst.params.insert("dc".into(), value.clone());
                p.save().map_err(|e| e.to_string())?;
                eprintln!("{r}: {} dc = {value} A ({} pixel)", set[2], COLORS[c]);
            }
        }
        Cmd::Results { project, cell, file } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let rep = ics_core::signoff::import_results(&file, &text, p.tech.dbu_per_um, &r.cell)?;
            println!("{}", rep.summary());
            if let Some(d) = &rep.drc {
                let um = |v: i64| v as f64 / p.tech.dbu_per_um as f64;
                for v in &d.violations {
                    let m = v.marker;
                    println!(
                        "  {:<16} ({:.3}, {:.3})-({:.3}, {:.3}) {}",
                        v.rule,
                        um(m.x0),
                        um(m.y0),
                        um(m.x1),
                        um(m.y1),
                        v.description
                    );
                }
                if !d.violations.is_empty() {
                    return Err("DRC failed".into());
                }
            }
            if let Some(l) = &rep.lvs {
                for m in &l.messages {
                    println!("  {m}");
                }
                if l.matched != Some(true) {
                    return Err("LVS failed".into());
                }
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
            if !no_infer && let Ok(sch) = p.circuit(&r) {
                let n = ics_core::constraints::infer(&mut lay, &p, &sch);
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
        Cmd::Verify { project, test, add, list, junit, against, no_history, json } => {
            use ics_core::verify::{self, Status, TestDef};
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            if !add.is_empty() {
                for a in &add {
                    let r = resolve(&p, a)?;
                    if p.settings.verification.tests.iter().any(|t| verify::find_cell(&p, &t.cell).as_ref() == Some(&r)) {
                        return Err(format!("{r} is already a test"));
                    }
                    if p.settings.verification.tests.is_empty() {
                        // Keep the tests that ran by default.
                        let defaults = verify::default_tests(&p);
                        p.settings.verification.tests = defaults.iter().map(|(_, r)| TestDef::new(&r.to_string())).collect();
                    }
                    if !p.settings.verification.tests.iter().any(|t| t.cell == r.to_string()) {
                        p.settings.verification.tests.push(TestDef::new(&r.to_string()));
                    }
                    println!("added {r}");
                }
                p.save().map_err(|e| e.to_string())?;
                return Ok(());
            }
            let mut tests = p.settings.verification.resolve(&p);
            if list {
                if p.settings.verification.is_empty() {
                    println!("no plan: every cell with specs is a test");
                }
                for (n, r) in &tests {
                    let c = p.cell(r).unwrap();
                    println!("{n:<20} {r:<28} {} spec(s)", c.sim.sweep.specs.len());
                }
                for t in p.settings.verification.tests.iter().filter(|t| !t.enabled) {
                    println!("{:<20} {:<28} (disabled)", t.display_name(), t.cell);
                }
                return Ok(());
            }
            if !test.is_empty() {
                tests.retain(|(n, r)| test.iter().any(|t| t == n || *t == r.to_string() || *t == r.cell));
            }
            if tests.is_empty() {
                return Err("no tests: give cells specs in their sweep setup, or add tests with --add".into());
            }
            let mut opts = SimOptions::default();
            if let Some(exe) = &p.trusted_ngspice() {
                opts.ngspice_exe = exe.clone();
            }
            let progress = |name: &str, done: usize, total: usize| {
                if !json && (done.is_multiple_of(50) || done == total) {
                    eprintln!("{name}: {done}/{total} point(s)");
                }
            };
            let run = ics_sim::verify::run(&p, &tests, &opts, &progress);
            let root = p.root.clone();
            let before = match (&against, &root) {
                (Some(f), _) => Some((f.clone(), verify::load_run(f)?)),
                (None, Some(r)) => verify::list_history(r).pop().and_then(|f| verify::load_run(&f).ok().map(|b| (f, b))),
                _ => None,
            };
            let changes = before.as_ref().map(|(_, b)| run.compare(b)).unwrap_or_default();
            if json {
                let mut v = serde_json::to_value(&run).map_err(|e| e.to_string())?;
                v["status"] = serde_json::json!(run.status().label());
                v["changes"] = changes
                    .iter()
                    .map(|c| {
                        serde_json::json!({"test": c.test, "spec": c.spec, "before": c.before.map(|s| s.label()),
                            "after": c.after.label(), "worst_before": c.worst_before, "worst_after": c.worst_after,
                            "regression": c.is_regression()})
                    })
                    .collect();
                println!("{v}");
            } else {
                println!("verify {}: {} test(s)", run.time, run.tests.len());
                for t in &run.tests {
                    println!("{} ({}): {} point(s), {:.1} s", t.name, t.cell, t.points.len(), t.seconds);
                    if let Some(e) = &t.error {
                        println!("  ERROR {e}");
                    }
                    for n in &t.notes {
                        println!("  note: {n}");
                    }
                    let w = t.specs.iter().map(|s| s.name.len()).max().unwrap_or(4);
                    for s in &t.specs {
                        let at = s.worst_point.and_then(|k| t.points.get(k)).map_or(String::new(), |p| format!(" at {p}"));
                        let detail = match s.status() {
                            Status::Info => {
                                format!("range {} .. {}", verify::fmt(s.range.map(|r| r.0)), verify::fmt(s.range.map(|r| r.1)))
                            }
                            _ if s.worst.is_some() => {
                                format!("worst {} (margin {}){at}", verify::fmt(s.worst), verify::fmt(s.margin))
                            }
                            _ => format!("{} point(s) not measured: {}", s.errors, s.first_error),
                        };
                        let mut line = format!(
                            "  {:<5} {:<w$}  {:<14} {detail}  {}/{}",
                            s.status().label(),
                            s.name,
                            s.limits(),
                            s.passed,
                            s.values.len()
                        );
                        if s.errors > 0 && s.worst.is_some() {
                            line += &format!(", {} not measured", s.errors);
                        }
                        println!("{line}");
                    }
                }
                if let Some((f, _)) = &before {
                    let name = f.file_name().map_or(String::new(), |n| n.to_string_lossy().to_string());
                    if changes.is_empty() {
                        println!("no changes since {name}");
                    } else {
                        println!("changes since {name}:");
                        for c in &changes {
                            println!(
                                "  {} {}: {} -> {} (worst {} -> {}){}",
                                c.test,
                                c.spec,
                                c.before.map_or("new", |s| s.label()),
                                c.after.label(),
                                verify::fmt(c.worst_before),
                                verify::fmt(c.worst_after),
                                if c.is_regression() { "  REGRESSION" } else { "" }
                            );
                        }
                    }
                }
                let n = run.counts();
                println!("{}: {} pass, {} fail, {} error, {} info", run.status().label(), n.pass, n.fail, n.error, n.info);
            }
            if let Some(f) = &junit {
                std::fs::write(f, run.junit()).map_err(|e| format!("{}: {e}", f.display()))?;
                eprintln!("wrote {}", f.display());
            }
            if !no_history && let Some(r) = &root {
                let f = verify::save_history(r, &run)?;
                eprintln!("saved {}", f.display());
            }
            if matches!(run.status(), Status::Fail | Status::Error) {
                std::process::exit(1);
            }
        }
        Cmd::Yield { project, cell, runs, samples, target, process, json } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell_mut(&r).unwrap();
            if let Some(n) = runs {
                c.sim.sweep.monte_carlo.runs = n;
            }
            if process {
                c.sim.sweep.monte_carlo.process = true;
            }
            let mc = &c.sim.sweep.monte_carlo;
            if mc.runs == 0 || (mc.vars.iter().all(|v| v.target.trim().is_empty()) && !mc.process) {
                return Err(format!("{r}: the sweep setup has no Monte Carlo (varied parameters and runs)"));
            }
            let specs = c.sim.sweep.specs.clone();
            if specs.is_empty() {
                return Err(format!("{r}: the sweep setup has no specs"));
            }
            let mut opts = SimOptions::default();
            if let Some(exe) = &p.trusted_ngspice() {
                opts.ngspice_exe = exe.clone();
            }
            let started = std::time::Instant::now();
            let progress = |done: usize, total: usize| {
                if !json && (done.is_multiple_of(50) || done == total) {
                    eprintln!("{done}/{total} point(s)");
                }
            };
            let run = ics_sim::sweep::run(&p, &r, &opts, &progress)?;
            for n in &run.notes {
                eprintln!("{n}");
            }
            let conds = run.predict(&specs, samples, 1);
            let plan = run.plan.as_ref().unwrap();
            let pct = |v: f64| format!("{:.2}%", v * 100.0);
            let below = conds.iter().any(|c| target.is_some_and(|t| c.prediction.joint.is_none_or(|y| y < t)));
            if json {
                let cj: Vec<serde_json::Value> = conds
                    .iter()
                    .map(|c| {
                        let (lo, hi) = ics_core::predict::wilson(c.passed, c.runs);
                        let sj: Vec<serde_json::Value> = specs
                            .iter()
                            .zip(&c.prediction.specs)
                            .map(|(s, sp)| match sp {
                                Ok(sp) => serde_json::json!({"name": s.name, "mean": sp.mean, "sigma": sp.std, "yield": sp.yield_,
                                    "cpk": sp.cpk, "q2": sp.model.q2, "rmse": sp.model.rmse, "quadratic": sp.model.quadratic,
                                    "sensitivity": plan.inputs.iter().zip(&sp.sensitivity).enumerate()
                                        .map(|(i, (n, v))| serde_json::json!({"input": n, "share": v, "slope": sp.model.slope(i)}))
                                        .collect::<Vec<_>>()}),
                                Err(e) => serde_json::json!({"name": s.name, "error": e}),
                            })
                            .collect();
                        serde_json::json!({"condition": c.condition, "runs": c.runs, "passed": c.passed,
                            "simulated_yield": c.passed as f64 / c.runs.max(1) as f64, "simulated_ci95": [lo, hi],
                            "predicted_yield": c.prediction.joint, "failed_runs": c.prediction.failed_runs, "specs": sj})
                    })
                    .collect();
                println!("{}", serde_json::json!({"cell": r.to_string(), "samples": samples, "conditions": cj}));
            } else {
                println!(
                    "{r}: {} point(s) in {:.1} s, {samples} surrogate samples per condition",
                    run.points.len(),
                    started.elapsed().as_secs_f64()
                );
                for c in &conds {
                    let (lo, hi) = ics_core::predict::wilson(c.passed, c.runs);
                    println!(
                        "\n{}: simulated {}/{} ({}, 95% CI {} to {}), predicted {}",
                        c.condition,
                        c.passed,
                        c.runs,
                        pct(c.passed as f64 / c.runs.max(1) as f64),
                        pct(lo),
                        pct(hi),
                        c.prediction.joint.map_or("—".into(), pct)
                    );
                    if c.prediction.failed_runs > 0 {
                        println!("  {} run(s) gave no value for a spec: counted as failures", c.prediction.failed_runs);
                    }
                    for (s, sp) in specs.iter().zip(&c.prediction.specs) {
                        match sp {
                            Ok(sp) => {
                                let mut top: Vec<(usize, f64)> = sp.sensitivity.iter().copied().enumerate().collect();
                                top.sort_by(|a, b| b.1.total_cmp(&a.1));
                                let top: Vec<String> = top
                                    .iter()
                                    .take(3)
                                    .filter(|t| t.1 >= 0.01)
                                    .map(|(i, v)| format!("{} {:.0}%", plan.inputs[*i], v * 100.0))
                                    .collect();
                                println!(
                                    "  {:<12} mean {:>10}  sigma {:>10}  yield {:>8}  cpk {:>6}  Q2 {:.3}  [{}]",
                                    s.name,
                                    format_eng(sp.mean, ""),
                                    format_eng(sp.std, ""),
                                    sp.yield_.map_or("—".into(), pct),
                                    sp.cpk.map_or("—".into(), |v| format!("{v:.2}")),
                                    sp.model.q2,
                                    top.join(", ")
                                );
                                if sp.model.q2 < 0.9 {
                                    println!(
                                        "  {:<12} the model explains little of the spread (Q2 < 0.9): run more samples or check the spec",
                                        ""
                                    );
                                }
                            }
                            Err(e) => println!("  {:<12} no model: {e}", s.name),
                        }
                    }
                }
            }
            if below {
                return Err(format!("a condition's predicted yield is below {}", pct(target.unwrap())));
            }
        }
        Cmd::Correlate {
            project,
            cell,
            data,
            x,
            analysis,
            traces,
            log,
            params,
            fit,
            iterations,
            write_models,
            apply,
            save,
            json,
        } => {
            use ics_core::silicon::{FitParam, Trace};
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell_mut(&r).unwrap();
            let si = &mut c.sim.silicon;
            if let Some(d) = &data {
                let abs = std::fs::canonicalize(d).map_err(|e| format!("{}: {e}", d.display()))?;
                let root = std::fs::canonicalize(&project).unwrap_or(project.clone());
                si.data = abs.strip_prefix(&root).map_or(abs.clone(), |r| r.to_path_buf()).to_string_lossy().into_owned();
            }
            if let Some(x) = x {
                si.x = x;
            }
            if let Some(a) = analysis {
                si.analysis = a;
            }
            if !traces.is_empty() {
                si.traces = traces
                    .iter()
                    .map(|t| {
                        let (col, rest) = t.split_once('=').ok_or(format!("--trace '{t}': write COLUMN=EXPR[@BIAS]"))?;
                        let (expr, bias) = rest.split_once('@').unwrap_or((rest, ""));
                        Ok(Trace { column: col.trim().into(), expr: expr.trim().into(), bias: bias.trim().into(), log })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
            } else if log {
                si.traces.iter_mut().for_each(|t| t.log = true);
            }
            if !params.is_empty() {
                si.params = params
                    .iter()
                    .map(|q| {
                        let (target, range) = q.rsplit_once('=').ok_or(format!("--param '{q}': write TARGET=MIN:MAX[:log]"))?;
                        let parts: Vec<&str> = range.split(':').collect();
                        match parts.as_slice() {
                            [lo, hi] | [lo, hi, "log"] => Ok(FitParam {
                                target: target.trim().into(),
                                min: lo.trim().into(),
                                max: hi.trim().into(),
                                log: parts.len() == 3,
                            }),
                            _ => Err(format!("--param '{q}': write TARGET=MIN:MAX[:log]")),
                        }
                    })
                    .collect::<Result<Vec<_>, String>>()?;
            }
            if save || apply {
                p.save().map_err(|e| e.to_string())?;
            }
            let mut opts = SimOptions::default();
            if let Some(exe) = &p.trusted_ngspice() {
                opts.ngspice_exe = exe.clone();
            }
            let cor = ics_sim::silicon::Correlation::new(&p, &r, &opts)?;
            let start_values = cor.start_values();
            let (start, best, iters, std_errors) = if fit {
                let rep = cor.fit(iterations, &mut |k, e| {
                    if !json {
                        eprintln!("iteration {k}: error {:.3}%", e.error() * 100.0);
                    }
                    true
                })?;
                (rep.start, Some(rep.best), rep.iterations, rep.std_errors)
            } else {
                (cor.evaluate(&start_values), None, 0, vec![])
            };
            let fitted = best.as_ref().map(|b| b.values.clone());
            let cards = match &fitted {
                Some(v) => cor.fitted_cards(v)?,
                None => String::new(),
            };
            let pct = |v: f64| format!("{:.3}%", v * 100.0);
            if json {
                let tr = |e: &ics_sim::silicon::Evaluation| -> Vec<serde_json::Value> {
                    e.traces
                        .iter()
                        .zip(&cor.setup.traces)
                        .map(|(t, s)| match t {
                            Ok(t) => serde_json::json!({"column": t.column, "error": t.error}),
                            Err(e) => serde_json::json!({"column": s.column, "error_message": e}),
                        })
                        .collect()
                };
                let ps: Vec<serde_json::Value> = cor
                    .params()
                    .iter()
                    .enumerate()
                    .map(|(i, q)| {
                        serde_json::json!({"target": q.target, "start": start_values[i], "fitted": fitted.as_ref().map(|v| v[i]),
                        "std_error": std_errors.get(i).filter(|e| e.is_finite())})
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({"cell": r.to_string(), "error_start": start.error(), "traces_start": tr(&start),
                        "error_fitted": best.as_ref().map(|b| b.error()), "traces_fitted": best.as_ref().map(tr),
                        "iterations": iters, "params": ps, "model_cards": cards})
                );
            } else {
                println!("{r}: {} trace(s) against {}", cor.setup.traces.len(), cor.setup.data);
                for (i, t) in start.traces.iter().enumerate() {
                    let after = best.as_ref().map(|b| match &b.traces[i] {
                        Ok(t) => pct(t.error),
                        Err(e) => e.clone(),
                    });
                    match t {
                        Ok(t) => println!(
                            "  {:<16} error {:>9}{}",
                            t.column,
                            pct(t.error),
                            after.map_or(String::new(), |a| format!(" -> {a}"))
                        ),
                        Err(e) => println!("  {:<16} {e}", cor.setup.traces[i].column),
                    }
                }
                match &best {
                    Some(b) => {
                        println!("RMS error {} -> {} in {iters} iteration(s)", pct(start.error()), pct(b.error()));
                        for (i, q) in cor.params().iter().enumerate() {
                            let se = std_errors
                                .get(i)
                                .filter(|e| e.is_finite())
                                .map_or(String::new(), |e| format!(" ± {}", format_eng(*e, "")));
                            println!(
                                "  {:<24} {:>10} -> {}{se}",
                                q.target,
                                format_eng(start_values[i], ""),
                                format_eng(b.values[i], "")
                            );
                        }
                        if !cards.is_empty() {
                            println!("{}", cards.trim_end());
                        }
                    }
                    None => println!("RMS error {}", pct(start.error())),
                }
            }
            if let (Some(path), false) = (&write_models, cards.is_empty()) {
                let head = format!("* Model cards fitted to {} ({})\n", cor.setup.data, r);
                std::fs::write(path, head + &cards).map_err(|e| format!("{}: {e}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            if apply && let Some(v) = &fitted {
                let targets: Vec<(String, f64)> = cor
                    .params()
                    .iter()
                    .zip(v)
                    .filter(|(q, _)| q.model_param().is_none())
                    .map(|(q, x)| (q.target.clone(), *x))
                    .collect();
                drop(cor);
                let sch = p.cell_mut(&r).unwrap().schematic.as_mut().ok_or("cell has no schematic")?;
                for (t, x) in targets {
                    if let Some((inst, param)) = t.split_once('.')
                        && let Some(i) = sch.instances.iter_mut().find(|i| i.name.eq_ignore_ascii_case(inst))
                    {
                        i.params.insert(param.to_ascii_lowercase(), ics_core::optimize::spice_number(x));
                    }
                }
                p.save().map_err(|e| e.to_string())?;
            }
        }
        Cmd::Xsection { project, cell, flow, cut, steps, at, svg, profile, pinned_pd, apply, json } => {
            use ics_core::process::{Cut, GENERIC180, ProcessFlow, default_cut, emulate_cell};
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let text = match &flow {
                Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
                None => GENERIC180.to_string(),
            };
            let fl = ProcessFlow::from_toml(&text).map_err(|e| format!("process flow: {e}"))?;
            let cut = match &cut {
                Some(c) => Cut::parse(c)?,
                None => default_cut(&p, &r)?,
            };
            let started = std::time::Instant::now();
            let s = emulate_cell(&p, &r, &fl, &cut, steps)?;
            let len = cut.length();
            let profile_at = at.first().copied().unwrap_or(len / 2.0);
            let at = if at.is_empty() { (1..=5).map(|k| len * k as f64 / 6.0).collect() } else { at };
            if json {
                let st: Vec<serde_json::Value> = at
                    .iter()
                    .map(|x| {
                        serde_json::json!({"x": x, "surface": s.surface_at(*x),
                            "stack": s.stack_at(*x).iter().map(|(m, t)| serde_json::json!({"material": m, "thickness": t})).collect::<Vec<_>>(),
                            "doping": s.doped_layers(*x).iter().map(|l| serde_json::json!({"material": l.material, "type": if l.n_type { "n" } else { "p" },
                                "top": l.top, "bottom": l.bottom, "peak": l.peak, "sheet_resistance": l.sheet_resistance.is_finite().then_some(l.sheet_resistance)})).collect::<Vec<_>>(),
                            "junctions": s.junctions(*x)})
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({"cell": r.to_string(), "flow": fl.name, "cut": [cut.x0, cut.y0, cut.x1, cut.y1], "length": len,
                        "steps": steps.unwrap_or(fl.steps.len()).min(fl.steps.len()), "materials": s.materials, "columns": st})
                );
            } else {
                let n = steps.unwrap_or(fl.steps.len()).min(fl.steps.len());
                println!(
                    "{r}: {} along ({:.3}, {:.3}) - ({:.3}, {:.3}), {len:.3} µm, {n} of {} steps, {} x {} cells in {:.2} s",
                    fl.name,
                    cut.x0,
                    cut.y0,
                    cut.x1,
                    cut.y1,
                    fl.steps.len(),
                    s.nx,
                    s.nz,
                    started.elapsed().as_secs_f64()
                );
                if let Some(last) = fl.steps[..n].last() {
                    println!("after: {}", last.label());
                }
                for x in &at {
                    let stack: Vec<String> = s.stack_at(*x).iter().map(|(m, t)| format!("{m} {t:.3}")).collect();
                    println!("  x = {x:>7.3}  surface {:>6.3}  {}", s.surface_at(*x), stack.join(" | "));
                    let layers: Vec<String> = s
                        .doped_layers(*x)
                        .iter()
                        .map(|l| {
                            format!(
                                "{} {} {:.3}..{:.3} peak {:.1e} {} Ω/sq",
                                if l.n_type { "n" } else { "p" },
                                l.material,
                                l.top,
                                l.bottom,
                                l.peak,
                                format_eng(l.sheet_resistance, "")
                            )
                        })
                        .collect();
                    if !layers.is_empty() {
                        println!("                       doping  {}", layers.join(" | "));
                    }
                }
            }
            if let Some(path) = &profile {
                let mut t = format!("# dopant profile at x = {profile_at} µm (cm^-3; net > 0 is n-type)\nz_um,net");
                for (sp, _) in &s.dopants {
                    t += &format!(",{}", sp.name());
                }
                t.push('\n');
                for (z, net, cs) in s.profile(profile_at) {
                    t += &format!("{z:.4},{net:.4e}");
                    for c in cs {
                        t += &format!(",{c:.4e}");
                    }
                    t.push('\n');
                }
                std::fs::write(path, t).map_err(|e| format!("{}: {e}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            if let Some(path) = &svg {
                let px_per_um = (1200.0 / len).clamp(50.0, 2000.0);
                std::fs::write(path, s.svg(&fl, px_per_um, &format!("{r} {}", fl.name)))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            if let Some(x) = pinned_pd {
                let pd = ics_core::pinned_pd::from_section(&s, x)?;
                println!(
                    "pinned photodiode at x = {x}: n region {:.3}..{:.3} µm ({:.3e} cm^-2), potential peak at {:.3} µm, \
                     depleted {:.3}..{:.3} µm\n  pinning {:.3} V, storage capacitance {:.4} fF/µm^2  ->  {}",
                    pd.n_top,
                    pd.n_bottom,
                    pd.n_dose,
                    pd.peak,
                    pd.depletion.0,
                    pd.depletion.1,
                    pd.vpin,
                    pd.cap_area,
                    pd.toml()
                );
                if apply {
                    let def = p
                        .tech
                        .cross_section
                        .as_mut()
                        .and_then(|c| c.photodiode.as_mut())
                        .ok_or("the technology has no cross-section photodiode to set")?;
                    def.pinning = Some((pd.vpin * 1000.0).round() / 1000.0);
                    def.cap_area = Some((pd.cap_area * 1e4).round() / 1e4);
                    def.cap_perimeter = None;
                    p.save().map_err(|e| e.to_string())?;
                    println!("  written to the technology's photodiode");
                }
            }
        }
        Cmd::ProcessData { data, spec, json } => {
            use ics_core::process_data::{GENERIC180_SPEC, ProcessSpec, WatData, correlation, summarize};
            let text = std::fs::read_to_string(&data).map_err(|e| format!("{}: {e}", data.display()))?;
            let d = WatData::parse(&text).map_err(|e| format!("{}: {e}", data.display()))?;
            let st = match &spec {
                Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
                None => GENERIC180_SPEC.to_string(),
            };
            let sp = ProcessSpec::from_toml(&st).map_err(|e| format!("spec: {e}"))?;
            let sum = summarize(&d, &sp);
            let wafers = d.wafers();
            // The strongest correlations between parameters.
            let mut cors = vec![];
            for a in 0..d.params.len() {
                for b in a + 1..d.params.len() {
                    let r = correlation(&d, a, b);
                    if r.abs() >= 0.7 {
                        cors.push((d.params[a].clone(), d.params[b].clone(), r));
                    }
                }
            }
            cors.sort_by(|x, y| y.2.abs().total_cmp(&x.2.abs()));
            let bad = sum.iter().any(|s| s.out_of_spec > 0 || !s.violations.is_empty());
            if json {
                let ps: Vec<serde_json::Value> = sum
                    .iter()
                    .map(|s| {
                        serde_json::json!({"name": s.name, "unit": sp.param(&s.name).map(|p| p.unit.clone()), "n": s.n, "mean": s.mean,
                            "sigma": s.std, "min": s.min, "max": s.max, "cp": s.cp, "cpk": s.cpk, "out_of_spec": s.out_of_spec,
                            "control": {"center": s.center, "ucl": s.ucl, "lcl": s.lcl},
                            "violations": s.violations.iter().map(|v| serde_json::json!({"rule": v.rule,
                                "lot": wafers[v.wafer].0, "wafer": wafers[v.wafer].1, "message": v.message})).collect::<Vec<_>>(),
                            "corners": s.corners.iter().map(|c| serde_json::json!({"corner": c.0, "value": c.1, "z": c.2})).collect::<Vec<_>>(),
                            "corners_cover_3sigma": s.corners_cover})
                    })
                    .collect();
                let cj: Vec<serde_json::Value> = cors.iter().map(|c| serde_json::json!({"a": c.0, "b": c.1, "r": c.2})).collect();
                println!(
                    "{}",
                    serde_json::json!({"spec": sp.name, "sites": d.sites.len(), "wafers": wafers.len(), "params": ps, "correlations": cj})
                );
            } else {
                println!("{}: {} sites on {} wafers against {}", data.display(), d.sites.len(), wafers.len(), sp.name);
                println!(
                    "  {:<10} {:>6} {:>11} {:>10} {:>6} {:>6} {:>5}  corners",
                    "param", "unit", "mean", "sigma", "Cp", "Cpk", "OOS"
                );
                let f2 = |v: Option<f64>| v.map_or("—".to_string(), |x| format!("{x:.2}"));
                for s in &sum {
                    let unit = sp.param(&s.name).map_or(String::new(), |p| p.unit.clone());
                    let corners = match s.corners_cover {
                        Some(true) => "cover ±3σ".to_string(),
                        Some(false) => {
                            let z: Vec<String> = s.corners.iter().map(|c| format!("{} {:+.1}σ", c.0, c.2)).collect();
                            format!("NARROWER than ±3σ ({})", z.join(", "))
                        }
                        None => String::new(),
                    };
                    println!(
                        "  {:<10} {:>6} {:>11} {:>10} {:>6} {:>6} {:>5}  {corners}",
                        s.name,
                        unit,
                        format_eng(s.mean, ""),
                        format_eng(s.std, ""),
                        f2(s.cp),
                        f2(s.cpk),
                        s.out_of_spec
                    );
                }
                for s in &sum {
                    let Some(first) = s.violations.first() else { continue };
                    let (lot, wafer, _) = &wafers[first.wafer];
                    let mut rules: Vec<u8> = s.violations.iter().map(|v| v.rule).collect();
                    rules.sort_unstable();
                    rules.dedup();
                    let n_wafers = {
                        let mut w: Vec<usize> = s.violations.iter().map(|v| v.wafer).collect();
                        w.dedup();
                        w.len()
                    };
                    println!(
                        "  SPC {}: out of control from lot {lot} wafer {wafer} ({}), {n_wafers} wafer(s) flagged, rules {}",
                        s.name,
                        first.message,
                        rules.iter().map(u8::to_string).collect::<Vec<_>>().join(", ")
                    );
                }
                for (a, b, r) in cors.iter().take(6) {
                    println!("  correlated: {a} / {b}  r = {r:+.2}");
                }
            }
            if bad {
                return Err("out-of-spec sites or control-rule violations".into());
            }
        }
        Cmd::Mismatch { data, project, apply, json } => {
            use ics_core::mismatch::{GENERIC180_PAIRS, MismatchModel, PairData, fit};
            let (text, name) = match &data {
                Some(f) => (std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?, f.display().to_string()),
                None => (GENERIC180_PAIRS.to_string(), "generic180 pairs (built in)".to_string()),
            };
            let d = PairData::parse(&text).map_err(|e| format!("{name}: {e}"))?;
            let fits = fit(&d)?;
            if json {
                let fj: Vec<serde_json::Value> = fits
                    .iter()
                    .map(|f| {
                        let pel = |p: &ics_core::mismatch::Pelgrom| {
                            serde_json::json!({"a": p.a, "a_err": p.a_err, "floor": p.floor,
                                "sizes": p.sizes.iter().map(|s| serde_json::json!({"w": s.w, "l": s.l, "n": s.n, "mean": s.mean, "sigma": s.sigma, "fitted": s.fitted})).collect::<Vec<_>>()})
                        };
                        serde_json::json!({"device": f.device, "pmos": f.pmos, "vt": pel(&f.vt), "beta": f.beta.as_ref().map(pel)})
                    })
                    .collect();
                println!("{}", serde_json::json!({"data": name, "pairs": d.rows.len(), "devices": fj}));
            } else {
                println!("{name}: {} pairs", d.rows.len());
                for f in &fits {
                    print!("  {:<6} Avt {:.3} ± {:.3} mV·µm", f.device, f.vt.a * 1e3, f.vt.a_err * 1e3);
                    if f.vt.floor > 0.0 {
                        print!(" (floor {:.3} mV)", f.vt.floor * 1e3);
                    }
                    if let Some(b) = &f.beta {
                        print!("   Abeta {:.3} ± {:.3} %·µm", b.a * 100.0, b.a_err * 100.0);
                    }
                    println!();
                    for s in &f.vt.sizes {
                        println!(
                            "         W/L {:>5}/{:<5} n {:>3}  σ(ΔVt) {:>6.3} mV (fit {:>6.3})  mean {:>+7.3} mV",
                            s.w,
                            s.l,
                            s.n,
                            s.sigma * 1e3,
                            s.fitted * 1e3,
                            s.mean * 1e3
                        );
                    }
                }
            }
            if apply {
                let dir = project.as_ref().ok_or("--apply needs --project")?;
                let mut p = Project::open(dir).map_err(|e| e.to_string())?;
                p.tech.mismatch = Some(MismatchModel::from_fits(&fits, &name));
                p.save().map_err(|e| e.to_string())?;
                eprintln!("stored the mismatch in the technology of {}", p.name);
            }
        }
        Cmd::Corners { data, spec, project, lib, variation, apply, iterations, json } => {
            if apply && project.is_none() {
                return Err("--apply needs --project".into());
            }
            use ics_core::process_data::{GENERIC180_SPEC, ProcessSpec, WatData};
            let text = std::fs::read_to_string(&data).map_err(|e| format!("{}: {e}", data.display()))?;
            let d = WatData::parse(&text).map_err(|e| format!("{}: {e}", data.display()))?;
            let st = match &spec {
                Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
                None => GENERIC180_SPEC.to_string(),
            };
            let sp = ProcessSpec::from_toml(&st).map_err(|e| format!("spec: {e}"))?;
            let mut p = match &project {
                Some(dir) => Project::open(dir).map_err(|e| e.to_string())?,
                None => ics_core::examples::demo_project(),
            };
            let mut opts = SimOptions::default();
            if let Some(exe) = &p.trusted_ngspice() {
                opts.ngspice_exe = exe.clone();
            }
            let started = std::time::Instant::now();
            let ex = ics_sim::corners::Extraction::new(&p, &sp, &d, &opts)?;
            let fits = ex.run(iterations, &mut |f| {
                if !json {
                    eprintln!("{}: error {:.3}σ", f.name, f.error_sigma);
                }
            });
            let source = format!("{} WAT, {}", sp.name, data.display());
            let var = fits.first().map(|tt| ex.variation(tt, &source)).transpose();
            let var = match var {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("warning: no process variation: {e}");
                    None
                }
            };
            let library =
                ex.library(&fits, var.as_ref().map(|v| &v.variation), &format!("{} corners from {}", sp.name, data.display()))?;
            let var_json = var.as_ref().map(|v| {
                let pv = &v.variation;
                serde_json::json!({
                    "params": (0..pv.params.len()).map(|i| serde_json::json!({"name": pv.params[i], "mean": pv.mean[i], "sigma": pv.sigma(i)})).collect::<Vec<_>>(),
                    "correlation": (0..pv.params.len()).map(|i| (0..pv.params.len()).map(|j| pv.correlation(i, j)).collect::<Vec<_>>()).collect::<Vec<_>>(),
                    "reproduced": v.params.iter().map(|(n, d, g)| serde_json::json!({"name": n, "data_sigma": d, "sigma": g})).collect::<Vec<_>>(),
                    "variation": pv,
                })
            });
            if json {
                let cj: Vec<serde_json::Value> = fits
                    .iter()
                    .map(|f| {
                        serde_json::json!({"corner": f.name, "error_sigma": f.error_sigma,
                            "knobs": ex.knobs.iter().zip(&f.values).zip(&f.std_errors).map(|((k, v), e)| serde_json::json!({"target": k.target, "value": v,
                                "std_error": e.is_finite().then_some(*e)})).collect::<Vec<_>>(),
                            "params": f.params.iter().map(|(n, t, g)| serde_json::json!({"name": n, "target": t, "simulated": g.as_ref().ok(),
                                "error": g.as_ref().err()})).collect::<Vec<_>>()})
                    })
                    .collect();
                println!("{}", serde_json::json!({"spec": sp.name, "corners": cj, "variation": var_json, "library": library}));
            } else {
                println!(
                    "{}: {} sites, {} corner(s) in {:.1} s",
                    data.display(),
                    d.sites.len(),
                    fits.len(),
                    started.elapsed().as_secs_f64()
                );
                let mut head = format!("  {:<6}", "corner");
                for k in &ex.knobs {
                    head += &format!(" {:>18}", k.target.trim_start_matches("model:"));
                }
                println!("{head}   error");
                for f in &fits {
                    let mut line = format!("  {:<6}", f.name);
                    for (v, e) in f.values.iter().zip(&f.std_errors) {
                        let t = if e.is_finite() {
                            format!("{} ±{}", format_eng(*v, ""), format_eng(*e, ""))
                        } else {
                            format_eng(*v, "")
                        };
                        line += &format!(" {t:>18}");
                    }
                    println!("{line}   {:.3}σ", f.error_sigma);
                }
                let mut head = format!("  {:<6}", "");
                if let Some(f) = fits.first() {
                    for (n, _, _) in &f.params {
                        head += &format!(" {n:>20}");
                    }
                }
                println!("{head}");
                for f in &fits {
                    let mut line = format!("  {:<6}", f.name);
                    for (_, t, g) in &f.params {
                        let got = g.as_ref().map_or("fail".to_string(), |v| format_eng(*v, ""));
                        line += &format!(" {:>20}", format!("{got} ({})", format_eng(*t, "")));
                    }
                    println!("{line}");
                }
                println!("  (simulated, with the target in brackets)");
                if let Some(v) = &var {
                    let pv = &v.variation;
                    println!("Process variation (global, wafer to wafer):");
                    for i in 0..pv.params.len() {
                        let corr: Vec<String> = (0..pv.params.len()).map(|j| format!("{:+.2}", pv.correlation(i, j))).collect();
                        println!(
                            "  {:<10} {:>10} σ {:>10} ({:>5.1} %)   r [{}]",
                            pv.params[i],
                            format_eng(pv.mean[i], ""),
                            format_eng(pv.sigma(i), ""),
                            100.0 * pv.sigma(i) / pv.mean[i].abs().max(1e-300),
                            corr.join(" ")
                        );
                    }
                    let fit: Vec<String> = v
                        .params
                        .iter()
                        .map(|(n, d, g)| format!("{n} σ {} (data {})", format_eng(*g, ""), format_eng(*d, "")))
                        .collect();
                    println!("  reproduces {}", fit.join(", "));
                }
            }
            if let Some(path) = &lib {
                std::fs::write(path, &library).map_err(|e| format!("{}: {e}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            if let Some(path) = &variation {
                let v = var.as_ref().ok_or("no process variation to write")?;
                let t = serde_json::to_string_pretty(&v.variation).map_err(|e| e.to_string())?;
                std::fs::write(path, t).map_err(|e| format!("{}: {e}", path.display()))?;
                eprintln!("wrote {}", path.display());
            }
            if apply {
                p.tech.variation = Some(var.ok_or("no process variation to apply")?.variation);
                p.save().map_err(|e| e.to_string())?;
                eprintln!("stored the process variation in the technology of {}", p.name);
            }
        }
        Cmd::Optimize { project, cell, post_layout, apply, json } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            if post_layout {
                p.cell_mut(&r).unwrap().sim.optimize.post_layout = true;
            }
            let mut opts = SimOptions::default();
            if let Some(exe) = &p.trusted_ngspice() {
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
                let sch = p.circuit(&r)?.into_owned();
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
                let sch = p.circuit(&target)?.into_owned();
                let mut lay = p.cell_mut(&target).unwrap().layout.take().unwrap_or_default();
                let res = ics_core::route::route(&sch, &mut lay, &p, &p.tech);
                p.cell_mut(&target).unwrap().layout = Some(lay);
                println!("{}", res?.summary());
            }
            p.save().map_err(|e| e.to_string())?;
        }
        Cmd::Extract {
            project,
            cell,
            pixel,
            characterize,
            backend: b,
            schematic,
            into,
            overwrite,
            no_testbench,
            vdd,
            iph,
            temp,
            ccol,
            cds_dt,
        } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let lay = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let x = ics_core::extract::extract(lay, &p, &p.tech);
            for w in &x.warnings {
                eprintln!("warning: {w}");
            }
            for sc in &x.soft_conflicts {
                let names: Vec<&str> = sc.nets.iter().map(|&n| x.nets[n].name.as_str()).collect();
                let what = if sc.well.is_some() { "a well" } else { "the substrate" };
                eprintln!("warning: soft connection: {what} is tapped from nets {}; taken as {}", names.join(", "), names[0]);
            }
            print!("{}", x.spice(&r.to_string(), p.tech.dbu_per_um));
            println!(
                "* {} nets, {} transistors, {} resistors/capacitors, {} bipolar transistors",
                x.nets.len(),
                x.devices.len(),
                x.passives.len(),
                x.bjts.len()
            );
            if pixel || schematic {
                let pc = ics_core::pixel_circuit::classify(&x);
                for w in &pc.warnings {
                    eprintln!("warning: {w}");
                }
                let tb = ics_core::pixel_circuit::Testbench {
                    vdd,
                    iph,
                    temp,
                    ccol,
                    cds_dt,
                    ..ics_core::pixel_circuit::Testbench::for_tech(&x, &p.tech)
                };
                if pixel {
                    print!("{}", pc.report(&x, &tb.wire_caps));
                }
                if characterize {
                    let bench = tb.clone();
                    let opts = SimOptions { backend: backend(b), deck_dir: Some(project.clone()), ..Default::default() };
                    let corner = p.cell(&r).map(|c| c.sim.corner.clone()).unwrap_or_default();
                    for k in 0..pc.pixels.len().min(16) {
                        match ics_sim::pixel::characterize(&x, &pc, k, &p.tech, &bench, &corner, &opts) {
                            Ok(m) => print!("pixel {} {}", k + 1, m.summary().replace('\n', "\n  ").trim_end_matches(' ')),
                            Err(e) => eprintln!("warning: pixel {}: {e}", k + 1),
                        }
                    }
                }
                if schematic {
                    let bench = tb;
                    let (sch, sim) = ics_core::pixel_circuit::schematic(&x, &pc, (!no_testbench).then_some(&bench));
                    let target = match into.as_deref() {
                        None => r.clone(),
                        Some(name) => match resolve(&p, name) {
                            Ok(t) => t,
                            Err(_) => {
                                let (lib, n) = name.split_once('/').unwrap_or((r.lib.as_str(), name));
                                if p.library_mut(lib).is_none() {
                                    p.add_library(lib)?;
                                }
                                p.add_cell(lib, ics_core::Cell::new(n))?
                            }
                        },
                    };
                    let c = p.cell_mut(&target).unwrap();
                    if c.schematic.is_some() && !overwrite {
                        return Err(format!(
                            "{target} already has a schematic; pass --overwrite to replace it (or --into another cell)"
                        ));
                    }
                    c.schematic = Some(sch);
                    if !no_testbench {
                        c.sim = sim;
                    }
                    p.save().map_err(|e| e.to_string())?;
                    println!("* schematic written to {target}");
                }
            }
        }
        Cmd::Lvs { project, cell } => {
            let p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let c = p.cell(&r).unwrap();
            let lay = c.layout.as_ref().ok_or("cell has no layout")?;
            let circuit = p.circuit(&r)?;
            let sch = &*circuit;
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
            for w in &rep.warnings {
                println!("warning: {}", w.message);
            }
            if !rep.matched {
                return Err(format!("LVS: {} difference(s)", rep.errors.len()));
            }
            println!("LVS: layout matches schematic");
        }
        Cmd::Opc { project, cell, layer, apply, sigma, threshold, fragment, max_out, max_in, iterations, gain, json } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let r = resolve(&p, &cell)?;
            let l = p.tech.layer_index(&layer).ok_or_else(|| format!("unknown layer '{layer}'"))?;
            let o = ics_mcp::tools::OpcOverrides { sigma, threshold, fragment, max_out, max_in, iterations, gain };
            let params = ics_mcp::tools::opc_params(&p.tech, l, &o);
            let top = p.cell(&r).unwrap().layout.as_ref().ok_or("cell has no layout")?;
            let rep = ics_core::opc::correct(top, &p, &p.tech, &params)?;
            if json {
                println!("{}", ics_mcp::tools::opc_json(&rep, &params, &p.tech, 50));
            } else {
                println!("{} {layer}: {}", r, ics_core::opc::summary(&rep, p.tech.dbu_per_um));
            }
            if apply {
                let (out, added) = ics_core::opc::mask_layer(&mut p.tech, l)?;
                let lay = p.cell_mut(&r).and_then(|c| c.layout.as_mut()).ok_or("cell has no layout")?;
                let (removed, n) = ics_core::opc::write_mask(lay, &rep.mask, out);
                p.save().map_err(|e| e.to_string())?;
                if !json {
                    let name = &p.tech.layers[out].name;
                    let what = if added { "new layer" } else { "layer" };
                    println!("wrote {n} mask shape(s) to {what} {name} (replaced {removed}) and saved the project");
                }
            }
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
        Cmd::ImportGds { project, file, lib, skip_unknown_layers, json, layermap } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            if let Some(m) = &layermap {
                let text = std::fs::read_to_string(m).map_err(|e| format!("{}: {e}", m.display()))?;
                let r = ics_core::stream_map::apply_layer_map(&mut p.tech, &text);
                println!("layer map {}: {} row(s)", m.display(), r.rows);
                if !r.added_layers.is_empty() {
                    println!("added {} layer(s) from the layer map: {}", r.added_layers.len(), r.added_layers.join(", "));
                }
                for w in &r.warnings {
                    eprintln!("warning: {}: {w}", m.display());
                }
            }
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
        Cmd::ImportTech { project, file, lyt, drf, layermap } => {
            let mut p = Project::open(&project).map_err(|e| e.to_string())?;
            let read = |f: &PathBuf| std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()));
            let name = file.file_stem().map_or("tech".into(), |s| s.to_string_lossy().to_string());
            let virtuoso = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("tf"));
            let tech = if virtuoso {
                let drf = drf.as_ref().map(read).transpose()?;
                let map = layermap.as_ref().map(read).transpose()?;
                let (tech, notes) = ics_core::virtuoso::from_techfile(&read(&file)?, drf.as_deref(), map.as_deref(), &name)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                for n in &notes {
                    eprintln!("note: {n}");
                }
                tech
            } else {
                let mut tech =
                    ics_core::tech_import::from_lyp(&read(&file)?, &name).map_err(|e| format!("{}: {e}", file.display()))?;
                if let Some(f) = &lyt {
                    ics_core::tech_import::apply_lyt(&mut tech, &read(f)?).map_err(|e| format!("{}: {e}", f.display()))?;
                }
                tech
            };
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

fn ext_root() -> Result<PathBuf, String> {
    ics_mcp::ext::user_dir().ok_or_else(|| "no per-user data folder; set ICS_EXTENSIONS_DIR".to_string())
}

/// Run `tool` on the project in `dir` and print its result; save when it changed the project.
fn ext_call(
    dir: &std::path::Path,
    tool: &str,
    args: &serde_json::Map<String, serde_json::Value>,
    no_save: bool,
) -> Result<(), String> {
    use ics_mcp::ext::Registry;
    let reg = Registry::load();
    let p = Project::open(dir).map_err(|e| e.to_string())?;
    let mut state = ics_mcp::tools::State::with_project(p);
    let res = if reg.has_tool(tool) {
        let r = reg.call(tool, args, &mut |t: &str, a: &serde_json::Map<String, serde_json::Value>| {
            ics_mcp::call_tool(&mut state, t, a)
        });
        reg.stop_all();
        r
    } else if ics_mcp::has_tool(tool) {
        ics_mcp::call_tool(&mut state, tool, args)
    } else {
        return Err(format!("unknown tool '{tool}' (see `ics ext list`)"));
    };
    if res.is_error {
        return Err(res.text);
    }
    match res.value() {
        Some(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or(res.text)),
        None => println!("{}", res.text),
    }
    let (project, dirty) = state.into_parts();
    if dirty
        && !no_save
        && let Some(p) = project
    {
        p.save().map_err(|e| e.to_string())?;
        eprintln!("saved {}", dir.display());
    }
    Ok(())
}

fn ext(cmd: ExtCmd) -> Result<(), String> {
    use ics_mcp::ext::install::{self, Source};
    use ics_mcp::ext::{Context, Registry};
    match cmd {
        ExtCmd::List { json } => {
            let root = ext_root()?;
            let reg = Registry::load_from(&root);
            if json {
                let list: Vec<serde_json::Value> = reg
                    .extensions
                    .iter()
                    .map(|e| {
                        let i = &e.manifest.extension;
                        serde_json::json!({
                            "id": i.id, "name": i.name, "version": i.version, "description": i.description,
                            "enabled": e.enabled, "problem": e.problem, "dir": e.dir,
                            "tools": e.manifest.tools.iter().map(|t| e.manifest.tool_name(&t.name)).collect::<Vec<_>>(),
                            "commands": e.manifest.commands.iter().map(|c| format!("{}/{}", i.id, c.id)).collect::<Vec<_>>(),
                            "source": e.source().map(|s| s.describe()),
                        })
                    })
                    .collect();
                let errors: Vec<serde_json::Value> =
                    reg.errors.iter().map(|(p, e)| serde_json::json!({"dir": p, "error": e})).collect();
                println!("{}", serde_json::json!({"dir": root, "extensions": list, "errors": errors}));
                return Ok(());
            }
            println!("extensions in {}", root.display());
            if reg.extensions.is_empty() {
                println!("  (none; `ics ext search` lists published ones)");
            }
            for e in &reg.extensions {
                let i = &e.manifest.extension;
                let state = match (&e.problem, e.enabled) {
                    (Some(p), _) => format!("unusable: {p}"),
                    (None, true) => "enabled".into(),
                    (None, false) => "disabled".into(),
                };
                println!("  {} {}  {}  [{state}]", i.id, i.version, i.name);
                if !i.description.is_empty() {
                    println!("      {}", i.description);
                }
                for t in &e.manifest.tools {
                    println!("      tool    {}", e.manifest.tool_name(&t.name));
                }
                for c in &e.manifest.commands {
                    println!("      command {}/{}  {}", i.id, c.id, c.title);
                }
                for p in &e.manifest.pdks {
                    println!("      pdk     {}  {}", p.name, p.file);
                }
            }
            for (p, e) in &reg.errors {
                println!("  ! {}: {e}", p.display());
            }
        }
        ExtCmd::Install { source, path, rev, force } => {
            let root = ext_root()?;
            let src = match Source::parse(&source, rev.clone(), path.clone()) {
                Ok(s) => s,
                // Not a folder or URL: an id of the index.
                Err(_) if !source.contains(['/', '\\', ':']) => {
                    let index = install::fetch_index(&install::index_location())?;
                    let e = index.into_iter().find(|e| e.id == source).ok_or_else(|| {
                        format!("'{source}' is not a folder, a git URL or an extension of the index (`ics ext search`)")
                    })?;
                    let mut s = e.source();
                    if let Source::Git { rev: r, .. } = &mut s
                        && rev.is_some()
                    {
                        *r = rev;
                    }
                    s
                }
                Err(e) => return Err(e),
            };
            let (m, dir) = install::install(&root, &src, force)?;
            let i = &m.extension;
            println!("installed {} {} into {}", i.id, i.version, dir.display());
            if m.run.is_some() {
                println!(
                    "note: it runs its own program ({}) with your rights",
                    m.run.as_ref().map(|r| r.argv().join(" ")).unwrap_or_default()
                );
            }
        }
        ExtCmd::Update { id } => {
            let (m, _) = install::update(&ext_root()?, &id)?;
            println!("updated {} to {}", id, m.extension.version);
        }
        ExtCmd::Remove { id } => {
            install::remove(&ext_root()?, &id)?;
            println!("removed {id}");
        }
        ExtCmd::Enable { id } => install::set_enabled(&ext_root()?, &id, true)?,
        ExtCmd::Disable { id } => install::set_enabled(&ext_root()?, &id, false)?,
        ExtCmd::Search { query, json } => {
            let index = install::fetch_index(&install::index_location())?;
            let installed = ext_root().map(|r| Registry::load_from(&r)).unwrap_or_default();
            let hits: Vec<_> = index.iter().filter(|e| e.matches(query.as_deref().unwrap_or(""))).collect();
            if json {
                let v: Vec<serde_json::Value> = hits
                    .iter()
                    .map(|e| {
                        serde_json::json!({"id": e.id, "name": e.name, "description": e.description,
                        "repository": e.repository, "path": e.path, "installed": installed.get(&e.id).is_some()})
                    })
                    .collect();
                println!("{}", serde_json::Value::Array(v));
                return Ok(());
            }
            for e in &hits {
                let mark = if installed.get(&e.id).is_some() { " (installed)" } else { "" };
                println!("{}{mark}  {}\n    {}", e.id, e.name, e.description);
            }
            if hits.is_empty() {
                println!("no extension matches");
            }
        }
        ExtCmd::New { dir, id, template } => {
            let id = id.unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
            let t = match template {
                ExtTemplate::Python => install::Template::Python,
                ExtTemplate::Skill => install::Template::Skill,
            };
            install::scaffold(&dir, &id, t)?;
            install::read_manifest(&dir)?;
            println!("created {} in {}; try it with `ics ext install {}`", id, dir.display(), dir.display());
        }
        ExtCmd::Check { dir } => {
            let m = install::read_manifest(&dir)?;
            for c in &m.commands {
                if let Some(f) = &c.skill
                    && !dir.join(f).is_file()
                {
                    return Err(format!("command {}: {} does not exist", c.id, f));
                }
            }
            for p in &m.pdks {
                if !dir.join(&p.file).is_file() {
                    return Err(format!("pdk {}: {} does not exist", p.name, p.file));
                }
            }
            println!(
                "{} {}: {} tools, {} commands, {} PDKs",
                m.id(),
                m.extension.version,
                m.tools.len(),
                m.commands.len(),
                m.pdks.len()
            );
        }
        ExtCmd::Call { project, tool, arguments, no_save } => {
            let args: serde_json::Value = serde_json::from_str(&arguments).map_err(|e| format!("arguments: {e}"))?;
            let args = args.as_object().cloned().ok_or("arguments must be a JSON object")?;
            ext_call(&project, &tool, &args, no_save)?;
        }
        ExtCmd::Run { project, command, cell, no_save } => {
            let (id, cmd) = command.split_once('/').ok_or("give the command as <extension id>/<command id>")?;
            let reg = Registry::load();
            let root = std::fs::canonicalize(&project).ok();
            let inv = reg.invocation(id, cmd, &Context { cell, project: root })?;
            ext_call(&project, &inv.tool, &inv.arguments, no_save)?;
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
