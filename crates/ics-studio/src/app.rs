//! Application shell: workspaces, project explorer, tabs, commands, dialogs and persistence.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Key, KeyboardShortcut, Modifiers, RichText, Sense, Stroke, Vec2};
use ics_core::schematic::PortDir;
use ics_core::{Cell, CellRef, DeviceKind, Layout, Project, SchObj, Schematic, ViewKind, examples};
use ics_mcp::bridge::Pending;
use ics_mcp::{ToolResult, tools};
use ics_sim::SimOptions;
use serde::{Deserialize, Serialize};

use crate::calc_view::Calculator;
use crate::canvas::Profile;
use crate::canvas::View2D;
use crate::ciw::{Ciw, CiwKind};
use crate::display_view::DisplayEditor;
use crate::fill_view::FillTool;
use crate::gen_view::{GenAction, GenTab, LayoutGenerators};
use crate::hier_view::{HierAction, HierBrowser};
use crate::home_view;
use crate::i18n::{t, tf};
use crate::icons::{self, Icon};
use crate::layout_view::{EipContext, LayAction, LayCtx, LayRequest, LayTool, LayoutEditor, Verify, fmt_count, um};
use crate::net_view::{NetAction, NetBrowser};
use crate::opc_view::OpcTool;
use crate::pcell_designer::{PCellAction, PCellDesigner};
use crate::pixel_circuit_view::PixelCircuitView;
use crate::pixel_view::{PixelAction, PixelOptics};
use crate::rule_designer::{DesignerAction, RuleDesigner};
use crate::schematic_view::{SchAction, SchRequest, SchTool, SchematicEditor, device_palette};
use crate::sim_view::{self, SimEvent, SimState};
use crate::stream_map_view::StreamMapEditor;
use crate::sweep_view::{SweepAction, SweepManager};
use crate::theme::{self, Palette};
use crate::update;
use ics_core::gds::StreamFormat;
use ics_core::hier::Hier;
use ics_core::project::LibStorage;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Hash)]
pub enum Workspace {
    Home,
    Schematic,
    Simulation,
    Layout,
}

impl Workspace {
    pub const ALL: [Workspace; 4] = [Workspace::Home, Workspace::Schematic, Workspace::Simulation, Workspace::Layout];
    pub fn name(self) -> &'static str {
        match self {
            Workspace::Home => "Home",
            Workspace::Schematic => "Schematic",
            Workspace::Simulation => "Simulation",
            Workspace::Layout => "Layout",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Workspace::Home => Icon::Home,
            Workspace::Schematic => Icon::Schematic,
            Workspace::Simulation => Icon::Simulation,
            Workspace::Layout => Icon::Layout,
        }
    }
    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            Workspace::Home => p.accent,
            Workspace::Schematic => p.schematic,
            Workspace::Simulation => p.simulation,
            Workspace::Layout => p.layout,
        }
    }
}

/// Every user-facing action goes through a command, so menus, shortcuts, the command
/// palette and (later) scripting / MCP interfaces share one code path.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    NewProjectDialog,
    NewDemoProject,
    /// Write the example projects, one per topic, to a folder chosen by the user.
    SaveExamples,
    /// Open the example project of a topic (`examples::TOPICS`), unsaved.
    OpenExample(String),
    OpenProjectDialog,
    OpenProject(PathBuf),
    Save,
    SaveAs,
    CloseProject,
    NewCellDialog,
    OpenCell(CellRef, Option<Workspace>),
    CloseTab(CellRef),
    /// Close every tab but this one.
    CloseOtherTabs(CellRef),
    CloseAllTabs,
    RenameCellDialog(CellRef),
    /// Copy a cell (every view, specs and notes) as the starting point of a new design.
    CopyCellDialog(CellRef),
    /// Start a new design from a cell: copy it, set new targets, size and lay it out.
    DeriveDialog(CellRef),
    /// Turn beginner mode (menus with the everyday flow only) on or off.
    BeginnerMode(bool),
    /// Switch the UI language.
    SetLanguage(crate::i18n::Lang),
    DeleteCellConfirm(CellRef),
    /// New cell dialog with the library preselected.
    NewCellInLibrary(String),
    RenameLibraryDialog(String),
    DeleteLibraryConfirm(String),
    /// Store a library as JSON cell directories or as one GDSII file.
    SetLibraryStorage(String, LibStorage),
    /// Export one library's layouts (its top cells and everything they place) as GDSII / OASIS.
    ExportLibraryStream(String, StreamFormat),
    CreateView(CellRef, ViewKind),
    SetWorkspace(Workspace),
    RunSimulation,
    /// Open the Waveform Calculator on the active cell's results.
    Calculator,
    /// Image sensor pixel optics: QE, crosstalk and photocurrent of a pixel stack.
    PixelOptics,
    /// Run the pixel model of the Pixel Optics window.
    RunPixelOptics,
    /// Fill the Pixel Optics window with the active cell's stack from the technology's cross
    /// section.
    PixelStackFromPdk,
    /// Write the photocurrent of a Bayer colour (0 R, 1 Gr, 2 Gb, 3 B) into a current source of
    /// the active cell's schematic.
    ApplyPhotocurrent(String, usize),
    /// Load a measured QE table into the Pixel Optics window (a file dialog without a path).
    LoadMeasuredQe(Option<PathBuf>),
    /// Fit the Pixel Optics stack's parameters (the window's Fit list) to the measured QE.
    FitPixelStack,
    /// Write the Pixel Optics device model's mean pinning voltage and storage capacitance into
    /// the technology's photodiode (the circuit simulation's pinned photodiode).
    ApplyPixelDevice,
    /// Evaluate a calculator expression on the active cell's shown dataset and plot waveforms.
    CalcPlot(String),
    /// Open the Corner / Sweep Manager on the active cell.
    SweepManager,
    /// Run the active cell's corner / parametric sweep.
    RunSweep,
    /// Open the Verification window (the project's tests, ADE Assembler style).
    Verification,
    /// Run every test of the verification plan.
    RunVerification,
    /// Open the device-model Monte Carlo (process + mismatch) window on the active cell.
    MonteCarlo,
    /// Run the Monte Carlo window's setup on its cell.
    RunMonteCarlo,
    /// Open the Process Data window (WAT / PCM results, SPC, corners).
    ProcessData,
    /// Open the Process Emulation window for the active cell's layout.
    ProcessEmulation,
    /// Open the Silicon Correlation window for the active cell.
    SiliconCorrelation,
    /// Compare the measured curves with the simulation.
    CompareSilicon,
    /// Fit the correlation parameters to the measured curves.
    FitSilicon,
    /// Stop a running Monte Carlo after its current sample, keeping the samples so far.
    StopMonteCarlo,
    /// Open the Results Browser (earlier runs of the active cell, for comparison).
    ResultsBrowser,
    /// Check the active schematic (Check and Save: `true` also saves when it has no errors).
    CheckSchematic(bool),
    /// Choose a cell to place in the active schematic (a hierarchical schematic).
    PlaceCellDialog,
    /// Place instances of this cell in the active schematic (its pins from its schematic).
    PlaceCell(CellRef),
    /// Open the schematic of cell instance `.0` of the active schematic.
    DescendSchematic(u64),
    /// Back to the schematic the last descent started from.
    ReturnSchematic,
    /// Refresh the pins of the active schematic's cell instances from the placed cells.
    UpdateCellInstances,
    /// Edit how the active cell's symbol arranges its pins (side and order).
    EditSymbolDialog,
    /// Open the Net Browser on the active layout's extraction.
    NetBrowser,
    /// Open the Hierarchy Browser.
    HierarchyBrowser,
    /// Open the Layout Generators on the guard ring tab.
    GuardRing,
    /// Open the Layout Generators on the via array tab.
    ViaArray,
    /// Open the Layout Generators on the matched placement tab.
    MatchedPlacement,
    Undo,
    Redo,
    Delete,
    FitView,
    ExportGds,
    ExportLibraryGds,
    ExportOasis,
    ExportLibraryOasis,
    RunDrc,
    ExtractNets,
    /// Draw a schematic of the active layout's extraction (image sensor pixels recognised, with
    /// a test bench): into the cell when it has none, else into a new `<cell>_from_layout`.
    SchematicFromLayout,
    /// Open the Pixel Circuit window on the active layout and characterise its pixels.
    PixelCircuit,
    RunLvs,
    /// IR drop / EM analysis of the active layout: of the named net, else the Verify panel's.
    IrDrop(Option<String>),
    /// Open the DRC Rule Designer window on the technology's deck.
    RuleDesigner,
    /// Open the PCell Designer (on the active layout cell when nothing is being designed).
    PCellDesigner,
    /// Start a PCell Designer design from the active layout cell.
    PCellFromActive,
    /// Set the technology's device layer roles: the gate (poly) layer and the diffusion layers.
    SetDeviceRoles(String, Vec<String>),
    /// Draw new MOS devices with a body pin (`true`) or without one, the arrow on the source.
    MosSymbol(bool),
    /// Set the project's body net of new 3-terminal PMOS (`true`) or NMOS devices; empty is the
    /// built-in default (`vdd` / `0`).
    DefaultBulk(bool, String),
    /// Copy a DRC deck file (IC Studio or SVRF) into the technology; `None` asks for the file.
    ImportDrcDeck(Option<PathBuf>),
    /// Write the technology's DRC deck to a file; `None` asks where.
    ExportDrcDeck(Option<PathBuf>),
    /// Link the technology to a shared DRC deck file, which is re-read when the project opens
    /// and written when the deck is applied; `None` asks for the file.
    LinkDrcDeck(Option<PathBuf>),
    /// Stop using the shared DRC deck file; the project stores its deck itself from then on.
    UnlinkDrcDeck,
    /// Re-read the shared DRC deck file.
    ReloadDrcDeck,
    /// Open the Stream Layer Map editor (GDSII / OASIS layer numbers on import and export).
    StreamMap,
    /// Open the Layer Display editor (fill colour, stipple, outline colour and line style per
    /// layer, as Virtuoso display packets), on the named layer when given.
    LayerDisplay(Option<String>),
    /// Open the Dummy Fill tool.
    DummyFill,
    /// Add dummy fill with the Dummy Fill tool's parameters to the active cell.
    AddFill,
    /// Open the OPC (optical proximity correction) tool.
    Opc,
    /// Write the OPC tool's corrected mask into the active cell.
    ApplyOpc,
    /// Run an external sign-off runset of the technology on the active cell: the one named
    /// (or uniquely matched), else the one selected in the Sign-off section.
    RunSignoff(Option<String>),
    /// Show the results of a sign-off run made elsewhere (KLayout `.lyrdb` / `.lvsdb`, Calibre
    /// results database or LVS report, netgen report) on the active cell; `None` asks for the file.
    ImportSignoffResults(Option<PathBuf>),
    GenerateLayout,
    RouteFlylines,
    /// Compare the active layout with its schematic (Layout XL "Check Against Source"): missing,
    /// extra and changed devices, shorts and opens, listed in the Verify panel.
    CheckAgainstSource,
    /// Bring the active layout in line with its schematic (Layout XL "Update Components and
    /// Nets"): place new devices and resize changed ones in place; `true` also deletes devices
    /// no longer in the schematic.
    UpdateFromSource(bool),
    /// Import a GDSII or OASIS file (detected from its contents); `None` asks for the file.
    ImportGds(Option<PathBuf>),
    /// Import a GDSII or OASIS file through a Cadence Virtuoso layer map file (`RX drawing 2 0`
    /// per line), as XStream In does: the map's rows join the technology's stream map and the
    /// layers it names that the technology lacks are added. Takes the stream file and the map;
    /// `None` asks for it.
    ImportGdsWithLayerMap(Option<PathBuf>, Option<PathBuf>),
    /// Export the active cell's LEF abstract (`false`) or DEF placement (`true`).
    ExportLefDef(bool),
    /// Import a LEF file's macros as a new library (`false`) or a DEF file as a new top cell
    /// (`true`); `None` asks for the file.
    ImportLefDef(bool, Option<PathBuf>),
    ImportTech,
    /// Show the CIW (SKILL command interpreter) tab and focus its prompt.
    Ciw,
    /// Evaluate SKILL in the CIW session, as if typed at its prompt.
    CiwEval(String),
    /// Stop the SKILL script running in the background.
    CancelSkill,
    /// Select the instance of this name in the active cell's schematic (shown in the
    /// inspector).
    SelectInstance(String),
    /// Stop the Virtuoso batch dump running in the background.
    CancelVirtuoso,
    /// Open the spec-driven optimizer for the active cell.
    Optimizer,
    /// Run the optimizer with the cell's saved setup.
    RunOptimizer,
    /// Show or hide the design copilot chat.
    Copilot,
    /// Provider, model and API key for the AI features.
    AiModels,
    /// Ask the copilot a question, or answer the one it is waiting on.
    CopilotAsk(String),
    /// Approve the plan the copilot presented in plan mode.
    CopilotApprove,
    /// Stop the copilot's running request.
    CopilotStop,
    /// Run a Cadence SKILL (.il) script on the project; `None` asks for the file.
    LoadSkill(Option<PathBuf>),
    /// Open the Extensions window (installed extensions, install, update, remove).
    Extensions,
    /// Load the installed extensions again (after installing one by hand).
    ReloadExtensions,
    /// Run menu command `.1` of extension `.0` on the active cell.
    RunExtension(String, String),
    /// Replace the technology with a PDK file (see `ics_core::pdk`); `None` asks for the file.
    LoadPdk(Option<PathBuf>),
    ExportNetlist,
    /// Import a hierarchical CDL netlist (Virtuoso auCdl) as a new cell's schematic; `None`
    /// asks for the file.
    ImportCdl(Option<PathBuf>),
    /// Write the active schematic as a SKILL script that recreates it in Virtuoso.
    ExportSkill,
    /// Import a Virtuoso library dump (`icsDumpLib`): schematics and layouts; `None` asks for
    /// the file. A `cds.lib` or a library directory is dumped by running Virtuoso in batch mode.
    ImportVirtuoso(Option<PathBuf>),
    /// Import library (name) of a `cds.lib` through Virtuoso in batch mode.
    ImportVirtuosoLib(PathBuf, String),
    /// Save `icsDump.il`, the SKILL script that writes a Virtuoso library dump; `None` asks
    /// where.
    SaveDumpSkill(Option<PathBuf>),
    ToggleTheme,
    ToggleConsole,
    Settings,
    Shortcuts,
    UserGuide,
    /// Start the interactive tutorial for first-time users.
    Tutorial,
    About,
    /// Look for a newer release on GitHub and show the update dialog.
    CheckForUpdates,
    /// Download and install the newer release found by the last check.
    InstallUpdate,
    /// Restart into the installed update.
    RestartToUpdate,
    /// Register `ics mcp` (next to this executable) as an MCP server of Claude Code.
    AddMcpToClaudeCode,
    /// Copy an MCP client configuration that starts `ics mcp`.
    CopyMcpConfig,
    CommandPalette,
    /// A layout editor action (Virtuoso menus and bindkeys) on the active layout.
    Lay(LayAction),
    /// A schematic editor action on the active cell.
    Sch(SchAction),
    Quit,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    dark: bool,
    recent: Vec<PathBuf>,
    ngspice_path: String,
    ui_scale: f32,
    reopen_last: bool,
    interaction: Profile,
    check_updates: bool,
    llm: crate::llm::LlmSettings,
    /// App version that last wrote the example projects to the documents folder.
    examples_version: String,
    /// The console panel was open when the app last quit. Closed on first start; errors open it.
    console: bool,
    /// New MOS devices get a body pin (4-terminal); `false` places them without one, the arrow
    /// on the source (3-terminal).
    mos_body_pin: bool,
    /// Beginner mode: menus show the everyday design flow and hide specialist tools (process,
    /// pixel optics, sign-off, SKILL, rule decks, ...), which stay in the command palette.
    beginner: bool,
    /// UI language (Korean needs a system font with Hangul).
    lang: crate::i18n::Lang,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            recent: vec![],
            ngspice_path: ics_sim::ngspice::default_exe(),
            ui_scale: 1.0,
            reopen_last: true,
            interaction: Profile::Virtuoso,
            check_updates: true,
            llm: Default::default(),
            examples_version: String::new(),
            console: false,
            mos_body_pin: true,
            beginner: true,
            lang: crate::i18n::Lang::En,
        }
    }
}

enum Dialog {
    NewProject {
        name: String,
        dir: String,
        demo: bool,
        error: String,
    },
    NewCell {
        lib: String,
        name: String,
        description: String,
        schematic: bool,
        layout: bool,
        error: String,
    },
    RenameCell {
        cell: CellRef,
        name: String,
        error: String,
    },
    CopyCell {
        from: CellRef,
        lib: String,
        name: String,
        error: String,
    },
    ConfirmDelete(CellRef),
    RenameLibrary {
        lib: String,
        name: String,
        error: String,
    },
    ConfirmDeleteLibrary(String),
    Unsaved {
        then: Box<Command>,
    },
    /// Saving stopped: these files changed on disk since the project was opened. `then` runs
    /// after overwriting them (the command that asked to save first).
    SaveConflict {
        files: Vec<std::path::PathBuf>,
        then: Option<Box<Command>>,
    },
    /// Pick a cell to place in the active schematic.
    PlaceCell {
        filter: String,
    },
    /// The pin sides and order of a cell's symbol.
    EditSymbol {
        cell: CellRef,
        pins: Vec<(String, ics_core::schematic::PinSide)>,
    },
    /// An autosaved copy of the project just opened is newer than its files (the app quit
    /// without saving).
    Recover {
        dir: std::path::PathBuf,
        root: std::path::PathBuf,
        saved: u64,
    },
    Settings,
    Shortcuts,
    About,
    Update,
}

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

struct LogLine {
    t: f64,
    level: Level,
    text: String,
}

#[derive(PartialEq, Clone, Copy)]
enum BottomTab {
    Console,
    Ciw,
    Problems,
    Netlist,
}

/// Live-mode link to `ics mcp`: tool calls arrive here and run on the open project.
struct Bridge {
    rx: Receiver<Pending>,
    calls: u64,
    last: Option<(Instant, String)>,
    /// Requests not started yet, served in arrival order.
    waiting: VecDeque<Pending>,
    job: Option<McpJob>,
}

/// A read-only tool call running on a worker thread against a snapshot of the project.
struct McpJob {
    tool: String,
    cell: Option<CellRef>,
    /// Layout editor generation of `cell` when the snapshot was taken.
    generation: u64,
    started: Instant,
    rx: Receiver<(ToolResult, Option<tools::Verified>)>,
    reply: Sender<ToolResult>,
}

type McpArgs = serde_json::Map<String, serde_json::Value>;
type McpSnapshot = (Project, McpArgs, Option<CellRef>);

/// A cell's schematic pins and symbol arrangement.
type PinsAndSymbol = (Vec<(String, PortDir)>, Option<ics_core::schematic::SymbolSpec>);

/// MCP tools that change neither layouts nor schematics. Every other tool runs between
/// snapshots of the project, so whatever it changes is undoable and redrawn.
const MCP_READ_ONLY: [&str; 29] = [
    "project_info",
    "docs_search",
    "docs_read",
    "pcell_design_check",
    "pcell_design_export_skill",
    "gui_status",
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
    "diff_layout",
    "check_against_source",
    "export_gds",
    "export_lef",
    "export_def",
    "export_skill_schematic",
    "verify",
    "predict_yield",
    "process_data",
    "process_cross_section",
    "extract_mismatch",
    "extract_corners",
];

/// Virtuoso dumping a library in the background ([`IcStudio::import_virtuoso`]).
struct VirtuosoJob {
    path: PathBuf,
    /// The library, for the progress window.
    what: String,
    rx: Receiver<Result<String, String>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    started: std::time::Instant,
}

/// A SKILL evaluation running in the background ([`IcStudio::skill_start`]).
struct SkillJob {
    kind: SkillJobKind,
    /// Every layout before the script, for undo history and change detection.
    before: HashMap<CellRef, Layout>,
    started: std::time::Instant,
}

enum SkillJobKind {
    Ciw,
    Load(PathBuf),
    Form,
}

/// Every cell layout, to tell afterwards which ones a SKILL script changed.
fn layout_snapshot(p: &Project) -> HashMap<CellRef, Layout> {
    p.all_cells().filter_map(|(r, c)| Some((r, c.layout.clone()?))).collect()
}

fn schematic_snapshot(p: &Project) -> HashMap<CellRef, Schematic> {
    p.all_cells().filter_map(|(r, c)| Some((r, c.schematic.clone()?))).collect()
}

/// One-line console summary of an MCP tool result.
fn mcp_summary(tool: &str, res: &ToolResult) -> String {
    let Some(v) = res.value().filter(|_| !res.is_error) else {
        let line = res.text.lines().next().unwrap_or_default();
        return line.chars().take(160).collect();
    };
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    let len = |k: &str| v[k].as_array().map_or(0, Vec::len);
    match tool {
        "run_lvs" if v["matched"] == true => "LVS clean".into(),
        "run_lvs" => format!("{} difference(s)", n("differences")),
        "run_drc" if v["clean"] == true => "DRC clean".into(),
        "run_drc" => format!("{} violation(s)", n("violations")),
        "extract" => format!("{} net(s), {} device(s)", n("net_count"), n("device_count")),
        "simulate" | "run_spice" => format!("{} dataset(s)", len("datasets")),
        "generate_layout" => format!("placed {}, {} flyline(s)", len("placed"), n("unrouted_flylines")),
        "flylines" => format!("{} flyline(s)", n("count")),
        "abut" => format!("{} abutment(s)", len("abutted")),
        "check_against_source" => v["summary"].as_str().unwrap_or("checked").into(),
        "update_from_source" => format!("placed {}, updated {}, removed {}", len("placed"), len("updated"), len("removed")),
        "route" => format!("{} connection(s), {} flyline(s) left", n("connections"), n("unrouted_flylines")),
        "delete_items" => format!("deleted {}", len("deleted")),
        "write_schematic" => format!("{} device(s), {} net(s)", n("devices"), len("nets")),
        "add_rect" | "add_path" | "add_label" | "place_instance" | "place_pcell" => format!("id {}", v["id"]),
        "open_project" | "new_project" | "project_info" => format!("project '{}'", v["name"].as_str().unwrap_or("?")),
        "import_gds" | "import_lef" => format!("library '{}'", v["library"].as_str().unwrap_or("?")),
        "import_def" => format!("cell '{}'", v["cell"].as_str().unwrap_or("?")),
        "run_skill" => format!("{} cell(s) modified", len("modified")),
        // Extension tools: their "message", if any, is the line shown in the console.
        _ if tool.contains(ics_mcp::ext::SEP) => v["message"].as_str().unwrap_or("ok").chars().take(240).collect(),
        _ => "ok".into(),
    }
}

const GUIDE_URL: &str = "https://pages.euiyun.com/ic-studio/";

const MOD: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };

type ShortcutGroup = (&'static str, &'static [(&'static str, &'static str)]);

const SHORTCUTS: &[ShortcutGroup] = &[
    (
        "General",
        &[
            ("Mod+K / Mod+Shift+P", "Command palette"),
            ("Mod+S / Mod+Shift+S", "Save / save as"),
            ("Mod+O", "Open project"),
            ("Mod+N", "New cell"),
            ("Mod+W", "Close tab"),
            ("Mod+Z / Mod+Y", "Undo / redo"),
            ("Mod+1 … Mod+4", "Home, Schematic, Simulation, Layout"),
            ("F5 / Mod+Enter", "Run simulation"),
            ("Mod+J", "Toggle console (layout: flip)"),
            ("Mod+,", "Settings"),
            ("F1", "Keyboard shortcuts"),
            ("F2", "Save"),
        ],
    ),
    (
        "Canvas",
        &[
            ("Wheel", "Pan (Conventional profile: zoom)"),
            ("Ctrl+wheel / pinch", "Zoom at the cursor"),
            ("Shift+wheel", "Pan horizontally"),
            ("Middle drag", "Pan"),
            ("Right drag", "Layout: zoom into the box (Conventional: pan); schematic: pan"),
            ("Middle click (layout)", "Pop-up menu"),
            ("F", "Fit view"),
            ("Esc", "Cancel, back to Select, clear selection"),
            ("Del / Backspace", "Delete selection"),
            ("Mod+A / Mod+D", "Select all / deselect all"),
            ("Shift+Mod+D", "Schematic: duplicate"),
        ],
    ),
    (
        "Schematic",
        &[
            ("W", "Wire"),
            ("T", "Net label"),
            ("R C L V I D", "Resistor, capacitor, inductor, V and I source, diode"),
            ("N P", "NMOS, PMOS"),
            ("G S", "Ground, supply"),
            ("Space / X", "Rotate / mirror (placement or selection)"),
            ("/", "Swap wire bend direction"),
            ("Shift+I", "Place a cell instance"),
            ("Shift+X / Shift+B", "Descend into a cell instance / return"),
        ],
    ),
    (
        "Layout",
        &[
            ("Esc", "Cancel the command, back to Select"),
            ("S", "Stretch an edge or vertex"),
            ("M / C", "Move, copy: reference point, then destination"),
            ("Right-click / Shift+right-click", "Rotate, flip while moving or copying"),
            ("R / Shift+P / P", "Rectangle, polygon, path"),
            ("L O I K", "Label, via, instance, ruler"),
            ("Shift+C / Shift+M", "Chop, merge"),
            ("Shift+O", "Rotate the selection 90°"),
            ("+ / - (in path)", "Change metal and drop a via"),
            ("u / Shift+U", "Undo, redo"),
            ("y / Shift+Y", "Yank, paste (also Mod+C / Mod+V)"),
            ("T", "Layer tap: activate the layer under the cursor"),
            ("G", "Toggle gravity"),
            ("Mod+A / Mod+D", "Select all, deselect all"),
            ("Shift+X / x / Shift+B", "Descend into an instance, edit it in place, return"),
            ("Shift+T", "Hierarchy tree"),
            ("Mod+J", "Flip sideways (also while moving)"),
            ("Shift+N", "Mark net mode"),
            ("q", "Edit object properties"),
            ("F3", "Options of the current command"),
            ("e / Shift+E", "Display options, editor options"),
            ("F4", "Toggle partial select (edges and vertices)"),
            ("S + box", "Stretch every vertex inside the box"),
            ("Enter / Backspace", "Finish, remove last point while drawing"),
            ("Arrows / Shift+arrows", "Pan, nudge the selection 10 snap steps"),
            ("Z / Shift+Z", "Zoom box, zoom out"),
            ("[ / ]", "Zoom in, out at the cursor"),
            ("W / Shift+W", "Previous, next view"),
            ("F", "Fit"),
            ("Shift+K", "Clear rulers"),
            ("Shift+S", "Split shapes along a line"),
            ("Shift+F / Mod+F", "All levels, top level only"),
            ("0 … 8, 9", "Drawn hierarchy depth, 9 = all"),
            ("Right-to-left box", "Select everything the box touches"),
        ],
    ),
];

/// Explorer hover text of a cell: its full name (long names are cut by the panel) and description.
fn cell_tip(cell: &Cell) -> String {
    if cell.description.is_empty() { cell.name.clone() } else { format!("{}\n{}", cell.name, cell.description) }
}

/// How long a warning stays on the status bar.
const FLASH_TIME: Duration = Duration::from_secs(8);

const NEED_PROJECT: &str = "Open a project first";
const NEED_SCH: &str = "Open a cell with a schematic first";
const NEED_LAY: &str = "Open a cell with a layout first";

/// A menu button that is greyed out, with the reason on hover, while it cannot run.
/// Show the built-in design notes of demo cells in the UI language. Notes the user changed
/// are left alone.
fn localize_demo_notes(p: &mut Project) {
    if crate::i18n::current() != crate::i18n::Lang::Ko {
        return;
    }
    for c in p.libraries.iter_mut().flat_map(|l| &mut l.cells) {
        if !c.notes.is_empty()
            && c.notes == examples::design_notes(&c.name)
            && let Some(ko) = crate::i18n_ko::design_notes(&c.name)
        {
            c.notes = ko.to_string();
        }
    }
}

fn menu_item(ui: &mut egui::Ui, enabled: bool, label: &str, why: &str) -> bool {
    ui.add_enabled(enabled, egui::Button::new(t(label))).on_disabled_hover_text(t(why)).clicked()
}

/// Items of one of the layout editor's menus ([`LayAction::MENUS`]).
fn lay_menu_items(ui: &mut egui::Ui, menu: &str, q: &mut Vec<Command>) {
    let items = LayAction::MENUS.iter().find(|m| m.0 == menu).map_or(&[][..], |m| m.1);
    for item in items {
        match item {
            Some(a) => {
                if ui.add(egui::Button::new(a.name()).shortcut_text(a.key())).clicked() {
                    q.push(Command::Lay(*a));
                }
            }
            None => {
                ui.separator();
            }
        }
    }
}

struct Descent {
    parent: CellRef,
    child: CellRef,
    /// Edited in place: the instance, its array element and child-to-parent transform.
    at: Option<(u64, (u32, u32), ics_core::Trans)>,
}

/// The edit-in-place chain ending at `r`, if `r` was entered in place.
fn eip_context(stack: &[Descent], r: &CellRef) -> Option<EipContext> {
    let (mut cur, mut t, mut path) = (r, ics_core::Trans::IDENTITY, vec![]);
    for d in stack.iter().rev() {
        let Some((inst, elem, at)) = d.at.filter(|_| &d.child == cur) else { break };
        t = at.compose(&t);
        path.insert(0, (inst, elem));
        cur = &d.parent;
    }
    (!path.is_empty()).then(|| EipContext { root: cur.clone(), to_root: t, path })
}

struct PaletteState {
    query: String,
    sel: usize,
}

pub struct IcStudio {
    project: Option<Project>,
    dirty: bool,
    workspace: Workspace,
    tabs: Vec<CellRef>,
    active: Option<CellRef>,
    /// Library selected in the Explorer (its header was clicked).
    selected_lib: Option<String>,
    /// Explorer filter: a part of a cell name; empty shows the library tree.
    explorer_filter: String,
    sch_eds: HashMap<CellRef, SchematicEditor>,
    lay_eds: HashMap<CellRef, LayoutEditor>,
    /// Cells descended into (Shift+X) or edited in place (x), for Shift+B (return).
    lay_return: Vec<Descent>,
    /// Schematics descended from (Shift+X), most recent last.
    sch_return: Vec<CellRef>,
    hier: Hier,
    /// Device names last selected in either editor, for cross-probing.
    probe: Option<(CellRef, BTreeSet<String>)>,
    sims: HashMap<CellRef, SimState>,
    log: Vec<LogLine>,
    started: Instant,
    /// The last warning, flashed on the status bar while the console does not show it.
    flash: Option<(String, Instant)>,
    show_console: bool,
    bottom_tab: BottomTab,
    queue: Vec<Command>,
    dialog: Option<Dialog>,
    palette: Option<PaletteState>,
    settings: Settings,
    ngspice: Option<String>,
    allow_close: bool,
    last_title: String,
    bridge: Option<Bridge>,
    rule_designer: RuleDesigner,
    pcell_designer: PCellDesigner,
    copilot: crate::copilot::Copilot,
    ai_models: crate::llm::AiSettingsWindow,
    updater: crate::update::Updater,
    /// Background copies of the project while it has unsaved changes ([`crate::recovery`]).
    autosave: crate::recovery::Autosave,
    /// Version announced by the start-up update check, until dismissed.
    update_toast: Option<String>,
    optimizer: crate::optimizer_view::Optimizer,
    derive: crate::derive_view::DeriveWizard,
    stream_map: StreamMapEditor,
    display: DisplayEditor,
    fill: FillTool,
    opc: OpcTool,
    /// Mask layers OPC added to the technology (still there / taken out again by undo), so
    /// that undo and redo of the layout also remove and restore their definitions.
    opc_layers: (Vec<String>, Vec<ics_core::LayerDef>),
    calc: Calculator,
    pixel: PixelOptics,
    pixel_circuit: PixelCircuitView,
    sweep: SweepManager,
    monte_carlo: crate::mc_view::MonteCarlo,
    verification: crate::verify_view::Verification,
    silicon: crate::silicon_view::SiliconView,
    process: crate::process_view::ProcessView,
    process_data: crate::process_data_view::ProcessDataView,
    results_open: bool,
    checks: Vec<ics_core::sch_check::Finding>,
    nets: NetBrowser,
    hier_browser: HierBrowser,
    gens: LayoutGenerators,
    ciw: Ciw,
    /// A `cds.lib` being imported, its libraries and whether Virtuoso was found, waiting for the
    /// user to pick one.
    virtuoso_pick: Option<(PathBuf, Vec<String>, bool)>,
    /// A Virtuoso batch dump running in the background: the source path and its result.
    virtuoso_job: Option<VirtuosoJob>,
    /// SKILL forms displayed by scripts, waiting for OK or Cancel: the form and its edited fields.
    skill_forms: Vec<(ics_core::skill::FormRequest, Vec<ics_core::skill::FormField>)>,
    /// A SKILL evaluation running in the background, on a copy of the project.
    skill_job: Option<SkillJob>,
    tutorial: crate::tutorial::Tutorial,
    /// Installed extensions ([`ics_mcp::ext`]).
    extensions: ics_mcp::ext::Registry,
    ext_manager: crate::ext_view::ExtensionManager,
    /// Calls back and log lines from extension programs running a tool.
    ext_tx: Sender<ExtMsg>,
    ext_rx: Receiver<ExtMsg>,
}

/// From an extension program, while one of its tools runs on a worker thread.
enum ExtMsg {
    /// A built-in tool to run on the open project.
    Call(Pending),
    Log(String, String),
}

/// The app as seen by an extension program: its calls run on the UI thread.
struct ExtHost {
    tx: Sender<ExtMsg>,
    wake: egui::Context,
}

impl ics_mcp::ext::Host for ExtHost {
    fn call(&mut self, tool: &str, args: &McpArgs) -> ToolResult {
        let (reply, answer) = mpsc::channel();
        if self.tx.send(ExtMsg::Call(Pending { tool: tool.to_string(), arguments: args.clone(), reply })).is_err() {
            return ToolResult::error("IC Studio is closing");
        }
        self.wake.request_repaint();
        answer.recv().unwrap_or_else(|_| ToolResult::error("IC Studio dropped the call"))
    }

    fn log(&mut self, level: &str, message: &str) {
        let _ = self.tx.send(ExtMsg::Log(level.to_string(), message.to_string()));
        self.wake.request_repaint();
    }
}

const SETTINGS_KEY: &str = "ic-studio-settings";

/// Where the installer and the app put the example projects: `Documents/IC Studio/Examples`
/// (or `~/IC Studio/Examples` without a documents folder).
fn examples_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ICS_EXAMPLES_DIR").filter(|dir| !dir.is_empty()) {
        return Some(dir.into());
    }
    dirs::document_dir().or_else(dirs::home_dir).map(|d| d.join("IC Studio").join("Examples"))
}

impl IcStudio {
    pub fn new(cc: &eframe::CreationContext<'_>, args: crate::LaunchArgs) -> Self {
        let settings: Settings =
            cc.storage.and_then(|s| s.get_string(SETTINGS_KEY)).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        theme::setup_fonts(&cc.egui_ctx);
        crate::i18n::set(settings.lang);
        theme::apply(&cc.egui_ctx, settings.dark);
        cc.egui_ctx.set_zoom_factor(settings.ui_scale);
        let ngspice = ics_sim::ngspice::detect(&settings.ngspice_path);
        let (ext_tx, ext_rx) = mpsc::channel();
        let mut app = Self {
            project: None,
            dirty: false,
            workspace: Workspace::Home,
            tabs: vec![],
            selected_lib: None,
            explorer_filter: String::new(),
            active: None,
            sch_eds: HashMap::new(),
            lay_eds: HashMap::new(),
            lay_return: vec![],
            sch_return: vec![],
            hier: Hier::default(),
            probe: None,
            sims: HashMap::new(),
            log: vec![],
            started: Instant::now(),
            flash: None,
            show_console: settings.console,
            bottom_tab: BottomTab::Console,
            queue: vec![],
            dialog: None,
            palette: None,
            settings,
            ngspice,
            allow_close: false,
            last_title: String::new(),
            bridge: None,
            rule_designer: RuleDesigner::default(),
            pcell_designer: PCellDesigner::default(),
            copilot: Default::default(),
            ai_models: Default::default(),
            updater: Default::default(),
            autosave: Default::default(),
            update_toast: None,
            optimizer: Default::default(),
            derive: Default::default(),
            stream_map: StreamMapEditor::default(),
            display: DisplayEditor::default(),
            fill: FillTool::default(),
            opc: OpcTool::default(),
            opc_layers: Default::default(),
            calc: Calculator::default(),
            pixel: PixelOptics::default(),
            pixel_circuit: PixelCircuitView::default(),
            sweep: SweepManager::default(),
            monte_carlo: Default::default(),
            verification: Default::default(),
            silicon: Default::default(),
            process: Default::default(),
            process_data: Default::default(),
            results_open: false,
            checks: vec![],
            nets: NetBrowser::default(),
            hier_browser: HierBrowser::default(),
            gens: LayoutGenerators::default(),
            ciw: Ciw::default(),
            virtuoso_pick: None,
            virtuoso_job: None,
            skill_forms: vec![],
            skill_job: None,
            tutorial: Default::default(),
            extensions: ics_mcp::ext::Registry::load(),
            ext_manager: Default::default(),
            ext_tx,
            ext_rx,
        };
        app.info(format!("IC Studio {} ready — renderer: wgpu", env!("CARGO_PKG_VERSION")));
        match &app.ngspice {
            Some(v) => app.info(format!("Found {v}")),
            None => app.info("ngspice not found; using the built-in simulator (set a path in Settings)".into()),
        }
        if let Some(p) = &args.project {
            app.open_project(p);
        } else if args.demo {
            let p = examples::demo_project();
            app.set_project(p);
        } else if app.settings.reopen_last
            && let Some(p) = app.settings.recent.first().cloned()
            && p.join(ics_core::project::MANIFEST).exists()
        {
            app.open_project(&p);
        }
        if let (Some(c), Some(p)) = (&args.cell, &app.project)
            && let Some((r, _)) = p.all_cells().find(|(_, x)| &x.name == c)
        {
            app.tabs.clear();
            app.active = None;
            app.queue.push(Command::OpenCell(r, None));
        }
        if let Some(w) = &args.workspace
            && let Some(ws) = Workspace::ALL.iter().find(|x| x.name().eq_ignore_ascii_case(w))
        {
            app.queue.push(Command::SetWorkspace(*ws));
        }
        if args.run {
            app.queue.push(Command::RunSimulation);
        }
        app.copilot.ext = app.extensions.clone();
        for (p, e) in app.extensions.errors.clone() {
            app.warn(format!("extension in {}: {e}", p.display()));
        }
        app
    }

    /// The installed extensions.
    pub fn extensions(&self) -> &ics_mcp::ext::Registry {
        &self.extensions
    }

    /// Use these extensions (tests, or after installing one).
    pub fn set_extensions(&mut self, reg: ics_mcp::ext::Registry) {
        self.extensions.stop_all();
        self.copilot.ext = reg.clone();
        self.extensions = reg;
    }

    /// Queue a tool call as if an agent sent it (extension commands; the reply is logged).
    fn queue_tool(&mut self, tool: String, arguments: McpArgs) {
        let (reply, _) = mpsc::channel();
        self.bridge_queue().push_back(Pending { tool, arguments, reply });
    }

    /// Tool calls waiting to run, created without a live-mode bridge.
    fn bridge_queue(&mut self) -> &mut VecDeque<Pending> {
        &mut self
            .bridge
            .get_or_insert_with(|| Bridge {
                rx: std::sync::mpsc::channel().1,
                calls: 0,
                last: None,
                waiting: VecDeque::new(),
                job: None,
            })
            .waiting
    }

    /// Run extension tool `p` on a worker thread; its program's calls back come through `ext_rx`.
    fn start_ext_job(&mut self, ctx: &egui::Context, p: Pending) {
        let (tx, rx) = mpsc::channel();
        let (reg, host_tx, wake) = (self.extensions.clone(), self.ext_tx.clone(), ctx.clone());
        let (tool, args) = (p.tool.clone(), p.arguments.clone());
        std::thread::spawn(move || {
            let mut host = ExtHost { tx: host_tx, wake: wake.clone() };
            let res = reg.call(&tool, &args, &mut host);
            let _ = tx.send((res, None));
            wake.request_repaint();
        });
        self.info(format!("Extension · {} running", p.tool));
        if let Some(b) = &mut self.bridge {
            b.job = Some(McpJob { tool: p.tool, cell: None, generation: 0, started: Instant::now(), rx, reply: p.reply });
        }
    }

    /// Serve extension programs: their calls run right away (their tool's job waits for them).
    fn serve_extensions(&mut self) {
        let msgs: Vec<ExtMsg> = self.ext_rx.try_iter().collect();
        for m in msgs {
            match m {
                ExtMsg::Call(p) => {
                    let res = self.mcp_call(&p.tool, &p.arguments);
                    let _ = p.reply.send(res);
                }
                ExtMsg::Log(level, text) => match level.as_str() {
                    "error" | "warning" | "warn" => self.warn(format!("Extension · {text}")),
                    _ => self.info(format!("Extension · {text}")),
                },
            }
        }
    }

    fn extensions_window(&mut self, ctx: &egui::Context) {
        match self.ext_manager.window(ctx, &self.extensions) {
            Some(crate::ext_view::ExtAction::Reload) => self.queue.push(Command::ReloadExtensions),
            Some(crate::ext_view::ExtAction::OpenFolder(d)) => {
                ctx.open_url(egui::OpenUrl::new_tab(crate::ext_view::folder_url(&d)));
            }
            None => {}
        }
    }

    // ----- logging -----
    fn push_log(&mut self, level: Level, text: String) {
        self.log.push(LogLine { t: self.started.elapsed().as_secs_f64(), level, text });
        if self.log.len() > 2000 {
            self.log.drain(0..500);
        }
    }
    fn info(&mut self, t: String) {
        self.push_log(Level::Info, t);
    }
    fn ok(&mut self, t: String) {
        self.push_log(Level::Ok, t);
    }
    fn warn(&mut self, t: String) {
        self.flash = Some((t.clone(), Instant::now()));
        self.push_log(Level::Warn, t);
    }
    fn error(&mut self, t: String) {
        self.push_log(Level::Error, t);
        self.show_console = true;
        self.bottom_tab = BottomTab::Console;
    }

    // ----- project management -----
    fn set_project(&mut self, mut p: Project) {
        localize_demo_notes(&mut p);
        self.project = Some(p);
        self.ciw.reset();
        self.dirty = false;
        self.tabs.clear();
        self.active = None;
        self.selected_lib = None;
        self.sch_eds.clear();
        self.lay_eds.clear();
        self.hier.clear();
        self.sims.clear();
        self.workspace = Workspace::Home;
        // open the first cell as a convenience
        if let Some((r, _)) = self.project.as_ref().and_then(|p| p.all_cells().next()) {
            self.tabs.push(r.clone());
            self.active = Some(r);
        }
    }

    fn remember_recent(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.settings.recent.retain(|p| p != &path);
        self.settings.recent.insert(0, path);
        self.settings.recent.truncate(10);
    }

    fn open_project(&mut self, path: &Path) {
        match Project::open(path) {
            Ok(mut p) => {
                let root = p.root.clone().unwrap();
                self.info(format!("Opened project '{}' ({})", p.name, root.display()));
                if let Some(deck) = p.drc_deck_path() {
                    match p.reload_drc_deck() {
                        Ok(_) => self.info(format!("DRC deck: shared {}", deck.display())),
                        Err(e) => {
                            self.warn(format!("Shared DRC deck not read, using the deck derived from the layer rules: {e}"))
                        }
                    }
                }
                self.set_project(p);
                self.remember_recent(&root);
                if let Some((dir, info)) = crate::recovery::pending(&root) {
                    self.dialog = Some(Dialog::Recover { dir, root, saved: info.saved });
                }
            }
            Err(e) => self.error(format!("Cannot open project: {e}")),
        }
    }

    fn save(&mut self, force_dialog: bool) -> bool {
        let Some(p) = self.project.as_mut() else { return false };
        if p.root.is_none() || force_dialog {
            let Some(dir) = rfd::FileDialog::new().set_title("Choose a folder for the project").pick_folder() else {
                return false;
            };
            let target = if dir.join(ics_core::project::MANIFEST).exists()
                || dir.read_dir().map(|mut d| d.next().is_none()).unwrap_or(true)
            {
                dir
            } else {
                dir.join(&p.name)
            };
            p.root = Some(target);
        }
        let res = p.save();
        self.saved(res)
    }

    /// Save over files other programs changed since the project was opened.
    fn save_overwriting(&mut self) -> bool {
        let Some(p) = self.project.as_mut() else { return false };
        let res = p.save_overwriting();
        self.saved(res)
    }

    fn saved(&mut self, res: Result<(), ics_core::project::ProjectError>) -> bool {
        match res {
            Ok(()) => {
                let Some(p) = self.project.as_ref() else { return false };
                let root = p.root.clone().unwrap_or_default();
                let name = p.name.clone();
                self.dirty = false;
                crate::recovery::discard(&root);
                self.ok(format!("Saved '{name}' to {}", root.display()));
                self.remember_recent(&root);
                true
            }
            Err(ics_core::project::ProjectError::Conflict(files)) => {
                self.warn(format!("Not saved: {} file(s) changed on disk since the project was opened", files.len()));
                self.dialog = Some(Dialog::SaveConflict { files, then: None });
                false
            }
            Err(e) => {
                self.error(format!("Save failed: {e}"));
                false
            }
        }
    }

    /// Arm the active schematic editor to place `r` (its pins from its schematic).
    fn place_cell(&mut self, r: CellRef) {
        let (Some(here), Some(p)) = (self.active.clone(), self.project.as_ref()) else { return };
        let Some(sch) = p.cell(&r).and_then(|c| c.schematic.as_ref()) else {
            self.warn(format!("{r} has no schematic to place"));
            return;
        };
        if r == here {
            self.warn("A cell cannot place itself".into());
            return;
        }
        let (ports, spec) = (sch.ports(), sch.symbol.clone());
        if ports.is_empty() {
            self.warn(format!("{r} has no pins: add schematic pins (labels marked as pins) to connect to it"));
        }
        let ed = self.sch_eds.entry(here).or_default();
        ed.place_cell = Some((r.clone(), ports, spec));
        ed.tool = crate::schematic_view::SchTool::Place(DeviceKind::Cell);
        self.workspace = Workspace::Schematic;
        self.info(format!("Placing {r}: click to place, Esc ends"));
    }

    /// Give the cell instances of every schematic the current pins and symbol of their cells.
    /// Returns how many changed.
    fn refresh_cell_instances(&mut self) -> usize {
        let Some(p) = self.project.as_mut() else { return 0 };
        let defs: HashMap<CellRef, PinsAndSymbol> =
            p.all_cells().filter_map(|(r, c)| c.schematic.as_ref().map(|s| (r, (s.ports(), s.symbol.clone())))).collect();
        let mut n = 0;
        let refs: Vec<CellRef> = p.all_cells().map(|(r, _)| r).collect();
        for r in refs {
            let Some(sch) = p.cell_mut(&r).and_then(|c| c.schematic.as_mut()) else { continue };
            let stale = |i: &ics_core::Instance| {
                i.cell.as_ref().and_then(|c| defs.get(c)).is_some_and(|(ports, spec)| *ports != i.ports || *spec != i.symbol)
            };
            if !sch.instances.iter().any(|i| i.kind == DeviceKind::Cell && stale(i)) {
                continue;
            }
            self.sch_eds.entry(r.clone()).or_default().history.push(sch);
            for i in sch.instances.iter_mut().filter(|i| i.kind == DeviceKind::Cell) {
                if let Some((ports, spec)) = i.cell.as_ref().and_then(|c| defs.get(c))
                    && (*ports != i.ports || *spec != i.symbol)
                {
                    i.ports = ports.clone();
                    i.symbol = spec.clone();
                    n += 1;
                }
            }
        }
        if n > 0 {
            self.mark_dirty();
        }
        n
    }

    /// Update Cell Instance Pins: every schematic's cell instances follow their cells.
    fn update_cell_instances(&mut self) {
        match self.refresh_cell_instances() {
            0 => self.info("Cell instances are up to date".into()),
            n => self.ok(format!("Updated the pins of {n} cell instance(s)")),
        }
    }

    fn guard_unsaved(&mut self, then: Command) -> bool {
        if self.dirty {
            self.dialog = Some(Dialog::Unsaved { then: Box::new(then) });
            false
        } else {
            true
        }
    }

    fn active_cell(&self) -> Option<&Cell> {
        self.project.as_ref()?.cell(self.active.as_ref()?)
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn sim_options(&self, backend: ics_core::Backend) -> SimOptions {
        // Relative `.include` / `.lib` paths in a cell's setup are relative to the project.
        let deck_dir = self.project.as_ref().and_then(|p| p.root.clone());
        SimOptions { backend, ngspice_exe: self.settings.ngspice_path.clone(), deck_dir }
    }

    fn netlist_active(&self) -> Option<(String, Vec<String>)> {
        let r = self.active.as_ref()?;
        let cell = self.active_cell()?;
        let p = self.project.as_ref()?;
        // A hierarchical schematic is simulated flattened; its flattening notes are warnings.
        let (sch, mut warnings) = match p.circuit_with_warnings(r) {
            Ok(c) => c,
            Err(e) => return Some((String::new(), vec![e])),
        };
        let nl = ics_core::netlist::netlist_for(&sch, &p.tech, &cell.sim.corner);
        warnings.extend(nl.warnings.iter().cloned());
        Some((nl.deck(&r.to_string(), &cell.sim), warnings))
    }

    // ----- commands -----
    fn run_command(&mut self, ctx: &egui::Context, cmd: Command) {
        match cmd {
            Command::NewProjectDialog => {
                // Creating a project replaces the open one: save or discard its changes first.
                if !self.guard_unsaved(Command::NewProjectDialog) {
                    return;
                }
                let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or(".".into());
                self.dialog = Some(Dialog::NewProject {
                    name: "my_project".into(),
                    dir: Path::new(&home).join("ICStudioProjects").display().to_string(),
                    demo: false,
                    error: String::new(),
                });
            }
            Command::NewDemoProject => {
                if !self.guard_unsaved(Command::NewDemoProject) {
                    return;
                }
                let mut p = examples::demo_project();
                p.root = None;
                self.set_project(p);
                self.dirty = true;
                self.info("Created demo project (unsaved) — use File ▸ Save to choose a folder".into());
            }
            Command::SaveExamples => {
                let mut dialog = rfd::FileDialog::new().set_title("Save the example projects into a folder");
                if let Some(d) = examples_dir().filter(|d| d.exists()).or_else(dirs::document_dir) {
                    dialog = dialog.set_directory(d);
                }
                if let Some(dir) = dialog.pick_folder() {
                    self.write_examples(&dir);
                }
            }
            Command::OpenExample(name) => {
                if !self.guard_unsaved(Command::OpenExample(name.clone())) {
                    return;
                }
                match examples::topic_project(&name) {
                    Some(p) => {
                        self.set_project(p);
                        self.dirty = true;
                        self.info(format!("Opened the {name} example (unsaved) — use File ▸ Save to choose a folder"));
                    }
                    None => self.warn(format!("No example named {name}")),
                }
            }
            Command::OpenProjectDialog => {
                if !self.guard_unsaved(Command::OpenProjectDialog) {
                    return;
                }
                if let Some(dir) = rfd::FileDialog::new().set_title("Open IC Studio project folder").pick_folder() {
                    self.open_project(&dir);
                }
            }
            Command::OpenProject(p) => {
                if !self.guard_unsaved(Command::OpenProject(p.clone())) {
                    return;
                }
                self.open_project(&p);
            }
            Command::Save => {
                self.save(false);
            }
            Command::SaveAs => {
                self.save(true);
            }
            Command::CloseProject => {
                if !self.guard_unsaved(Command::CloseProject) {
                    return;
                }
                self.project = None;
                self.ciw.reset();
                self.dirty = false;
                self.tabs.clear();
                self.active = None;
                self.workspace = Workspace::Home;
            }
            Command::NewCellDialog => {
                if let Some(p) = &self.project {
                    // With every library deleted the cell goes to a new "work" library.
                    let lib = self
                        .active
                        .as_ref()
                        .map(|r| r.lib.clone())
                        .or_else(|| p.libraries.iter().find(|l| l.name != ics_core::pcell::PCELL_LIB).map(|l| l.name.clone()))
                        .unwrap_or_else(|| "work".into());
                    self.dialog = Some(Dialog::NewCell {
                        lib,
                        name: String::new(),
                        description: String::new(),
                        schematic: true,
                        layout: false,
                        error: String::new(),
                    });
                }
            }
            Command::OpenCell(r, ws) => {
                if !self.tabs.contains(&r) {
                    self.tabs.push(r.clone());
                }
                self.active = Some(r);
                if let Some(ws) = ws {
                    self.workspace = ws;
                } else if self.workspace == Workspace::Home {
                    self.workspace = Workspace::Schematic;
                }
            }
            Command::CloseTab(r) => {
                if let Some(i) = self.tabs.iter().position(|t| *t == r) {
                    self.tabs.remove(i);
                    if self.active.as_ref() == Some(&r) {
                        self.active = self.tabs.get(i.min(self.tabs.len().saturating_sub(1))).cloned();
                    }
                }
            }
            Command::CloseOtherTabs(r) => {
                self.tabs.retain(|t| *t == r);
                if self.tabs.contains(&r) {
                    self.active = Some(r);
                }
            }
            Command::CloseAllTabs => {
                self.tabs.clear();
                self.active = None;
            }
            Command::RenameCellDialog(r) => {
                self.dialog = Some(Dialog::RenameCell { name: r.cell.clone(), cell: r, error: String::new() })
            }
            Command::CopyCellDialog(r) => {
                if let Some(p) = &self.project
                    && p.cell(&r).is_some()
                {
                    let lib = if r.lib == ics_core::pcell::PCELL_LIB {
                        p.libraries.iter().find(|l| l.name != r.lib).map_or_else(|| "work".into(), |l| l.name.clone())
                    } else {
                        r.lib.clone()
                    };
                    let name = p.unused_cell_name(&lib, &r.cell);
                    self.dialog = Some(Dialog::CopyCell { from: r, lib, name, error: String::new() });
                }
            }
            Command::DeriveDialog(r) => {
                if let Some(p) = &self.project {
                    if p.cell(&r).is_some_and(|c| c.schematic.is_some()) {
                        self.derive.show(p, &r);
                    } else {
                        self.warn(format!("{r} has no schematic to derive from; Copy as New Cell copies it as it is"));
                    }
                }
            }
            Command::DeleteCellConfirm(r) => self.dialog = Some(Dialog::ConfirmDelete(r)),
            Command::NewCellInLibrary(lib) => {
                if self.project.as_ref().is_some_and(|p| p.libraries.iter().any(|l| l.name == lib)) {
                    self.dialog = Some(Dialog::NewCell {
                        lib,
                        name: String::new(),
                        description: String::new(),
                        schematic: true,
                        layout: false,
                        error: String::new(),
                    });
                }
            }
            Command::RenameLibraryDialog(lib) => {
                self.dialog = Some(Dialog::RenameLibrary { name: lib.clone(), lib, error: String::new() })
            }
            Command::DeleteLibraryConfirm(lib) => self.dialog = Some(Dialog::ConfirmDeleteLibrary(lib)),
            Command::SetLibraryStorage(lib, storage) => {
                if let Some(p) = self.project.as_mut() {
                    match p.set_library_storage(&lib, storage) {
                        Ok(()) => {
                            self.mark_dirty();
                            let how = if storage == LibStorage::Gds { "one GDSII file" } else { "JSON cell directories" };
                            self.info(format!("Library '{lib}' will be stored as {how} on the next save"));
                        }
                        Err(e) => self.error(format!("Cannot change the storage of '{lib}': {e}")),
                    }
                }
            }
            Command::ExportLibraryStream(lib, format) => self.export_library_stream(&lib, format),
            Command::CreateView(r, v) => {
                if let Some(c) = self.project.as_mut().and_then(|p| p.cell_mut(&r)) {
                    match v {
                        ViewKind::Schematic | ViewKind::Simulation => {
                            c.schematic.get_or_insert_with(Schematic::default);
                        }
                        ViewKind::Layout => {
                            c.layout.get_or_insert_with(Layout::default);
                        }
                    }
                    self.hier.clear();
                    self.mark_dirty();
                    self.info(format!("Created {} view for {r}", v.label()));
                }
                let ws = match v {
                    ViewKind::Schematic => Workspace::Schematic,
                    ViewKind::Layout => Workspace::Layout,
                    ViewKind::Simulation => Workspace::Simulation,
                };
                self.queue.push(Command::OpenCell(r, Some(ws)));
            }
            Command::SetWorkspace(w) => self.workspace = w,
            Command::RunSimulation => self.run_simulation(ctx),
            Command::Calculator => self.calc.show(),
            Command::PixelOptics => self.pixel.show(),
            Command::RunPixelOptics => {
                self.pixel.show();
                self.pixel.run(None, self.project.as_ref());
                // Automation waits for an RCWA run to finish.
                self.pixel.wait();
                match &self.pixel.result {
                    Some(Err(e)) => self.warn(format!("Pixel optics: {e}")),
                    Some(Ok(r)) => self.info(format!("Pixel optics ({}): {} at CRA {} deg", r.method, r.name, r.cra)),
                    None => {}
                }
            }
            Command::PixelStackFromPdk => self.pixel_stack_from_pdk(),
            Command::ApplyPixelDevice => self.apply_pixel_device(),
            Command::LoadMeasuredQe(path) => {
                let path = path.or_else(|| rfd::FileDialog::new().add_filter("Measured QE", &["csv", "txt"]).pick_file());
                if let Some(path) = path {
                    self.pixel.show();
                    if let Err(e) = self.pixel.load_measured(&path) {
                        self.warn(e);
                    }
                }
            }
            Command::FitPixelStack => {
                self.pixel.show();
                self.pixel.start_fit(Some(ctx), self.project.as_ref());
                // Automation waits for the fit.
                self.pixel.wait();
                match &self.pixel.fit {
                    Some(Ok(f)) => self.info(format!("Pixel stack fit: RMS {:.4} -> {:.4}", f.rms_before, f.rms_after)),
                    Some(Err(e)) => self.warn(format!("Pixel stack fit: {e}")),
                    None => {}
                }
            }
            Command::ApplyPhotocurrent(source, color) => match self.pixel.currents {
                Some(cur) if color < 4 => self.set_photocurrent(&source, cur[color]),
                _ => self.warn("Run the pixel optics model first".into()),
            },
            Command::SweepManager => match (self.active.clone(), self.active_cell()) {
                (Some(r), Some(c)) if c.schematic.is_some() => {
                    let setup = c.sim.sweep.clone();
                    self.sweep.show(&r, &setup);
                }
                _ => self.warn("Open a cell with a schematic to sweep".into()),
            },
            Command::RunSweep => self.run_sweep(ctx),
            Command::Verification => {
                let root = self.project.as_ref().and_then(|p| p.root.clone());
                self.verification.show(root.as_deref());
            }
            Command::RunVerification => self.run_verification(ctx),
            Command::MonteCarlo => match (self.active.clone(), self.active_cell()) {
                (Some(r), Some(c)) if c.schematic.is_some() => {
                    let setup = c.sim.montecarlo.clone();
                    self.monte_carlo.show(&r, &setup);
                }
                _ => self.warn("Open a cell with a schematic for Monte Carlo".into()),
            },
            Command::RunMonteCarlo => self.run_monte_carlo(ctx),
            Command::SiliconCorrelation => match (self.active.clone(), self.active_cell()) {
                (Some(r), Some(c)) if c.schematic.is_some() => {
                    let setup = c.sim.silicon.clone();
                    let root = self.project.as_ref().and_then(|p| p.root.clone());
                    self.silicon.show(&r, &setup, root.as_deref());
                }
                _ => self.warn("Open a cell with a schematic to correlate with measurements".into()),
            },
            Command::ProcessEmulation => match (self.active.clone(), self.active_cell(), self.project.as_ref()) {
                (Some(r), Some(c), Some(p)) if c.layout.is_some() => self.process.show(p, &r),
                _ => self.warn("Open a cell with a layout for process emulation".into()),
            },
            Command::ProcessData => self.process_data.show(),
            Command::CompareSilicon => self.run_silicon(ctx, false),
            Command::FitSilicon => self.run_silicon(ctx, true),
            Command::StopMonteCarlo => {
                if !self.monte_carlo.stop() {
                    self.warn("No Monte Carlo is running".into());
                }
            }
            Command::Optimizer => match (self.active.clone(), self.active_cell()) {
                (Some(r), Some(c)) if c.schematic.is_some() => {
                    let setup = c.sim.optimize.clone();
                    self.optimizer.show(&r, &setup);
                }
                _ => self.warn("Open a cell with a schematic to optimize".into()),
            },
            Command::RunOptimizer => {
                if self.optimizer.cell.is_none()
                    && let Some(r) = self.active.clone()
                {
                    let setup = self.active_cell().map(|c| c.sim.optimize.clone()).unwrap_or_default();
                    self.optimizer.show(&r, &setup);
                }
                if let (Some(p), Some(r)) = (self.project.as_ref(), self.optimizer.cell.clone()) {
                    let backend = p.cell(&r).map(|c| c.sim.backend).unwrap_or_default();
                    let opts = self.sim_options(backend);
                    self.optimizer.start(ctx, p, opts);
                    self.info(format!("Optimizer: running on {r}"));
                }
            }
            Command::ResultsBrowser => self.results_open = true,
            Command::CheckSchematic(save) => self.check_schematic(save),
            Command::PlaceCellDialog => {
                if self.active.is_some() {
                    self.dialog = Some(Dialog::PlaceCell { filter: String::new() });
                }
            }
            Command::PlaceCell(r) => self.place_cell(r),
            Command::DescendSchematic(id) => {
                let (Some(here), Some(p)) = (self.active.clone(), self.project.as_ref()) else { return };
                let child =
                    p.cell(&here).and_then(|c| c.schematic.as_ref()).and_then(|s| s.instance(id)).and_then(|i| i.cell.clone());
                match child {
                    Some(c) if p.cell(&c).is_some_and(|c| c.schematic.is_some()) => {
                        self.sch_return.push(here);
                        self.info(format!("Descended into {c} (Shift+B returns)"));
                        self.queue.push(Command::OpenCell(c, Some(Workspace::Schematic)));
                    }
                    Some(c) => self.warn(format!("{c} has no schematic")),
                    None => {}
                }
            }
            Command::ReturnSchematic => match self.sch_return.pop() {
                Some(r) => self.queue.push(Command::OpenCell(r, Some(Workspace::Schematic))),
                None => self.info("Return: no schematic to return to".into()),
            },
            Command::UpdateCellInstances => self.update_cell_instances(),
            Command::EditSymbolDialog => {
                let Some(r) = self.active.clone() else { return };
                match self.project.as_ref().and_then(|p| p.cell(&r)).and_then(|c| c.schematic.as_ref()) {
                    Some(sch) if !sch.ports().is_empty() => {
                        let pins = ics_core::schematic::SymbolSpec::arrange(sch.symbol.as_ref(), &sch.ports());
                        self.dialog = Some(Dialog::EditSymbol { cell: r, pins });
                    }
                    Some(_) => self.warn("The schematic has no pins: mark labels as pins first".into()),
                    None => self.warn(format!("{r} has no schematic")),
                }
            }
            Command::NetBrowser => {
                self.nets.open = true;
                if let Some(r) = self.active_layout()
                    && self.lay_eds.get(&r).is_none_or(|e| e.verify.extraction.is_none())
                {
                    self.queue.push(Command::ExtractNets);
                }
            }
            Command::HierarchyBrowser => {
                self.hier_browser.open = true;
                self.hier_browser.selected = self.active.clone();
            }
            Command::GuardRing | Command::ViaArray | Command::MatchedPlacement => {
                let tab = match cmd {
                    Command::GuardRing => GenTab::GuardRing,
                    Command::ViaArray => GenTab::ViaArray,
                    _ => GenTab::Matching,
                };
                let sel = self.selection_bbox();
                if let Some(p) = &self.project {
                    self.gens.show(tab, sel, &p.tech);
                }
            }
            Command::CalcPlot(expr) => {
                let Some(r) = self.active.clone() else { return };
                let Some(sim) = self.sims.get_mut(&r) else {
                    self.warn(format!("Calculator: simulate {r} first"));
                    return;
                };
                let tab = sim.tab.clone();
                match self.calc.evaluate(sim, &tab, &expr, true) {
                    Ok(v) => self.ok(format!("Calculator: {expr} = {v}")),
                    Err(e) => self.error(format!("Calculator: {e}")),
                }
            }
            Command::Undo | Command::Redo | Command::Delete | Command::FitView => self.editor_command(cmd),
            Command::Sch(a) => {
                if let Some(r) = self.active.clone() {
                    self.workspace = Workspace::Schematic;
                    self.sch_eds.entry(r).or_default().actions.push(a);
                }
            }
            Command::Lay(a) => {
                if let Some(r) = self.active.clone() {
                    self.workspace = Workspace::Layout;
                    self.lay_eds.entry(r).or_default().actions.push(a);
                }
            }
            Command::RunDrc => self.verify(ctx, Verify::Drc),
            Command::ExtractNets => self.verify(ctx, Verify::Extract),
            Command::SchematicFromLayout => self.schematic_from_layout(ctx),
            Command::PixelCircuit => self.pixel_circuit_start(ctx),
            Command::RunLvs => self.verify(ctx, Verify::Lvs),
            Command::IrDrop(net) => {
                if let (Some(r), Some(n)) = (self.active.clone(), net) {
                    self.lay_eds.entry(r).or_default().verify.emir_net = n;
                }
                self.verify(ctx, Verify::Emir);
            }
            Command::RuleDesigner => match &self.project {
                Some(p) => self.rule_designer.show(&p.tech),
                None => self.rule_designer.show_standalone(),
            },
            Command::PCellDesigner => match &self.project {
                Some(p) => {
                    let active = self.active_layout();
                    self.pcell_designer.show(p, active.as_ref());
                }
                None => self.warn("Open a project to design PCells".into()),
            },
            Command::PCellFromActive => match (&self.project, self.active_layout()) {
                (Some(p), Some(r)) => {
                    self.pcell_designer.load(p, &r);
                    self.pcell_designer.show(p, Some(&r));
                }
                _ => self.warn("Open a layout cell to make a PCell from it".into()),
            },
            Command::MosSymbol(body) => {
                self.settings.mos_body_pin = body;
                for ed in self.sch_eds.values_mut() {
                    ed.mos_body_pin = body;
                }
                let how = if body { "with a body pin (4-terminal)" } else { "with the arrow on the source (3-terminal)" };
                self.info(format!("New MOS devices are drawn {how}"));
            }
            Command::SetDeviceRoles(poly, diffusion) => match self.project.as_mut() {
                Some(p) => {
                    let unknown: Vec<&String> =
                        std::iter::once(&poly).chain(&diffusion).filter(|n| p.tech.layer_index(n).is_none()).collect();
                    if !unknown.is_empty() {
                        let list: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
                        self.warn(format!("Not layers of the technology: {}", list.join(", ")));
                    } else if p.tech.roles.poly != poly || p.tech.roles.diffusion != diffusion {
                        p.tech.roles.poly = poly.clone();
                        p.tech.roles.diffusion = diffusion.clone();
                        self.mark_dirty();
                        self.ok(format!("Device layers: gate {poly}, diffusion {}", diffusion.join(", ")));
                    }
                }
                None => self.warn("Open a project first".into()),
            },
            Command::DefaultBulk(pmos, net) => match self.project.as_mut() {
                Some(p) => {
                    let net = Some(net.trim().to_string()).filter(|n| !n.is_empty());
                    let slot = if pmos { &mut p.settings.pmos_bulk } else { &mut p.settings.nmos_bulk };
                    if *slot != net {
                        *slot = net;
                        self.mark_dirty();
                    }
                }
                None => self.warn("Open a project first".into()),
            },
            Command::ImportDrcDeck(path) => self.import_drc_deck(path),
            Command::ExportDrcDeck(path) => self.export_drc_deck(path),
            Command::LinkDrcDeck(path) => self.link_drc_deck(path),
            Command::UnlinkDrcDeck => match self.project.as_mut().map(Project::unlink_drc_deck) {
                Some(Some(path)) => {
                    self.mark_dirty();
                    self.ok(format!(
                        "Unlinked the shared DRC deck {}; the project now stores its deck (save to keep it)",
                        path.display()
                    ));
                }
                Some(None) => self.warn("The technology is not linked to a shared DRC deck".into()),
                None => self.warn("Open a project first".into()),
            },
            Command::ReloadDrcDeck => match self.project.as_mut().map(Project::reload_drc_deck) {
                Some(Ok(true)) => {
                    self.drc_deck_changed();
                    self.ok("Reloaded the shared DRC deck".into());
                }
                Some(Ok(false)) => match self.project.as_ref().and_then(Project::drc_deck_path) {
                    Some(p) => self.info(format!("The shared DRC deck {} is unchanged", p.display())),
                    None => self.warn("The technology is not linked to a shared DRC deck".into()),
                },
                Some(Err(e)) => self.error(format!("Cannot reload the shared DRC deck: {e}")),
                None => self.warn("Open a project first".into()),
            },
            Command::LayerDisplay(layer) => match &self.project {
                Some(p) => self.display.show(&p.tech, layer.as_deref()),
                None => self.warn("Open a project first".into()),
            },
            Command::StreamMap => match &self.project {
                Some(p) => self.stream_map.show(&p.tech),
                None => self.warn("Open a project to edit the stream layer map".into()),
            },
            Command::DummyFill => match &self.project {
                Some(p) => self.fill.show(&p.tech),
                None => self.warn("Open a project to add dummy fill".into()),
            },
            Command::AddFill => self.add_fill(),
            Command::Opc => match &self.project {
                Some(p) => self.opc.show(&p.tech),
                None => self.warn("Open a project to run OPC".into()),
            },
            Command::ApplyOpc => self.apply_opc(),
            Command::RunSignoff(name) => self.run_signoff(ctx, name.as_deref()),
            Command::ImportSignoffResults(path) => self.import_signoff_results(path),
            Command::GenerateLayout => self.generate_layout(),
            Command::RouteFlylines => self.route_flylines(),
            Command::CheckAgainstSource => self.check_against_source(),
            Command::UpdateFromSource(remove) => self.update_from_source(remove),
            Command::ExportGds => self.export_stream(false, StreamFormat::Gds),
            Command::ExportLibraryGds => self.export_stream(true, StreamFormat::Gds),
            Command::ExportOasis => self.export_stream(false, StreamFormat::Oasis),
            Command::ExportLibraryOasis => self.export_stream(true, StreamFormat::Oasis),
            Command::ImportGds(path) => self.import_gds(path, None),
            Command::ImportGdsWithLayerMap(path, map) => {
                let Some(map) = map.or_else(|| {
                    rfd::FileDialog::new()
                        .set_title("Virtuoso layer map")
                        .add_filter("Layer map", &["layermap", "map", "txt"])
                        .pick_file()
                }) else {
                    return;
                };
                self.import_gds(path, Some(map))
            }
            Command::ExportLefDef(def) => self.export_lef_def(def),
            Command::ImportLefDef(def, path) => self.import_lef_def(def, path),
            Command::ImportTech => self.import_tech(),
            Command::ImportCdl(path) => self.import_cdl(path),
            Command::ExportSkill => self.export_skill(),
            Command::ImportVirtuoso(path) => self.import_virtuoso(ctx, path, None),
            Command::ImportVirtuosoLib(path, lib) => self.import_virtuoso(ctx, Some(path), Some(lib)),
            Command::SaveDumpSkill(path) => self.save_dump_skill(path),
            Command::LoadSkill(path) => self.load_skill(path),
            Command::Ciw => {
                self.show_console = true;
                self.bottom_tab = BottomTab::Ciw;
                self.ciw.request_focus();
            }
            Command::Copilot => self.copilot.open = !self.copilot.open,
            Command::AiModels => self.ai_models.open = true,
            Command::CopilotAsk(q) => self.copilot_reply(ctx, crate::copilot::Reply::Text(q)),
            Command::CopilotApprove => self.copilot_reply(ctx, crate::copilot::Reply::Approve),
            Command::CopilotStop => self.copilot.stop(),
            Command::SelectInstance(name) => {
                let Some(r) = self.active.clone() else { return };
                let id = self
                    .project
                    .as_ref()
                    .and_then(|p| p.cell(&r)?.schematic.as_ref()?.instances.iter().find(|i| i.name == name).map(|i| i.id));
                match id {
                    Some(id) => {
                        let ed = self.sch_eds.entry(r).or_default();
                        ed.selection = [SchObj::Instance(id)].into_iter().collect();
                    }
                    None => self.warn(format!("No instance {name} in the schematic")),
                }
            }
            Command::CancelVirtuoso => {
                if let Some(job) = &self.virtuoso_job {
                    job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Command::CancelSkill => {
                if self.skill_job.is_some() {
                    self.ciw.session.cancel();
                }
            }
            Command::CiwEval(src) => {
                self.ciw.push(CiwKind::Input, src.clone());
                self.ciw_eval(&src);
            }
            Command::LoadPdk(path) => {
                if let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter("IC Studio PDK", &["toml"]).pick_file()) {
                    self.load_pdk(&path);
                }
            }
            Command::ExportNetlist => match self.netlist_active() {
                Some((deck, _)) => {
                    let name = self.active.as_ref().map(|r| r.cell.clone()).unwrap_or_default();
                    if let Some(path) = rfd::FileDialog::new().set_file_name(format!("{name}.cir")).save_file() {
                        match std::fs::write(&path, deck) {
                            Ok(()) => self.ok(format!("Wrote netlist {}", path.display())),
                            Err(e) => self.error(format!("Cannot write netlist: {e}")),
                        }
                    }
                }
                None => self.warn("Active cell has no schematic to netlist".into()),
            },
            Command::ToggleTheme => {
                self.settings.dark = !self.settings.dark;
                theme::apply(ctx, self.settings.dark);
            }
            Command::SetLanguage(lang) => {
                self.settings.lang = lang;
                crate::i18n::set(lang);
                ctx.request_repaint();
            }
            Command::BeginnerMode(on) => {
                self.settings.beginner = on;
                self.info(if on {
                    "Beginner mode: specialist tools are hidden from the menus (the command palette still finds them)".to_string()
                } else {
                    "Beginner mode off: every tool is in the menus".to_string()
                });
            }
            Command::ToggleConsole => self.show_console = !self.show_console,
            Command::Settings => self.dialog = Some(Dialog::Settings),
            Command::Shortcuts => self.dialog = Some(Dialog::Shortcuts),
            Command::UserGuide => ctx.open_url(egui::OpenUrl::new_tab(GUIDE_URL)),
            Command::Extensions => self.ext_manager.open = true,
            Command::ReloadExtensions => {
                let reg = match self.extensions.dir.clone() {
                    Some(d) => ics_mcp::ext::Registry::load_from(&d),
                    None => ics_mcp::ext::Registry::load(),
                };
                let usable = reg.extensions.iter().filter(|e| e.usable()).count();
                self.info(format!("{} extension(s) loaded, {usable} enabled", reg.extensions.len()));
                for (p, e) in &reg.errors {
                    self.warn(format!("extension in {}: {e}", p.display()));
                }
                self.set_extensions(reg);
            }
            Command::RunExtension(id, cmd) => {
                let ctx_ = ics_mcp::ext::Context {
                    cell: self.active.as_ref().map(CellRef::to_string),
                    project: self.project.as_ref().and_then(|p| p.root.clone()),
                };
                match self.extensions.invocation(&id, &cmd, &ctx_) {
                    Ok(inv) if self.project.is_none() => {
                        let _ = inv;
                        self.warn(format!("{id}: open a project first"));
                    }
                    Ok(inv) => self.queue_tool(inv.tool, inv.arguments),
                    Err(e) => self.warn(e),
                }
            }
            Command::Tutorial => self.tutorial.start(),
            Command::About => self.dialog = Some(Dialog::About),
            Command::CheckForUpdates => {
                if !matches!(self.updater.state, update::State::Installed { .. }) {
                    self.updater.check(ctx, true);
                }
                self.dialog = Some(Dialog::Update);
            }
            Command::InstallUpdate => {
                self.updater.download(ctx);
                self.dialog = Some(Dialog::Update);
            }
            Command::RestartToUpdate => {
                let update::State::Installed { exe, .. } = &self.updater.state else { return };
                let exe = exe.clone();
                if !self.guard_unsaved(Command::RestartToUpdate) {
                    return;
                }
                match update::relaunch(&exe) {
                    Ok(()) => {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => self.error(e),
                }
            }
            Command::AddMcpToClaudeCode => match update::add_to_claude_code() {
                Ok(m) => self.ok(m),
                Err(e) => {
                    self.error(format!("{e}. Install IC Studio with the installer, or use Copy MCP config and add it by hand."))
                }
            },
            Command::CopyMcpConfig => match update::mcp_config() {
                Ok(c) => {
                    ctx.copy_text(c);
                    self.ok("Copied the MCP server configuration (ics mcp)".into());
                }
                Err(e) => self.error(e),
            },
            Command::CommandPalette => self.palette = Some(PaletteState { query: String::new(), sel: 0 }),
            Command::Quit => {
                if !self.guard_unsaved(Command::Quit) {
                    return;
                }
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn editor_command(&mut self, cmd: Command) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(cell) = p.cell_mut(&r) else { return };
        let mut changed = false;
        match self.workspace {
            Workspace::Schematic => {
                if let Some(sch) = cell.schematic.as_mut() {
                    let ed = self.sch_eds.entry(r.clone()).or_default();
                    changed = match cmd {
                        Command::Undo => ed.undo(sch),
                        Command::Redo => ed.redo(sch),
                        Command::Delete => ed.delete_selection(sch),
                        Command::FitView => {
                            ed.request_fit();
                            false
                        }
                        _ => false,
                    };
                }
            }
            Workspace::Layout => {
                if let Some(l) = cell.layout.as_mut() {
                    let ed = self.lay_eds.entry(r.clone()).or_default();
                    changed = match cmd {
                        Command::Undo => ed.undo(l),
                        Command::Redo => ed.redo(l),
                        Command::Delete => ed.delete_selection(l),
                        Command::FitView => {
                            ed.request_fit();
                            false
                        }
                        _ => false,
                    };
                    if changed {
                        ed.layout_changed();
                        self.hier.invalidate(&r);
                        if Self::sync_opc_layers(p, &mut self.opc_layers) {
                            self.hier.clear();
                        }
                    }
                }
            }
            _ => {}
        }
        if changed {
            self.mark_dirty();
        }
    }

    fn generate_layout(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(sch) = p.circuit(&r).ok().map(|c| c.into_owned()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let plan = ics_core::sdl::plan(&sch, &p.tech);
        let mut lay = p.cell_mut(&r).and_then(|c| c.layout.take()).unwrap_or_default();
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.history.push(&lay);
        let res = ics_core::sdl::apply(p, &mut lay, &plan);
        let pins = if res.is_ok() { ics_core::eco::place_pins(p, &mut lay, &sch) } else { vec![] };
        ed.layout_changed();
        ed.show_flylines = true;
        ed.request_fit();
        if let Some(c) = p.cell_mut(&r) {
            c.layout = Some(lay);
        }
        self.hier.invalidate(&r);
        for s in &plan.skipped {
            self.info(format!("Not placed: {s}"));
        }
        for n in &plan.notes {
            self.info(format!("Matching: {n}"));
        }
        match res {
            Ok(names) if names.is_empty() && pins.is_empty() => self.info("Every schematic device is already placed".into()),
            Ok(names) if names.is_empty() => {
                self.mark_dirty();
                self.ok(format!("Placed pin(s) {}", pins.join(", ")));
            }
            Ok(names) => {
                self.mark_dirty();
                let pins = if pins.is_empty() { String::new() } else { format!("; pins {}", pins.join(", ")) };
                self.ok(format!("Placed {} device(s): {}{pins}", names.len(), names.join(", ")));
            }
            Err(e) => self.error(format!("Generate layout: {e}")),
        }
        self.workspace = Workspace::Layout;
    }

    fn check_against_source(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else { return };
        let Some(sch) = p.cell(&r).and_then(|c| c.schematic.as_ref()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let Some(lay) = p.cell(&r).and_then(|c| c.layout.as_ref()) else {
            self.warn(format!("{r} has no layout; generate it from the schematic first"));
            return;
        };
        let check = ics_core::eco::check(sch, lay, p);
        let summary = check.summary();
        let clean = check.is_clean();
        for i in &check.issues {
            self.info(format!("Check against source: {}", i.message()));
        }
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.verify.source = Some(check);
        ed.verify.source_stale = false;
        self.workspace = Workspace::Layout;
        if clean {
            self.ok(format!("Check against source: {summary}"))
        } else {
            self.warn(format!("Check against source: {summary}"))
        }
    }

    fn update_from_source(&mut self, remove_extra: bool) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(sch) = p.circuit(&r).ok().map(|c| c.into_owned()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let mut lay = p.cell_mut(&r).and_then(|c| c.layout.take()).unwrap_or_default();
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.history.push(&lay);
        let res = ics_core::eco::update(p, &mut lay, &sch, ics_core::eco::UpdateOptions { remove_extra });
        ed.layout_changed();
        ed.show_flylines = true;
        if let Some(c) = p.cell_mut(&r) {
            c.layout = Some(lay);
        }
        self.hier.invalidate(&r);
        match res {
            Ok(rep) => {
                for m in &rep.remaining {
                    self.info(format!("Still to do: {m}"));
                }
                if rep.changed() {
                    self.mark_dirty();
                    self.ok(format!("Update from source: {}", rep.summary()));
                } else {
                    self.info(format!(
                        "Update from source: nothing to change{}",
                        if rep.remaining.is_empty() { "" } else { " (wiring left)" }
                    ));
                }
            }
            Err(e) => self.error(format!("Update from source: {e}")),
        }
        self.workspace = Workspace::Layout;
        self.check_against_source();
    }

    fn route_flylines(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(sch) = p.circuit(&r).ok().map(|c| c.into_owned()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let Some(mut lay) = p.cell_mut(&r).and_then(|c| c.layout.take()) else {
            self.warn(format!("{r} has no layout; generate it from the schematic first"));
            return;
        };
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.history.push(&lay);
        let res = ics_core::route::route(&sch, &mut lay, &*p, &p.tech);
        ed.layout_changed();
        ed.show_flylines = true;
        if let Some(c) = p.cell_mut(&r) {
            c.layout = Some(lay);
        }
        self.hier.invalidate(&r);
        match res {
            Ok(rep) if rep.connections == 0 && rep.failed.is_empty() => self.info("Nothing to route: no flylines".into()),
            Ok(rep) => {
                if rep.connections > 0 {
                    self.mark_dirty();
                }
                let msg = format!("Route: {}", rep.summary());
                if rep.failed.is_empty() { self.ok(msg) } else { self.warn(msg) }
            }
            Err(e) => self.error(format!("Route: {e}")),
        }
        self.workspace = Workspace::Layout;
    }

    /// Cross-probing: keep the selected device names in step between schematic and layout.
    fn sync_probe(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), &self.project) else { return };
        let Some(cell) = p.cell(&r) else { return };
        let (Some(sch), Some(lay)) = (&cell.schematic, &cell.layout) else { return };
        let sch_names: BTreeSet<String> = self.sch_eds.get(&r).map_or_else(BTreeSet::new, |ed| {
            ed.selection
                .iter()
                .filter_map(|o| if let SchObj::Instance(i) = o { sch.instance(*i).map(|x| x.name.clone()) } else { None })
                .collect()
        });
        let lay_names: BTreeSet<String> = self.lay_eds.get(&r).map_or_else(BTreeSet::new, |ed| {
            lay.instances
                .iter()
                .filter(|i| ed.selection.contains(&i.id))
                .filter_map(|i| i.name.as_deref().map(|n| ics_core::sdl::device_of(n).to_string()))
                .collect()
        });
        let (from, names) = match self.workspace {
            Workspace::Schematic => (Workspace::Schematic, sch_names),
            Workspace::Layout => (Workspace::Layout, lay_names),
            _ => return,
        };
        if self.probe.as_ref().is_some_and(|(c, n)| c == &r && n == &names) {
            return;
        }
        self.probe = Some((r.clone(), names.clone()));
        if names.is_empty() {
            return;
        }
        if from == Workspace::Schematic {
            let ids: BTreeSet<u64> = lay
                .instances
                .iter()
                // A device of a placed cell (`X1/M1`) belongs to the schematic instance `X1`.
                .filter(|i| {
                    i.name.as_deref().is_some_and(|n| {
                        let d = ics_core::sdl::device_of(n);
                        names.contains(d) || names.contains(d.split('/').next().unwrap_or(d))
                    })
                })
                .map(|i| i.id)
                .collect();
            if !ids.is_empty() {
                self.lay_eds.entry(r).or_default().selection = ids;
            }
        } else {
            let objs: BTreeSet<SchObj> = sch
                .instances
                .iter()
                .filter(|i| names.contains(&i.name) || names.iter().any(|n| n.split('/').next() == Some(i.name.as_str())))
                .map(|i| SchObj::Instance(i.id))
                .collect();
            if !objs.is_empty() {
                self.sch_eds.entry(r).or_default().selection = objs;
            }
        }
    }

    fn verify(&mut self, ctx: &egui::Context, what: Verify) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(lay) = p.cell_mut(&r).and_then(|c| c.layout.take()) else {
            self.warn(format!("{r} has no layout"));
            return;
        };
        let tech = p.tech.clone();
        let ed = self.lay_eds.entry(r.clone()).or_default();
        let msg = {
            let cx = LayCtx { cell: &r, src: p, hier: &mut self.hier, tech: &tech, cells: &[], context: None };
            ed.start_verification(what, &lay, &cx, ctx)
        };
        if let Some(c) = p.cell_mut(&r) {
            c.layout = Some(lay);
        }
        self.workspace = Workspace::Layout;
        self.info(msg);
    }

    fn pixel_stack_from_pdk(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else {
            self.warn("Open the pixel cell first".into());
            return;
        };
        let Some(xs) = &p.tech.cross_section else {
            self.warn(format!("Technology {} has no cross section", p.tech.name));
            return;
        };
        let boundary = p.tech.layer_index("boundary").map(|_| "boundary");
        match xs.pixel_stack_toml(&p.tech.name, &r.to_string(), boundary) {
            Ok(t) => {
                self.pixel.text = t;
                self.pixel.path = None;
                self.pixel.show();
            }
            Err(e) => self.warn(e),
        }
    }

    /// Set current source `source` of the active cell's schematic to `amps` (undoable).
    /// The device model's photodiode figures into the technology (see
    /// [`ics_core::pixel_layout::apply_device_photodiode`]).
    fn apply_pixel_device(&mut self) {
        let Some(Ok(r)) = &self.pixel.result else {
            self.warn("Run the pixel model with a [device] section first".into());
            return;
        };
        let (Some(d), Some(src), Some(st)) = (&r.device, self.pixel.layout_source.clone(), &self.pixel.stack) else {
            self.warn("Applying to the technology needs a stack with [device] that reads a layout ([layout])".into());
            return;
        };
        let n = d.pixels.len().max(1) as f64;
        let vpin = d.pixels.iter().map(|p| p.pinning_voltage).sum::<f64>() / n;
        let cap = d.pixels.iter().map(|p| p.capacitance).sum::<f64>() / n;
        let pixels = if st.cell_layout { 4 } else { 1 };
        let Some(p) = self.project.as_mut() else { return };
        let cell = match src.cell.split_once('/') {
            Some((lib, cell)) => Some(ics_core::CellRef::new(lib, cell)),
            None => p.all_cells().find(|(_, c)| c.name == src.cell).map(|(r, _)| r),
        };
        let Some(cell) = cell else {
            self.warn(format!("no cell '{}'", src.cell));
            return;
        };
        match ics_core::pixel_layout::apply_device_photodiode(
            p,
            &cell,
            &src.param_list(),
            src.boundary.as_deref(),
            pixels,
            vpin,
            cap,
        ) {
            Ok(msg) => {
                self.mark_dirty();
                self.info(msg);
            }
            Err(e) => self.warn(e),
        }
    }

    fn set_photocurrent(&mut self, source: &str, amps: f64) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else {
            self.warn("Open the pixel circuit's schematic first".into());
            return;
        };
        let Some(sch) = p.cell_mut(&r).and_then(|c| c.schematic.as_mut()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let before = sch.clone();
        let Some(inst) = sch
            .instances
            .iter_mut()
            .find(|i| i.name.eq_ignore_ascii_case(source) && i.kind == ics_core::schematic::DeviceKind::ISource)
        else {
            self.warn(format!("{r} has no current source '{source}'"));
            return;
        };
        let value = format!("{amps:.4e}");
        inst.params.insert("dc".into(), value.clone());
        self.sch_eds.entry(r.clone()).or_default().history.push(&before);
        self.mark_dirty();
        self.info(format!("{r}: {source} = {value} A (photocurrent)"));
    }

    fn import_signoff_results(&mut self, path: Option<PathBuf>) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else {
            self.warn("Open a layout to show sign-off results on".into());
            return;
        };
        let Some(path) =
            path.or_else(|| rfd::FileDialog::new().set_title("Sign-off results (Calibre, KLayout, netgen)").pick_file())
        else {
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return self.warn(format!("{}: {e}", path.display())),
        };
        match ics_core::signoff::import_results(&path, &text, p.tech.dbu_per_um, &r.cell) {
            Ok(rep) => {
                let msg = self.lay_eds.entry(r).or_default().show_signoff(rep);
                self.workspace = Workspace::Layout;
                self.info(msg);
            }
            Err(e) => self.warn(e),
        }
    }

    fn run_signoff(&mut self, ctx: &egui::Context, name: Option<&str>) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else { return };
        if p.tech.signoff.is_empty() {
            self.warn(format!("Technology {} has no sign-off runsets", p.tech.name));
            return;
        }
        if let Some(n) = name {
            match ics_core::signoff::find(&p.tech, n) {
                Ok(rs) => {
                    let i = p.tech.signoff.iter().position(|x| x == rs).unwrap_or(0);
                    self.lay_eds.entry(r).or_default().verify.runset = i;
                }
                Err(e) => {
                    self.warn(e);
                    return;
                }
            }
        }
        self.verify(ctx, Verify::Signoff);
    }

    fn run_simulation(&mut self, ctx: &egui::Context) {
        let Some(r) = self.active.clone() else {
            self.warn("Open a cell to simulate".into());
            return;
        };
        let Some(cell) = self.active_cell() else { return };
        if cell.schematic.is_none() {
            self.warn(format!("{r} has no schematic to simulate"));
            return;
        }
        let analyses: Vec<_> = cell.sim.enabled().cloned().collect();
        if analyses.is_empty() {
            self.warn("No analyses enabled".into());
            return;
        }
        let backend = cell.sim.backend;
        let parasitics = cell.sim.parasitics && cell.layout.is_some();
        let (mut deck, warnings) = self.netlist_active().unwrap();
        if deck.is_empty() {
            // The hierarchy could not be flattened (a missing cell, a cell placing itself).
            self.error(format!("Cannot simulate {r}: {}", warnings.join("; ")));
            return;
        }
        for w in warnings {
            self.warn(format!("netlist: {w}"));
        }
        if parasitics && let Some(p) = &self.project {
            match ics_core::pex::post_layout(p, &r) {
                Ok(pl) => {
                    deck = pl.apply(&deck);
                    self.info(pl.summary);
                }
                Err(e) => self.warn(format!("Parasitics not included: {e}")),
            }
        }
        let opts = self.sim_options(backend);
        self.info(format!(
            "Simulating {r} with {} ({})",
            backend.label(),
            analyses.iter().map(|a| a.kind_name()).collect::<Vec<_>>().join(", ")
        ));
        self.sims.entry(r).or_default().start(ctx, deck, analyses, opts);
        self.workspace = Workspace::Simulation;
    }

    fn run_sweep(&mut self, ctx: &egui::Context) {
        if self.sweep.is_running() {
            self.warn("A sweep is already running".into());
            return;
        }
        let Some(r) = self.active.clone() else { return };
        if self.sweep.cell.as_ref() != Some(&r)
            && let Some(c) = self.active_cell()
        {
            let s = c.sim.sweep.clone();
            self.sweep.show(&r, &s);
            self.sweep.open = false;
        }
        let mut notes = vec![];
        match self.sweep_decks(&r, &mut notes) {
            Ok((decks, analyses, backend)) => {
                for n in notes {
                    self.warn(n);
                }
                let n = decks.len();
                let opts = self.sim_options(backend);
                let mc = &self.sweep.setup.monte_carlo;
                self.sweep.plan = match (mc.runs > 0, self.active_cell().and_then(|c| c.schematic.as_ref())) {
                    (true, Some(sch)) => {
                        ics_core::sweep::mc_plan(mc, sch, self.project.as_ref().and_then(|p| p.tech.variation.as_ref())).ok()
                    }
                    _ => None,
                };
                self.sweep.start(ctx, decks, analyses, opts);
                self.info(format!("Sweep {r}: {n} point(s) with {}", backend.label()));
            }
            Err(e) => {
                for n in notes {
                    self.warn(n);
                }
                self.error(format!("Sweep: {e}"));
            }
        }
    }

    fn run_monte_carlo(&mut self, ctx: &egui::Context) {
        if self.monte_carlo.cell.is_none()
            && let (Some(r), Some(c)) = (self.active.clone(), self.active_cell())
        {
            let setup = c.sim.montecarlo.clone();
            self.monte_carlo.show(&r, &setup);
        }
        let Some(r) = self.monte_carlo.cell.clone() else {
            self.warn("Open a cell with a schematic for Monte Carlo".into());
            return;
        };
        let mut notes = vec![];
        self.monte_carlo.process = self.project.as_ref().and_then(|p| p.tech.variation.clone());
        self.monte_carlo.mismatch = self.project.as_ref().and_then(|p| p.tech.mismatch.clone());
        let res = self.mc_deck(&r, &mut notes).and_then(|(deck, analyses)| self.monte_carlo.start(ctx, deck, analyses));
        for n in notes {
            self.warn(n);
        }
        match res {
            Ok(()) => self.info(format!("Monte Carlo {r}: {} run(s) on the built-in engine", self.monte_carlo.setup.runs)),
            Err(e) => {
                self.monte_carlo.error = Some(e.clone());
                self.error(format!("Monte Carlo: {e}"));
            }
        }
    }

    /// The netlist of cell `r` (with layout parasitics when its setup asks) and its enabled analyses.
    fn mc_deck(&self, r: &CellRef, notes: &mut Vec<String>) -> Result<(String, Vec<ics_core::Analysis>), String> {
        let p = self.project.as_ref().ok_or("no project")?;
        let cell = p.cell(r).ok_or_else(|| format!("no cell {r}"))?;
        let circuit = p.circuit(r)?;
        let sch = &*circuit;
        let analyses: Vec<_> = cell.sim.enabled().cloned().collect();
        if analyses.is_empty() {
            return Err("no analyses enabled".into());
        }
        let nl = ics_core::netlist::netlist_for(sch, &p.tech, &cell.sim.corner);
        notes.extend(nl.warnings.iter().map(|w| format!("netlist: {w}")));
        let mut deck = nl.deck(&r.to_string(), &cell.sim);
        if cell.sim.parasitics && cell.layout.is_some() {
            match ics_core::pex::post_layout(p, r) {
                Ok(pl) => deck = pl.apply(&deck),
                Err(e) => notes.push(format!("Parasitics not included: {e}")),
            }
        }
        Ok((deck, analyses))
    }

    fn run_verification(&mut self, ctx: &egui::Context) {
        if self.verification.is_running() {
            return self.warn("Verification is already running".into());
        }
        let Some(p) = self.project.as_ref() else { return };
        let tests = p.settings.verification.resolve(p);
        if tests.is_empty() {
            return self.warn("No tests: give a cell specs in its Corner / Sweep Manager, or add cells to the plan".into());
        }
        let root = p.root.clone();
        let snapshot = p.clone();
        let opts = self.sim_options(ics_core::Backend::Auto);
        if !self.verification.open {
            self.verification.show(root.as_deref());
        }
        let names: Vec<&str> = tests.iter().map(|(n, _)| n.as_str()).collect();
        self.info(format!("Verification: running {} test(s): {}", tests.len(), names.join(", ")));
        self.verification.start(ctx, snapshot, tests, opts);
    }

    fn verification_window(&mut self, ctx: &egui::Context) {
        let active = self.active.clone();
        match self.verification.window(ctx, self.project.as_ref(), active.as_ref()) {
            Some(crate::verify_view::VerifyAction::Plan(plan)) => {
                if let Some(p) = self.project.as_mut() {
                    p.settings.verification = plan;
                    self.mark_dirty();
                }
            }
            Some(crate::verify_view::VerifyAction::Run) => self.queue.push(Command::RunVerification),
            None => {}
        }
    }

    pub fn verification(&mut self) -> &mut crate::verify_view::Verification {
        &mut self.verification
    }

    fn monte_carlo_window(&mut self, ctx: &egui::Context) {
        if self.monte_carlo.open {
            let v = self.project.as_ref().and_then(|p| p.tech.variation.as_ref());
            if self.monte_carlo.process.as_ref() != v {
                self.monte_carlo.process = v.cloned();
            }
            let m = self.project.as_ref().and_then(|p| p.tech.mismatch.as_ref());
            if self.monte_carlo.mismatch.as_ref() != m {
                self.monte_carlo.mismatch = m.cloned();
            }
        }
        let (changed, run, stop) = self.monte_carlo.window(ctx);
        if let Some(s) = changed
            && let Some(c) = self.monte_carlo.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r))
        {
            c.sim.montecarlo = s;
            self.mark_dirty();
        }
        if run {
            self.queue.push(Command::RunMonteCarlo);
        }
        if stop {
            self.queue.push(Command::StopMonteCarlo);
        }
    }

    pub fn monte_carlo(&mut self) -> &mut crate::mc_view::MonteCarlo {
        &mut self.monte_carlo
    }

    pub fn silicon(&mut self) -> &mut crate::silicon_view::SiliconView {
        &mut self.silicon
    }

    fn run_silicon(&mut self, ctx: &egui::Context, fit: bool) {
        let Some(p) = self.project.clone() else { return };
        let backend = self.silicon.cell.as_ref().and_then(|r| p.cell(r)).map_or(ics_core::Backend::Builtin, |c| c.sim.backend);
        let opts = self.sim_options(backend);
        match self.silicon.start(ctx, p, fit, opts) {
            Ok(()) => self.info(format!("Silicon correlation: {}", if fit { "fitting" } else { "comparing" })),
            Err(e) => {
                self.silicon.error = Some(e.clone());
                self.error(format!("Silicon correlation: {e}"));
            }
        }
    }

    pub fn process_emulation(&mut self) -> &mut crate::process_view::ProcessView {
        &mut self.process
    }

    fn process_window(&mut self, ctx: &egui::Context) {
        if let Some(crate::process_view::ProcessAction::Load) = self.process.window(ctx, self.project.as_ref())
            && let Some(path) = rfd::FileDialog::new().set_title("Process flow").add_filter("Process flow", &["toml"]).pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(t) => self.process.set_text(t),
                Err(e) => self.error(format!("{}: {e}", path.display())),
            }
        }
    }

    pub fn process_data(&mut self) -> &mut crate::process_data_view::ProcessDataView {
        &mut self.process_data
    }

    fn process_data_window(&mut self, ctx: &egui::Context) {
        use crate::process_data_view::ProcessDataAction;
        let action = self.process_data.window(ctx);
        match &action {
            Some(ProcessDataAction::ExtractCorners) => {
                let p = self.project.clone().unwrap_or_else(ics_core::examples::demo_project);
                match self.process_data.start_corners(ctx, p) {
                    Ok(()) => self.info("Corner models: fitting".into()),
                    Err(e) => self.error(format!("Corner models: {e}")),
                }
                return;
            }
            Some(ProcessDataAction::ApplyVariation(v)) => {
                let Some(p) = self.project.as_mut() else {
                    self.warn("Open a project to store the process variation in its technology".into());
                    return;
                };
                p.tech.variation = Some(v.clone());
                self.mark_dirty();
                self.ok(format!(
                    "Process variation: {} parameter(s) from {} stored in the technology; Monte Carlo draws them",
                    v.params.len(),
                    v.source
                ));
                return;
            }
            Some(ProcessDataAction::ApplyMismatch(m)) => {
                let Some(p) = self.project.as_mut() else {
                    self.warn("Open a project to store the mismatch in its technology".into());
                    return;
                };
                p.tech.mismatch = Some(m.clone());
                self.mark_dirty();
                let txt: Vec<String> = m.devices.iter().map(|d| format!("{} Avt {:.2} mV·µm", d.device, d.avt * 1e9)).collect();
                self.ok(format!("Mismatch: {} stored in the technology; Monte Carlo draws it", txt.join(", ")));
                return;
            }
            Some(ProcessDataAction::LoadPairs) => {
                let mut dialog = rfd::FileDialog::new().set_title("Matched-pair data").add_filter("CSV", &["csv", "txt", "tsv"]);
                if let Some(r) = self.project.as_ref().and_then(|p| p.root.clone()) {
                    dialog = dialog.set_directory(r);
                }
                if let Some(path) = dialog.pick_file() {
                    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    match std::fs::read_to_string(&path) {
                        Ok(t) => self.process_data.set_pairs_text(&t, &name),
                        Err(e) => self.error(format!("{}: {e}", path.display())),
                    }
                }
                return;
            }
            Some(ProcessDataAction::SaveLibrary(text)) => {
                if let Some(path) = rfd::FileDialog::new().set_title("Corner library").set_file_name("corners.lib").save_file() {
                    match std::fs::write(&path, text) {
                        Ok(()) => self.ok(format!("Corner models: wrote {}", path.display())),
                        Err(e) => self.error(format!("{}: {e}", path.display())),
                    }
                }
                return;
            }
            _ => {}
        }
        let (title, filter, exts): (&str, &str, &[&str]) = match action {
            Some(ProcessDataAction::LoadData) => ("WAT / PCM data", "CSV", &["csv", "txt", "tsv"]),
            Some(ProcessDataAction::LoadSpec) => ("Process specification", "TOML", &["toml"]),
            _ => return,
        };
        let mut dialog = rfd::FileDialog::new().set_title(title).add_filter(filter, exts);
        if let Some(r) = self.project.as_ref().and_then(|p| p.root.clone()) {
            dialog = dialog.set_directory(r);
        }
        let Some(path) = dialog.pick_file() else { return };
        let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        match std::fs::read_to_string(&path) {
            Ok(t) if matches!(action, Some(ProcessDataAction::LoadData)) => self.process_data.set_data_text(&t, &name),
            Ok(t) => self.process_data.set_spec_text(&t, &name),
            Err(e) => self.error(format!("{}: {e}", path.display())),
        }
    }

    fn silicon_window(&mut self, ctx: &egui::Context) {
        use crate::silicon_view::SiliconAction;
        match self.silicon.window(ctx) {
            Some(SiliconAction::Changed(s)) => {
                if let Some(c) = self.silicon.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                    c.sim.silicon = s;
                    self.mark_dirty();
                }
            }
            Some(SiliconAction::LoadData) => {
                let root = self.project.as_ref().and_then(|p| p.root.clone());
                let mut dialog = rfd::FileDialog::new().set_title("Measured data").add_filter("CSV", &["csv", "txt", "tsv"]);
                if let Some(r) = &root {
                    dialog = dialog.set_directory(r);
                }
                if let Some(path) = dialog.pick_file() {
                    let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).map_or(path.clone(), |p| p.to_path_buf());
                    self.silicon.setup.data = rel.to_string_lossy().into_owned();
                    match self.silicon.load_data(root.as_deref()) {
                        Ok(()) => {
                            let s = self.silicon.setup.clone();
                            if let Some(c) = self.silicon.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                                c.sim.silicon = s;
                                self.mark_dirty();
                            }
                        }
                        Err(e) => {
                            self.silicon.error = Some(e.clone());
                            self.error(e);
                        }
                    }
                }
            }
            Some(SiliconAction::Compare) => self.queue.push(Command::CompareSilicon),
            Some(SiliconAction::Fit) => self.queue.push(Command::FitSilicon),
            Some(SiliconAction::Apply(r, values)) => {
                if let Some(sch) = self.project.as_mut().and_then(|p| p.cell_mut(&r)).and_then(|c| c.schematic.as_mut()) {
                    for (t, x) in &values {
                        if let Some((inst, param)) = t.split_once('.')
                            && let Some(i) = sch.instances.iter_mut().find(|i| i.name.eq_ignore_ascii_case(inst))
                        {
                            i.params.insert(param.to_ascii_lowercase(), ics_core::optimize::spice_number(*x));
                        }
                    }
                    self.mark_dirty();
                    self.ok(format!("Silicon correlation: applied {} parameter(s) to {r}", values.len()));
                }
            }
            None => {}
        }
    }

    /// The decks of every point of the sweep draft on cell `r`.
    #[allow(clippy::type_complexity)]
    fn sweep_decks(
        &self,
        r: &CellRef,
        notes: &mut Vec<String>,
    ) -> Result<(Vec<(ics_core::sweep::SweepPoint, String)>, Vec<ics_core::Analysis>, ics_core::Backend), String> {
        let p = self.project.as_ref().ok_or("no project")?;
        let cell = p.cell(r).ok_or_else(|| format!("no cell {r}"))?;
        let circuit = p.circuit(r)?;
        let sch = &*circuit;
        let mut setup = cell.sim.clone();
        setup.sweep = self.sweep.setup.clone();
        let analyses: Vec<_> = setup.enabled().cloned().collect();
        if analyses.is_empty() {
            return Err("no analyses enabled".into());
        }
        let points = setup.sweep.expand(&setup, sch, p.tech.variation.as_ref())?;
        let pex = if setup.parasitics && cell.layout.is_some() {
            match ics_core::pex::post_layout(p, r) {
                Ok(pl) => Some(pl),
                Err(e) => {
                    notes.push(format!("Parasitics not included: {e}"));
                    None
                }
            }
        } else {
            None
        };
        let mut decks = Vec::with_capacity(points.len());
        for (k, pt) in points.into_iter().enumerate() {
            let (deck, w) = ics_core::sweep::point_deck(sch, &p.tech, &r.to_string(), &setup, &pt)?;
            if k == 0 {
                notes.extend(w.into_iter().map(|w| format!("netlist: {w}")));
            }
            let deck = match &pex {
                Some(pl) => pl.apply(&deck),
                None => deck,
            };
            decks.push((pt, deck));
        }
        Ok((decks, analyses, setup.backend))
    }

    fn sweep_window(&mut self, ctx: &egui::Context) {
        match self.sweep.window(ctx, self.project.as_ref()) {
            Some(SweepAction::Changed(s)) => {
                let r = self.sweep.cell.clone();
                if let Some(c) = r.and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                    c.sim.sweep = s;
                    self.mark_dirty();
                }
            }
            Some(SweepAction::Run) => {
                if let Some(r) = self.sweep.cell.clone() {
                    self.queue.push(Command::OpenCell(r, None));
                    self.queue.push(Command::RunSweep);
                }
            }
            None => {}
        }
    }

    pub fn sweep_manager(&mut self) -> &mut SweepManager {
        &mut self.sweep
    }

    fn export_stream(&mut self, library: bool, format: StreamFormat) {
        let Some(p) = &self.project else { return };
        let tops: Vec<CellRef> = if library {
            p.all_cells().filter(|(_, c)| c.layout.is_some()).map(|(r, _)| r).collect()
        } else {
            match (self.active.as_ref(), self.active_cell().and_then(|c| c.layout.as_ref())) {
                (Some(r), Some(_)) => vec![r.clone()],
                _ => {
                    self.warn("Active cell has no layout".into());
                    return;
                }
            }
        };
        if tops.is_empty() {
            self.warn("No layouts to export".into());
            return;
        }
        let stem = if library { &p.name } else { &tops[0].cell };
        let filter: &[&str] = if format == StreamFormat::Oasis { &["oas", "oasis"] } else { &["gds", "gds2"] };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}.{}", format.extension()))
            .add_filter(format.label(), filter)
            .save_file()
        else {
            return;
        };
        let n = tops.iter().flat_map(|t| ics_core::hier::descendants(p, t)).collect::<std::collections::HashSet<_>>().len();
        let bytes = ics_core::gds::write_stream_hier(format, &p.name, p, &tops, &ics_core::stream_map::export_tech(&p.tech));
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.ok(format!("Exported {n} cell(s) to {} ({} bytes)", path.display(), bytes.len())),
            Err(e) => self.error(format!("{} export failed: {e}", format.label())),
        }
    }

    /// Export library `lib`: its top cells and the hierarchy below them, which may include
    /// cells of other libraries (PCells, for example).
    fn export_library_stream(&mut self, lib: &str, format: StreamFormat) {
        let Some(p) = &self.project else { return };
        let tops = p.library_tops(lib);
        if tops.is_empty() {
            self.warn(format!("Library '{lib}' has no layouts to export"));
            return;
        }
        let filter: &[&str] = if format == StreamFormat::Oasis { &["oas", "oasis"] } else { &["gds", "gds2"] };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{lib}.{}", format.extension()))
            .add_filter(format.label(), filter)
            .save_file()
        else {
            return;
        };
        let n = tops.iter().flat_map(|t| ics_core::hier::descendants(p, t)).collect::<std::collections::HashSet<_>>().len();
        let bytes = ics_core::gds::write_stream_hier(format, lib, p, &tops, &ics_core::stream_map::export_tech(&p.tech));
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.ok(format!("Exported {n} cell(s) of '{lib}' to {} ({} bytes)", path.display(), bytes.len())),
            Err(e) => self.error(format!("{} export failed: {e}", format.label())),
        }
    }

    /// After library `old` became `new` (or was deleted, `None`): retarget or drop the open
    /// tabs, editors and results of its cells.
    fn library_changed(&mut self, old: &str, new: Option<&str>) {
        let map = |r: &CellRef| new.map(|n| CellRef::new(n, &r.cell));
        self.hier.clear();
        self.lay_return.clear();
        self.probe = None;
        self.tabs =
            std::mem::take(&mut self.tabs).into_iter().filter_map(|t| if t.lib == old { map(&t) } else { Some(t) }).collect();
        if let Some(a) = self.active.clone().filter(|a| a.lib == old) {
            self.active = map(&a).or_else(|| self.tabs.first().cloned());
        }
        fn remap<V>(m: &mut HashMap<CellRef, V>, old: &str, new: Option<&str>) {
            for k in m.keys().filter(|k| k.lib == old).cloned().collect::<Vec<_>>() {
                let v = m.remove(&k).expect("key exists");
                if let Some(n) = new {
                    m.insert(CellRef::new(n, &k.cell), v);
                }
            }
        }
        remap(&mut self.sch_eds, old, new);
        remap(&mut self.lay_eds, old, new);
        remap(&mut self.sims, old, new);
        if self.selected_lib.as_deref() == Some(old) {
            self.selected_lib = new.map(str::to_string);
        }
    }

    fn import_gds(&mut self, path: Option<PathBuf>, layer_map: Option<PathBuf>) {
        if self.project.is_none() {
            return;
        }
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("GDSII / OASIS", &["gds", "gds2", "gdsii", "oas", "oasis"])
                .add_filter("GDSII", &["gds", "gds2", "gdsii"])
                .add_filter("OASIS", &["oas", "oasis"])
                .pick_file()
        }) else {
            return;
        };
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                self.error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        let map_text = match layer_map.as_ref().map(std::fs::read_to_string).transpose() {
            Ok(t) => t,
            Err(e) => {
                self.error(format!(
                    "Cannot read {}: {e}",
                    layer_map.as_ref().map_or_else(String::new, |m| m.display().to_string())
                ));
                return;
            }
        };
        let Some(p) = self.project.as_mut() else { return };
        let t0 = Instant::now();
        // The map is applied to a copy, kept only when the import succeeds.
        let (mut mapped, mut map_info, mut map_warnings) = (None, None, vec![]);
        if let (Some(text), Some(m)) = (&map_text, &layer_map) {
            let mut tech = p.tech.clone();
            let r = ics_core::stream_map::apply_layer_map(&mut tech, text);
            let name = m.file_name().unwrap_or_default().to_string_lossy().to_string();
            map_warnings.extend(r.warnings.into_iter().map(|w| format!("{name}: {w}")));
            let mut info = format!("Layer map {name}: {} row(s) applied", r.rows);
            if !r.added_layers.is_empty() {
                info += &format!("; added layer(s) {}", r.added_layers.join(", "));
            }
            map_info = Some(info);
            mapped = Some(tech);
        }
        let opts = ics_core::gds::ReadOptions { add_unknown_layers: true, use_stream_map: true };
        let label = StreamFormat::detect(&data).label();
        let imp = match ics_core::gds::read_stream_with(&data, mapped.as_ref().unwrap_or(&p.tech), "gds", opts) {
            Ok(imp) => imp,
            Err(e) => {
                self.error(format!("{label} import failed: {e}"));
                return;
            }
        };
        let skipped = imp.skipped_unknown_layers;
        let pairs = |m: &std::collections::BTreeMap<(u16, u16), usize>| {
            let mut v: Vec<String> = m.iter().take(12).map(|((l, d), n)| format!("{l}/{d} ({n})")).collect();
            if m.len() > 12 {
                v.push(format!("and {} more", m.len() - 12));
            }
            v.join(", ")
        };
        let skipped_pairs = pairs(&imp.skipped_pairs);
        let fallbacks = (!imp.datatype_fallbacks.is_empty())
            .then(|| (imp.datatype_fallbacks.values().sum::<usize>(), pairs(&imp.datatype_fallbacks)));
        let added = imp.new_layers.len();
        let warnings = imp.warnings.clone();
        let n = imp.cells.len();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let original = mapped.map(|t| std::mem::replace(&mut p.tech, t));
        match p.add_gds_library(&stem, imp) {
            Ok((lib, tops)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(format!(
                    "Imported {n} cell(s) from {} into library '{lib}' in {:.2} s",
                    path.display(),
                    t0.elapsed().as_secs_f64()
                ));
                if let Some(info) = map_info {
                    self.info(info);
                }
                for w in map_warnings {
                    self.warn(w);
                }
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
                if skipped > 0 {
                    self.warn(format!(
                        "{skipped} shape(s) on layers not in the technology were skipped: {skipped_pairs}. Add the \
                         layers to the technology or import with unknown layers added"
                    ));
                }
                if let Some((n, list)) = fallbacks {
                    self.warn(format!(
                        "{n} shape(s) on datatypes the technology lacks were read onto datatype 0 of their layer: {list}. \
                         Pin, label or blockage datatypes then sit on the drawing layer"
                    ));
                }
                if added > 0 {
                    self.warn(format!("{added} layer(s) not in the technology were added to it"));
                }
                if let Some(r) = tops.into_iter().next() {
                    self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
                }
            }
            Err(e) => {
                if let Some(t) = original {
                    p.tech = t;
                }
                self.error(format!("{label} import failed: {e}"))
            }
        }
    }

    fn export_lef_def(&mut self, def: bool) {
        let Some(p) = &self.project else { return };
        let (label, ext) = if def { ("DEF", "def") } else { ("LEF", "lef") };
        let Some(r) = self.active.clone().filter(|_| self.active_cell().is_some_and(|c| c.layout.is_some())) else {
            self.warn("Active cell has no layout".into());
            return;
        };
        let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.{ext}", r.cell)).add_filter(label, &[ext]).save_file()
        else {
            return;
        };
        let res = if def {
            ics_core::lefdef::export_def(p, &r).map(|(text, d, w)| {
                (text, format!("{} component(s), {} pin(s), {} net(s)", d.components.len(), d.pins.len(), d.nets.len()), w)
            })
        } else {
            ics_core::lefdef::export_lef(p, std::slice::from_ref(&r), &Default::default())
                .map(|(text, w)| (text, format!("the abstract of {}", r.cell), w))
        };
        match res.and_then(|(text, what, w)| std::fs::write(&path, text).map(|_| (what, w)).map_err(|e| e.to_string())) {
            Ok((what, warnings)) => {
                self.ok(format!("Exported {what} to {}", path.display()));
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
            }
            Err(e) => self.error(format!("{label} export failed: {e}")),
        }
    }

    fn import_lef_def(&mut self, def: bool, path: Option<PathBuf>) {
        let Some(p) = self.project.as_mut() else { return };
        let (label, ext) = if def { ("DEF", "def") } else { ("LEF", "lef") };
        let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter(label, &[ext]).pick_file()) else {
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        let res = if def {
            ics_core::lefdef::import_def(p, &text, None).map(|i| {
                let msg = format!("Imported {} from {} ({} component(s) placed)", i.cell, path.display(), i.placed);
                (Some(i.cell), msg, i.warnings)
            })
        } else {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            ics_core::lefdef::import_lef(p, &text, &stem).map(|i| {
                let msg = format!("Imported {} macro(s) from {} into library '{}'", i.cells.len(), path.display(), i.lib);
                (i.cells.first().cloned(), msg, i.warnings)
            })
        };
        match res {
            Ok((open, msg, warnings)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(msg);
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
                if let Some(r) = open {
                    self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
                }
            }
            Err(e) => self.error(format!("{label} import failed: {e}")),
        }
    }

    fn load_pdk(&mut self, path: &Path) {
        let Some(p) = self.project.as_mut() else { return };
        let loaded = ics_core::pdk::load(path).and_then(|pdk| {
            let w = p.set_tech(pdk.tech)?;
            let t = &p.tech;
            let msg = format!(
                "PDK '{}' loaded from {} ({} layers, {} devices)",
                t.name,
                path.display(),
                t.layers.len(),
                t.devices.len()
            );
            Ok((msg, pdk.warnings, w))
        });
        match loaded {
            Ok((msg, pdk_warnings, warnings)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(msg);
                for w in pdk_warnings.into_iter().chain(warnings) {
                    self.warn(format!("PDK: {w}"));
                }
            }
            Err(e) => self.error(format!("Cannot load PDK: {e}")),
        }
    }

    /// Load the active layout into the Pixel Circuit window and characterise it in the background.
    fn pixel_circuit_start(&mut self, ctx: &egui::Context) {
        let (Some(r), Some(p)) = (self.active.as_ref(), self.project.as_ref()) else {
            return self.warn("Open a layout cell to find its pixels".into());
        };
        self.pixel_circuit.load(p, r);
        self.pixel_circuit.start(Some(ctx), p);
    }

    fn schematic_from_layout(&mut self, ctx: &egui::Context) {
        use ics_core::pixel_circuit::{Testbench, classify, schematic};
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(lay) = p.cell(&r).and_then(|c| c.layout.as_ref()) else {
            return self.warn(format!("{r} has no layout"));
        };
        let x = ics_core::extract::extract(lay, p, &p.tech);
        let pc = classify(&x);
        let bench = Testbench::for_tech(&x, &p.tech);
        let (sch, sim) = schematic(&x, &pc, Some(&bench));
        // Into the cell itself when it has no schematic (ready for LVS), else a new cell.
        let target = if p.cell(&r).is_some_and(|c| c.schematic.is_none()) {
            r.clone()
        } else {
            let base = format!("{}_from_layout", r.cell);
            let mut name = base.clone();
            let mut k = 1;
            while p.cell(&CellRef::new(&r.lib, &name)).is_some() {
                name = format!("{base}_{k}");
                k += 1;
            }
            match p.add_cell(&r.lib, ics_core::Cell::new(&name)) {
                Ok(t) => t,
                Err(e) => return self.error(e),
            }
        };
        let c = p.cell_mut(&target).expect("target cell");
        self.sch_eds.entry(target.clone()).or_default().history.push(c.schematic.as_ref().unwrap_or(&Default::default()));
        c.schematic = Some(sch);
        c.sim = sim;
        self.hier.clear();
        self.mark_dirty();
        let pixels = pc.pixels.len();
        self.ok(format!(
            "Schematic from the layout of {r}: {} transistor(s), {} photodiode(s), {pixels} pixel(s) -> {target}",
            x.devices.len(),
            x.photodiodes.len()
        ));
        for w in x.warnings.iter().chain(&pc.warnings) {
            self.warn(format!("{r}: {w}"));
        }
        // The pixels' characterisation runs in the background in the Pixel Circuit window.
        if pixels > 0
            && let Some(p) = self.project.as_ref()
        {
            self.pixel_circuit.load(p, &r);
            self.pixel_circuit.start(Some(ctx), p);
        }
        self.queue.push(Command::OpenCell(target, Some(Workspace::Schematic)));
    }

    fn import_cdl(&mut self, path: Option<PathBuf>) {
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new().add_filter("CDL / SPICE netlist", &["cdl", "cir", "sp", "spi", "net"]).pick_file()
        }) else {
            return;
        };
        let Some(p) = self.project.as_mut() else { return };
        // Subcircuits placing each other keep their hierarchy: a cell per subcircuit.
        if let Ok(text) = std::fs::read_to_string(&path)
            && ics_core::cadence::cdl_is_hierarchical(&text)
        {
            // Into a new library named after the file, so no existing cell is touched.
            let stem: String = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .collect();
            let stem = if stem.is_empty() || stem.starts_with('-') { "cdl".to_string() } else { stem };
            let mut lib = stem.clone();
            let mut k = 1;
            while p.libraries.iter().any(|l| l.name.eq_ignore_ascii_case(&lib)) {
                lib = format!("{stem}_{k}");
                k += 1;
            }
            match ics_core::cadence::import_cdl_hier(&text, &p.tech, None, &lib).and_then(|h| {
                let cells = h.add_to(p, &lib, false)?;
                Ok((h, cells))
            }) {
                Ok((h, cells)) => {
                    self.hier.clear();
                    self.mark_dirty();
                    let names: Vec<String> = cells.iter().map(|r| r.cell.clone()).collect();
                    let lib = cells.last().map_or(String::new(), |r| r.lib.clone());
                    self.ok(format!(
                        "CDL: {} cell(s) imported with their hierarchy into library {lib}, top {}: {}",
                        cells.len(),
                        h.top,
                        names.join(", ")
                    ));
                    for w in h.warnings.iter().chain(h.cells.iter().flat_map(|c| &c.imported.warnings)) {
                        self.warn(format!("CDL: {w}"));
                    }
                    if let Some(top) = cells.last() {
                        self.queue.push(Command::OpenCell(top.clone(), Some(Workspace::Schematic)));
                    }
                    return;
                }
                Err(e) => self.warn(format!("CDL: hierarchy not kept ({e}); importing it flat")),
            }
        }
        let Some(p) = self.project.as_mut() else { return };
        let res = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| ics_core::cadence::import_cdl(&t, &p.tech, None));
        let imp = match res {
            Ok(i) => i,
            Err(e) => return self.error(format!("Cannot import {}: {e}", path.display())),
        };
        let base = if imp.flat.top.is_empty() {
            path.file_stem().unwrap_or_default().to_string_lossy().to_string()
        } else {
            imp.flat.top.clone()
        };
        if p.library_mut("work").is_none()
            && let Err(e) = p.add_library("work")
        {
            return self.error(e);
        }
        // A free cell name.
        let mut name = base.clone();
        let mut k = 1;
        while p.cell(&CellRef::new("work", &name)).is_some() {
            name = format!("{base}_{k}");
            k += 1;
        }
        let mut cell = ics_core::Cell::new(&name);
        cell.schematic = Some(imp.imported.schematic);
        match p.add_cell("work", cell) {
            Ok(r) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(format!(
                    "CDL: {} device(s) of {} imported as {r} (subcircuits: {})",
                    imp.flat.devices,
                    if imp.flat.top.is_empty() { "the top level" } else { &imp.flat.top },
                    imp.subckts.join(", ")
                ));
                for w in imp.imported.warnings {
                    self.warn(format!("CDL: {w}"));
                }
                self.queue.push(Command::OpenCell(r, Some(Workspace::Schematic)));
            }
            Err(e) => self.error(e),
        }
    }

    fn import_virtuoso(&mut self, ctx: &egui::Context, path: Option<PathBuf>, lib: Option<String>) {
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new().add_filter("Virtuoso library dump or cds.lib", &["icsdump", "txt", "lib"]).pick_file()
        }) else {
            return;
        };
        // A cds.lib names several libraries: ask which one.
        let is_cds = path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("cds.lib"));
        if lib.is_none() && is_cds {
            match ics_core::virtuoso_batch::parse_cds_lib(&path) {
                Ok(libs) => {
                    let found = ics_core::virtuoso_batch::find_virtuoso().is_some();
                    self.virtuoso_pick = Some((path, libs.into_iter().map(|(n, _)| n).collect(), found));
                }
                Err(e) => self.error(format!("Cannot read {}: {e}", path.display())),
            }
            return;
        }
        if self.project.is_none() {
            return;
        }
        let is_dump = matches!(
            ics_core::virtuoso_batch::resolve_source(&path, lib.as_deref()),
            Ok(ics_core::virtuoso_batch::Source::Dump(_))
        );
        if !is_dump && ics_core::virtuoso_batch::find_virtuoso().is_some() {
            // Virtuoso can take minutes: dump in the background, import when it is done.
            if self.virtuoso_job.is_some() {
                return self.warn("A Virtuoso import is already running".to_string());
            }
            self.info(format!("Running Virtuoso in batch mode to dump {}…", lib.as_deref().unwrap_or("the library")));
            let (tx, rx) = mpsc::channel();
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (src, ctx, stop) = (path.clone(), ctx.clone(), cancel.clone());
            let what = lib.clone().unwrap_or_else(|| path.display().to_string());
            std::thread::spawn(move || {
                let _ = tx.send(ics_core::virtuoso_batch::read_dump_with(&src, lib.as_deref(), &stop));
                ctx.request_repaint();
            });
            self.virtuoso_job = Some(VirtuosoJob { path, what, rx, cancel, started: std::time::Instant::now() });
            return;
        }
        let text = ics_core::virtuoso_batch::read_dump(&path, lib.as_deref());
        self.import_virtuoso_text(&path, text);
    }

    /// Finishes a background Virtuoso dump.
    fn poll_virtuoso_job(&mut self) {
        let Some(job) = &self.virtuoso_job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the Virtuoso run stopped".to_string()),
        };
        let job = self.virtuoso_job.take().expect("checked");
        self.import_virtuoso_text(&job.path, res);
    }

    /// While Virtuoso dumps a library: its elapsed time and a Cancel button.
    fn virtuoso_job_window(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.virtuoso_job else { return };
        let mut cancel = false;
        egui::Window::new("Virtuoso")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -36.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Dumping {} in batch mode: {:.0} s", job.what, job.started.elapsed().as_secs_f32()));
                });
                cancel = ui.button("Cancel").clicked();
            });
        if cancel {
            self.queue.push(Command::CancelVirtuoso);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    /// Whether Virtuoso is dumping a library in the background.
    pub fn virtuoso_running(&self) -> bool {
        self.virtuoso_job.is_some()
    }

    fn import_virtuoso_text(&mut self, path: &Path, text: Result<String, String>) {
        let Some(p) = self.project.as_mut() else { return };
        let res = text.and_then(|t| {
            let d = ics_core::virtuoso_dump::Dump::parse(&t)?;
            ics_core::virtuoso_dump::import_dump(&d, &p.tech, &ics_core::cadence::SymbolMap::analog_lib(), 0.1875)
        });
        let imp = match res {
            Ok(i) => i,
            Err(e) => return self.error(format!("Cannot import {}: {e}", path.display())),
        };
        let mut warnings = imp.warnings.clone();
        let done = match ics_core::virtuoso_dump::apply(p, &imp, false, &mut warnings) {
            Ok(d) => d,
            Err(e) => return self.error(e),
        };
        self.hier.clear();
        self.mark_dirty();
        let (sch, lay) = (done.iter().filter(|a| a.schematic).count(), done.iter().filter(|a| a.layout).count());
        self.ok(format!("Virtuoso: {sch} schematic(s) and {lay} layout(s) imported from {}", path.display()));
        for w in warnings {
            self.warn(format!("Virtuoso: {w}"));
        }
        if let Some(a) = done.iter().find(|a| a.schematic).or_else(|| done.iter().find(|a| a.layout)) {
            let ws = if a.schematic { Workspace::Schematic } else { Workspace::Layout };
            self.queue.push(Command::OpenCell(a.cell.clone(), Some(ws)));
        }
    }

    fn save_dump_skill(&mut self, path: Option<PathBuf>) {
        let Some(path) =
            path.or_else(|| rfd::FileDialog::new().set_file_name("icsDump.il").add_filter("SKILL script", &["il"]).save_file())
        else {
            return;
        };
        match std::fs::write(&path, ics_core::virtuoso_dump::DUMP_SKILL) {
            Ok(()) => self.ok(format!(
                "Wrote {}. In the Virtuoso CIW: load(\"icsDump.il\") then icsDumpLib(\"myLib\" \"/tmp/myLib.icsdump\")",
                path.display()
            )),
            Err(e) => self.error(format!("Cannot write {}: {e}", path.display())),
        }
    }

    fn export_skill(&mut self) {
        let (Some(p), Some(r)) = (self.project.as_ref(), self.active.clone()) else { return };
        let Some(sch) = p.cell(&r).and_then(|c| c.schematic.as_ref()) else {
            return self.warn("Open a cell with a schematic to export it to Virtuoso".into());
        };
        // A symbol map next to the project (virtuoso_symbols.toml), else analogLib.
        let map_file = p.root.as_ref().map(|d| d.join("virtuoso_symbols.toml")).filter(|f| f.exists());
        let map = match &map_file {
            Some(f) => match std::fs::read_to_string(f)
                .map_err(|e| e.to_string())
                .and_then(|t| ics_core::cadence::SymbolMap::from_toml(&t))
            {
                Ok(m) => m,
                Err(e) => return self.error(format!("{}: {e}", f.display())),
            },
            None => ics_core::cadence::SymbolMap::analog_lib(),
        };
        let (il, warnings) = ics_core::cadence::skill_schematic(sch, &r.lib, &r.cell, &map, 0.1875);
        let Some(path) =
            rfd::FileDialog::new().add_filter("Cadence SKILL", &["il"]).set_file_name(format!("{}.il", r.cell)).save_file()
        else {
            return;
        };
        match std::fs::write(&path, il) {
            Ok(()) => self.ok(format!(
                "Wrote {} (load it in the Virtuoso CIW; symbols: {})",
                path.display(),
                map_file.map_or("analogLib".into(), |f| f.display().to_string())
            )),
            Err(e) => self.error(format!("{}: {e}", path.display())),
        }
        for w in warnings {
            self.warn(format!("SKILL export: {w}"));
        }
    }

    fn import_tech(&mut self) {
        let Some(p) = self.project.as_mut() else { return };
        let Some(path) = rfd::FileDialog::new().add_filter("KLayout layer properties", &["lyp"]).pick_file() else { return };
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let tech = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|xml| ics_core::tech_import::from_lyp(&xml, &name))
            .and_then(|t| p.set_tech(t).map(|w| (w, p.tech.layers.len())));
        match tech {
            Ok((warnings, layers)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(format!("Technology '{name}' imported from {} ({layers} layers)", path.display()));
                for w in warnings {
                    self.warn(format!("Technology: {w}"));
                }
            }
            Err(e) => self.error(format!("Cannot import {}: {e}", path.display())),
        }
    }

    fn load_skill(&mut self, path: Option<PathBuf>) {
        if self.project.is_none() {
            return;
        }
        let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter("Cadence SKILL", &["il", "ils"]).pick_file()) else {
            return;
        };
        self.ciw.push(CiwKind::Input, format!("load(\"{}\")", path.display().to_string().replace('\\', "/")));
        self.skill_start(ics_core::skill::Task::Load(path.clone()), SkillJobKind::Load(path));
    }

    /// Evaluate a CIW command in the persistent SKILL session.
    fn ciw_eval(&mut self, src: &str) {
        if self.project.is_none() {
            self.ciw.push(CiwKind::Error, "*Error* no project is open");
            return;
        }
        self.skill_start(ics_core::skill::Task::Eval { src: src.to_string(), file: None }, SkillJobKind::Ciw);
    }

    /// Run `task` in the CIW's SKILL session on a copy of the project. A quick script finishes
    /// at once; a longer one runs in the background behind a dialog that can cancel it, and its
    /// result replaces the project when it ends.
    fn skill_start(&mut self, task: ics_core::skill::Task, kind: SkillJobKind) {
        if self.skill_job.is_some() {
            return self.warn("A SKILL script is still running".to_string());
        }
        let current = self.active.clone();
        let Some(p) = self.project.as_ref() else { return };
        let before = layout_snapshot(p);
        self.ciw.session.start(p.clone(), task, current);
        self.skill_job = Some(SkillJob { kind, before, started: std::time::Instant::now() });
        self.poll_skill_job(std::time::Duration::from_millis(150));
    }

    /// Collect the background SKILL evaluation if it has ended (waiting up to `wait`).
    fn poll_skill_job(&mut self, wait: std::time::Duration) {
        if self.skill_job.is_none() {
            return;
        }
        let Some(f) = self.ciw.session.wait_timeout(wait) else { return };
        let job = self.skill_job.take().expect("checked");
        if let Some(p) = f.project {
            self.project = Some(p);
        }
        let res = f.result;
        self.ciw.show(&res);
        if let Ok(r) = &res {
            self.skill_forms.extend(r.forms.iter().map(|f| (f.clone(), f.fields.clone())));
        }
        let changed = self.layouts_edited(job.before);
        match &job.kind {
            SkillJobKind::Ciw => {}
            SkillJobKind::Load(path) => {
                let (output, err) = match &res {
                    Ok(r) => (r.output.clone(), None),
                    Err(e) => (e.output.clone(), Some(e.to_string())),
                };
                for line in output.lines() {
                    self.info(format!("SKILL: {line}"));
                }
                match err {
                    None => self.ok(format!("Loaded {}: {} cell(s) modified", path.display(), changed.len())),
                    Some(e) => {
                        self.error(format!("SKILL error: {e}"));
                        if !changed.is_empty() {
                            self.warn(format!("Edits made before the error are kept in {} cell(s)", changed.len()));
                        }
                    }
                }
            }
            SkillJobKind::Form => {
                if let Err(e) = &res {
                    self.error(format!("SKILL error: {e}"));
                }
            }
        }
        if self.active.is_none()
            && let Some(r) = changed.into_iter().next()
        {
            self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
        }
    }

    /// Whether a SKILL script is running in the background.
    pub fn skill_running(&self) -> bool {
        self.skill_job.is_some()
    }

    /// While a SKILL script runs: a dialog with its elapsed time and a Cancel button.
    fn skill_job_window(&mut self, ctx: &egui::Context) {
        self.poll_skill_job(std::time::Duration::ZERO);
        let Some(job) = &self.skill_job else { return };
        let secs = job.started.elapsed().as_secs_f32();
        let what = match &job.kind {
            SkillJobKind::Ciw => "CIW command".to_string(),
            SkillJobKind::Load(p) => p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().to_string()),
            SkillJobKind::Form => "form callback".to_string(),
        };
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("skill_running")).show(ctx, |ui| {
            ui.heading("Running SKILL");
            ui.label(format!("{what}: {secs:.1} s"));
            ui.add_space(6.0);
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            self.queue.push(Command::CancelSkill);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// After an external edit of any number of layouts (a SKILL script): record undo history
    /// for each changed cell that had a layout, refresh the editors and mark the project dirty.
    /// Returns the changed cells.
    /// Push an undo step for every schematic that differs from `before` (an MCP tool or a
    /// script changed it) and refit its editor.
    fn schematics_edited(&mut self, before: HashMap<CellRef, Schematic>) {
        let Some(p) = &self.project else { return };
        let changed: Vec<(CellRef, Schematic)> = p
            .all_cells()
            .filter(|(r, c)| c.schematic.is_some() && c.schematic.as_ref() != before.get(r))
            .map(|(r, _)| {
                let old = before.get(&r).cloned().unwrap_or_default();
                (r, old)
            })
            .collect();
        if changed.is_empty() {
            return;
        }
        for (r, old) in changed {
            let ed = self.sch_eds.entry(r).or_default();
            ed.history.push(&old);
            ed.selection.clear();
            ed.request_fit();
        }
        self.mark_dirty();
    }

    fn layouts_edited(&mut self, before: HashMap<CellRef, Layout>) -> Vec<CellRef> {
        let Some(p) = &self.project else { return vec![] };
        let mut changed: Vec<CellRef> =
            p.all_cells().filter(|(r, c)| c.layout.is_some() && c.layout.as_ref() != before.get(r)).map(|(r, _)| r).collect();
        changed.sort();
        if changed.is_empty() {
            return changed;
        }
        for r in &changed {
            if let Some(lay) = before.get(r) {
                self.lay_eds.entry(r.clone()).or_default().history.push(lay);
            }
        }
        for (r, ed) in self.lay_eds.iter_mut() {
            ed.layout_changed();
            let lay = p.cell(r).and_then(|c| c.layout.as_ref());
            ed.selection.retain(|id| lay.is_some_and(|l| l.ids().any(|i| i == *id)));
        }
        self.hier.clear();
        self.mark_dirty();
        changed
    }

    // ----- shortcuts -----
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let fire = |m: Modifiers, k: Key, c: Command, q: &mut Vec<Command>| {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k))) {
                q.push(c);
            }
        };
        let q = &mut self.queue;
        fire(cs, Key::S, Command::SaveAs, q);
        fire(cmd, Key::S, Command::Save, q);
        fire(cmd, Key::O, Command::OpenProjectDialog, q);
        fire(cmd, Key::N, Command::NewCellDialog, q);
        fire(cmd, Key::K, Command::CommandPalette, q);
        fire(cs, Key::P, Command::CommandPalette, q);
        // In the layout, Ctrl+J flips (Virtuoso); the console stays on the status bar and the View menu.
        if self.workspace != Workspace::Layout {
            fire(cmd, Key::J, Command::ToggleConsole, q);
        }
        fire(cmd, Key::Enter, Command::RunSimulation, q);
        fire(Modifiers::NONE, Key::F5, Command::RunSimulation, q);
        fire(cmd, Key::Num1, Command::SetWorkspace(Workspace::Home), q);
        fire(cmd, Key::Num2, Command::SetWorkspace(Workspace::Schematic), q);
        fire(cmd, Key::Num3, Command::SetWorkspace(Workspace::Simulation), q);
        fire(cmd, Key::Num4, Command::SetWorkspace(Workspace::Layout), q);
        fire(cmd, Key::Comma, Command::Settings, q);
        fire(Modifiers::NONE, Key::F1, Command::Shortcuts, q);
        fire(Modifiers::NONE, Key::F2, Command::Save, q);
        if !ctx.text_edit_focused() {
            fire(cs, Key::Z, Command::Redo, q);
            fire(cmd, Key::Z, Command::Undo, q);
            fire(cmd, Key::Y, Command::Redo, q);
        }
        if let Some(r) = self.active.clone()
            && ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::W)))
        {
            self.queue.push(Command::CloseTab(r));
        }
    }

    // ----- UI pieces -----
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let (has_sch, has_lay) = self.active_cell().map_or((false, false), |c| (c.schematic.is_some(), c.layout.is_some()));
        let q = &mut self.queue;
        let has_project = self.project.is_some();
        let lay = self.workspace == Workspace::Layout && self.active.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            let adv = !self.settings.beginner;
            ui.menu_button(t("File"), |ui| {
                if ui.button(t("New Project…")).clicked() {
                    q.push(Command::NewProjectDialog);
                }
                if ui.button(t("New Demo Project")).clicked() {
                    q.push(Command::NewDemoProject);
                }
                ui.menu_button(t("Open Example"), |ui| {
                    for t in &examples::TOPICS {
                        if ui.button(t.title).on_hover_text(t.description).clicked() {
                            q.push(Command::OpenExample(t.name.into()));
                        }
                    }
                });
                if ui
                    .button("Save Examples to Folder…")
                    .on_hover_text(t("Write the example projects, one folder per topic"))
                    .clicked()
                {
                    q.push(Command::SaveExamples);
                }
                if ui.add(egui::Button::new(t("Open Project…")).shortcut_text(theme::kbd("⌘O"))).clicked() {
                    q.push(Command::OpenProjectDialog);
                }
                ui.menu_button(t("Open Recent"), |ui| {
                    if self.settings.recent.is_empty() {
                        ui.label(t("No recent projects"));
                    }
                    for p in &self.settings.recent {
                        if ui.button(p.display().to_string()).clicked() {
                            q.push(Command::OpenProject(p.clone()));
                        }
                    }
                });
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new(t("Save")).shortcut_text(theme::kbd("⌘S / F2"))).clicked() {
                    q.push(Command::Save);
                }
                if ui.add_enabled(has_project, egui::Button::new(t("Save As…")).shortcut_text(theme::kbd("⇧⌘S"))).clicked() {
                    q.push(Command::SaveAs);
                }
                if ui
                    .add_enabled(has_project, egui::Button::new(t("Check and Save")))
                    .on_hover_text(t("Check the active schematic, save if it has no errors"))
                    .clicked()
                {
                    q.push(Command::CheckSchematic(true));
                }
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new(t("New Cell…")).shortcut_text(theme::kbd("⌘N"))).clicked() {
                    q.push(Command::NewCellDialog);
                }
                if ui
                    .add_enabled(self.active.is_some(), egui::Button::new(t("Copy Cell as New…")))
                    .on_hover_text(t("Start a new design from the active cell: every view, specs and notes"))
                    .clicked()
                    && let Some(r) = self.active.clone()
                {
                    q.push(Command::CopyCellDialog(r));
                }
                ui.menu_button(t("Import"), |ui| {
                    if ui.add_enabled(has_project, egui::Button::new(t("GDSII / OASIS…"))).clicked() {
                        q.push(Command::ImportGds(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("PDK…"))).clicked() {
                        q.push(Command::LoadPdk(None));
                    }
                    if !adv {
                        return;
                    }
                    ui.separator();
                    if ui.add_enabled(has_project, egui::Button::new(t("GDSII / OASIS with Layer Map…"))).clicked() {
                        q.push(Command::ImportGdsWithLayerMap(None, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("LEF abstracts…"))).clicked() {
                        q.push(Command::ImportLefDef(false, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("DEF placement…"))).clicked() {
                        q.push(Command::ImportLefDef(true, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("KLayout layer properties…"))).clicked() {
                        q.push(Command::ImportTech);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("CDL netlist (Virtuoso)…"))).clicked() {
                        q.push(Command::ImportCdl(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Virtuoso library (dump or cds.lib)…"))).clicked() {
                        q.push(Command::ImportVirtuoso(None));
                    }
                });
                ui.menu_button(t("Export"), |ui| {
                    if ui.add_enabled(has_project, egui::Button::new(t("SPICE netlist…"))).clicked() {
                        q.push(Command::ExportNetlist);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell GDSII…"))).clicked() {
                        q.push(Command::ExportGds);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Library GDSII…"))).clicked() {
                        q.push(Command::ExportLibraryGds);
                    }
                    if !adv {
                        return;
                    }
                    ui.separator();
                    if ui.add_enabled(has_project, egui::Button::new(t("Virtuoso schematic (SKILL)…"))).clicked() {
                        q.push(Command::ExportSkill);
                    }
                    if ui.button(t("Virtuoso dump script (icsDump.il)…")).clicked() {
                        q.push(Command::SaveDumpSkill(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell OASIS…"))).clicked() {
                        q.push(Command::ExportOasis);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Library OASIS…"))).clicked() {
                        q.push(Command::ExportLibraryOasis);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell LEF abstract…"))).clicked() {
                        q.push(Command::ExportLefDef(false));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell DEF…"))).clicked() {
                        q.push(Command::ExportLefDef(true));
                    }
                });
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new(t("Close Project"))).clicked() {
                    q.push(Command::CloseProject);
                }
                if ui.add(egui::Button::new(t("Settings…")).shortcut_text(theme::kbd("⌘,"))).clicked() {
                    q.push(Command::Settings);
                }
                if ui.add(egui::Button::new(t("Quit")).shortcut_text(theme::kbd("⌘Q"))).clicked() {
                    q.push(Command::Quit);
                }
            });
            ui.menu_button(t("Edit"), |ui| {
                if ui.add(egui::Button::new(t("Undo")).shortcut_text(theme::kbd(if lay { "u / ⌘Z" } else { "⌘Z" }))).clicked() {
                    q.push(Command::Undo);
                }
                if ui
                    .add(egui::Button::new(t("Redo")).shortcut_text(theme::kbd(if lay { "Shift+U / ⇧⌘Z" } else { "⇧⌘Z" })))
                    .clicked()
                {
                    q.push(Command::Redo);
                }
                ui.separator();
                if ui.add(egui::Button::new(t("Delete")).shortcut_text("Del")).clicked() {
                    q.push(Command::Delete);
                }
                if lay {
                    ui.separator();
                    lay_menu_items(ui, "Edit", q);
                }
            });
            ui.menu_button(t("View"), |ui| {
                for (i, w) in Workspace::ALL.iter().enumerate() {
                    if ui.add(egui::Button::new(w.name()).shortcut_text(theme::kbd(&format!("⌘{}", i + 1)))).clicked() {
                        q.push(Command::SetWorkspace(*w));
                    }
                }
                ui.separator();
                if ui.add(egui::Button::new(t("Fit to Window")).shortcut_text("F")).clicked() {
                    q.push(Command::FitView);
                }
                if ui.add(egui::Button::new(t("Toggle Console")).shortcut_text(theme::kbd("⌘J"))).clicked() {
                    q.push(Command::ToggleConsole);
                }
                if ui.button(if self.settings.dark { "Light Theme" } else { "Dark Theme" }).clicked() {
                    q.push(Command::ToggleTheme);
                }
                if lay {
                    ui.separator();
                    lay_menu_items(ui, "View", q);
                }
            });
            if lay {
                ui.menu_button(t("Create"), |ui| lay_menu_items(ui, "Create", q));
            }
            ui.menu_button(t("Simulation"), |ui| {
                if ui
                    .add_enabled(has_sch, egui::Button::new(t("Run")).shortcut_text("F5"))
                    .on_disabled_hover_text(NEED_SCH)
                    .clicked()
                {
                    q.push(Command::RunSimulation);
                }
                if menu_item(ui, has_sch, "Export Netlist…", NEED_SCH) {
                    q.push(Command::ExportNetlist);
                }
                ui.separator();
                if menu_item(ui, has_sch, "Corner / Sweep Manager…", NEED_SCH) {
                    q.push(Command::SweepManager);
                }
                if adv && menu_item(ui, has_sch, "Monte Carlo…", NEED_SCH) {
                    q.push(Command::MonteCarlo);
                }
                if menu_item(ui, has_sch, "Optimizer…", NEED_SCH) {
                    q.push(Command::Optimizer);
                }
                if menu_item(ui, has_sch, "Derive New Design…", NEED_SCH)
                    && let Some(r) = self.active.clone()
                {
                    q.push(Command::DeriveDialog(r));
                }
                if menu_item(ui, has_project, "Verification…", NEED_PROJECT) {
                    q.push(Command::Verification);
                }
                ui.separator();
                if ui.button(t("Waveform Calculator…")).clicked() {
                    q.push(Command::Calculator);
                }
                if ui.button(t("Results Browser…")).clicked() {
                    q.push(Command::ResultsBrowser);
                }
                if !adv {
                    return;
                }
                ui.separator();
                if ui.button(t("Pixel Optics…")).clicked() {
                    q.push(Command::PixelOptics);
                }
                if menu_item(ui, has_sch, "Silicon Correlation…", NEED_SCH) {
                    q.push(Command::SiliconCorrelation);
                }
                if menu_item(ui, has_lay, "Process Emulation…", NEED_LAY) {
                    q.push(Command::ProcessEmulation);
                }
                if ui.button(t("Process Data…")).clicked() {
                    q.push(Command::ProcessData);
                }
            });
            ui.menu_button(t("Verify"), |ui| {
                if menu_item(ui, has_lay, "Run DRC", NEED_LAY) {
                    q.push(Command::RunDrc);
                }
                if menu_item(ui, has_lay, "Extract Nets", NEED_LAY) {
                    q.push(Command::ExtractNets);
                }
                if adv && menu_item(ui, has_lay, "Schematic from Layout", NEED_LAY) {
                    q.push(Command::SchematicFromLayout);
                }
                if adv && menu_item(ui, has_lay, "Pixel Circuit…", NEED_LAY) {
                    q.push(Command::PixelCircuit);
                }
                if menu_item(ui, has_lay && has_sch, "Run LVS", "Open a cell with a schematic and a layout first") {
                    q.push(Command::RunLvs);
                }
                if lay && ui.add(egui::Button::new(t("Mark Net")).shortcut_text("Shift+N")).clicked() {
                    q.push(Command::Lay(LayAction::MarkNet));
                }
                if adv && menu_item(ui, has_lay, "Run Sign-off Runset", NEED_LAY) {
                    q.push(Command::RunSignoff(None));
                }
                if adv && menu_item(ui, has_lay, "Import Sign-off Results…", NEED_LAY) {
                    q.push(Command::ImportSignoffResults(None));
                }
                ui.separator();
                if menu_item(ui, has_sch, "Generate Layout from Schematic", NEED_SCH) {
                    q.push(Command::GenerateLayout);
                }
                if menu_item(ui, has_lay, "Route Flylines", NEED_LAY) {
                    q.push(Command::RouteFlylines);
                }
                if menu_item(ui, has_lay && has_sch, "Check Against Source", NEED_SCH) {
                    q.push(Command::CheckAgainstSource);
                }
                if menu_item(ui, has_lay && has_sch, "Update Components and Nets", NEED_SCH) {
                    q.push(Command::UpdateFromSource(false));
                }
                if adv && menu_item(ui, has_lay && has_sch, "Update and Delete Extra Devices", NEED_SCH) {
                    q.push(Command::UpdateFromSource(true));
                }
            });
            ui.menu_button(t("Tools"), |ui| {
                if ui.button(t("Design Copilot…")).clicked() {
                    q.push(Command::Copilot);
                }
                if ui.button(t("AI Models…")).clicked() {
                    q.push(Command::AiModels);
                }
                ui.separator();
                if menu_item(ui, has_sch, "Check Schematic", NEED_SCH) {
                    q.push(Command::CheckSchematic(false));
                }
                if menu_item(ui, has_sch, "Place Cell Instance…", NEED_SCH) {
                    q.push(Command::PlaceCellDialog);
                }
                if menu_item(ui, has_sch, "Update Cell Instance Pins", NEED_SCH) {
                    q.push(Command::UpdateCellInstances);
                }
                if adv && menu_item(ui, has_sch, "Edit Symbol Pins…", NEED_SCH) {
                    q.push(Command::EditSymbolDialog);
                }
                if menu_item(ui, has_project, "Hierarchy Browser…", NEED_PROJECT) {
                    q.push(Command::HierarchyBrowser);
                }
                if menu_item(ui, has_project, "Net Browser…", NEED_PROJECT) {
                    q.push(Command::NetBrowser);
                }
                if !adv {
                    return;
                }
                ui.menu_button(t("Layout Generators"), |ui| {
                    if menu_item(ui, has_project, "Guard Ring…", NEED_PROJECT) {
                        q.push(Command::GuardRing);
                    }
                    if menu_item(ui, has_project, "Via Array…", NEED_PROJECT) {
                        q.push(Command::ViaArray);
                    }
                    if menu_item(ui, has_project, "Matched Placement…", NEED_PROJECT) {
                        q.push(Command::MatchedPlacement);
                    }
                });
                ui.separator();
                if ui.button(t("DRC Rule Designer…")).clicked() {
                    q.push(Command::RuleDesigner);
                }
                if menu_item(ui, has_project, "PCell Designer…", NEED_PROJECT) {
                    q.push(Command::PCellDesigner);
                }
                ui.menu_button(t("DRC Deck"), |ui| {
                    let linked = self.project.as_ref().and_then(Project::drc_deck_path);
                    if let Some(f) = &linked {
                        ui.label(format!("Shared: {}", f.display()));
                        ui.separator();
                    }
                    if menu_item(ui, has_project, "Import Deck…", NEED_PROJECT) {
                        q.push(Command::ImportDrcDeck(None));
                    }
                    if menu_item(ui, has_project, "Export Deck…", NEED_PROJECT) {
                        q.push(Command::ExportDrcDeck(None));
                    }
                    ui.separator();
                    if menu_item(ui, has_project, "Link Shared Deck…", NEED_PROJECT) {
                        q.push(Command::LinkDrcDeck(None));
                    }
                    let why = if has_project { "The technology is not linked to a shared deck" } else { NEED_PROJECT };
                    if menu_item(ui, linked.is_some(), "Reload Shared Deck", why) {
                        q.push(Command::ReloadDrcDeck);
                    }
                    if menu_item(ui, linked.is_some(), "Unlink Shared Deck", why) {
                        q.push(Command::UnlinkDrcDeck);
                    }
                });
                if menu_item(ui, has_project, "Stream Layer Map…", NEED_PROJECT) {
                    q.push(Command::StreamMap);
                }
                if menu_item(ui, has_project, "Layer Display…", NEED_PROJECT) {
                    q.push(Command::LayerDisplay(None));
                }
                if menu_item(ui, has_project, "Dummy Fill…", NEED_PROJECT) {
                    q.push(Command::DummyFill);
                }
                if menu_item(ui, has_project, "OPC…", NEED_PROJECT) {
                    q.push(Command::Opc);
                }
                ui.separator();
                if ui.button(t("SKILL CIW")).clicked() {
                    q.push(Command::Ciw);
                }
                if menu_item(ui, has_project, "Load SKILL (.il)…", NEED_PROJECT) {
                    q.push(Command::LoadSkill(None));
                }
            });
            if lay {
                ui.menu_button(t("Options"), |ui| lay_menu_items(ui, "Options", q));
            }
            if adv {
                Self::extensions_menu(&self.extensions, ui, has_project, q);
            }
            ui.menu_button(t("Help"), |ui| {
                if ui.button(t("Interactive Tutorial")).clicked() {
                    q.push(Command::Tutorial);
                }
                let mut beginner = self.settings.beginner;
                if ui
                    .checkbox(&mut beginner, t("Beginner Mode"))
                    .on_hover_text(t("Show the everyday design flow in the menus and hide specialist tools (the command palette still finds every command)"))
                    .clicked()
                {
                    q.push(Command::BeginnerMode(beginner));
                }
                if ui.button(t("User Guide")).clicked() {
                    q.push(Command::UserGuide);
                }
                if ui.add(egui::Button::new(t("Keyboard Shortcuts")).shortcut_text("F1")).clicked() {
                    q.push(Command::Shortcuts);
                }
                if ui.add(egui::Button::new(t("Command Palette")).shortcut_text(theme::kbd("⌘K"))).clicked() {
                    q.push(Command::CommandPalette);
                }
                ui.separator();
                if ui.button(t("Check for Updates…")).clicked() {
                    q.push(Command::CheckForUpdates);
                }
                if ui.button(t("About IC Studio")).clicked() {
                    q.push(Command::About);
                }
            });
            // breadcrumb
            ui.add_space(16.0);
            if let Some(p) = &self.project {
                ui.label(RichText::new(&p.name).strong());
                if let Some(r) = &self.active {
                    ui.label(RichText::new("›").color(pal.text_weak));
                    ui.label(RichText::new(&r.lib).color(pal.text_weak));
                    ui.label(RichText::new("›").color(pal.text_weak));
                    ui.label(RichText::new(&r.cell));
                }
                if self.dirty {
                    ui.label(RichText::new("●").color(pal.warning)).on_hover_text("Unsaved changes");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(theme::kbd(&format!("🔍  {}   ⌘K", t("Search commands…")))).color(pal.text_weak))
                    .min_size(Vec2::new(220.0, 22.0));
                if ui.add(b).clicked() {
                    q.push(Command::CommandPalette);
                }
            });
        });
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let spot = self.tutorial.spotlight().filter(|w| *w != self.workspace);
        ui.add_space(6.0);
        for w in Workspace::ALL {
            let active = self.workspace == w;
            let (r, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 48.0), Sense::click());
            let color = w.color(&pal);
            if spot == Some(w) {
                let t = ui.input(|i| i.time);
                let a = 0.45 + 0.4 * (t * 4.0).sin().abs() as f32;
                p.rect_stroke(
                    r.rect.shrink2(Vec2::new(4.0, 1.0)),
                    9.0,
                    Stroke::new(2.0, color.gamma_multiply(a)),
                    egui::StrokeKind::Inside,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
            if active {
                p.rect_filled(r.rect.shrink2(Vec2::new(6.0, 3.0)), 8.0, color.gamma_multiply(0.16));
                p.rect_filled(egui::Rect::from_min_size(r.rect.min + Vec2::new(0.0, 10.0), Vec2::new(3.0, 28.0)), 2.0, color);
            } else if r.hovered() {
                p.rect_filled(r.rect.shrink2(Vec2::new(6.0, 3.0)), 8.0, pal.bg_hover);
            }
            let ic = egui::Rect::from_center_size(r.rect.center(), Vec2::splat(24.0));
            icons::paint(
                &p,
                ic,
                w.icon(),
                if active {
                    color
                } else if r.hovered() {
                    pal.text
                } else {
                    pal.text_weak
                },
            );
            if r.clicked() {
                self.queue.push(Command::SetWorkspace(w));
            }
            let i = Workspace::ALL.iter().position(|x| *x == w).unwrap() + 1;
            r.on_hover_text(theme::kbd(&format!("{} (⌘{i})", w.name())));
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            let (r, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 44.0), Sense::click());
            if r.hovered() {
                p.rect_filled(r.rect.shrink2(Vec2::new(6.0, 3.0)), 8.0, pal.bg_hover);
            }
            icons::paint(&p, egui::Rect::from_center_size(r.rect.center(), Vec2::splat(22.0)), Icon::Settings, pal.text_weak);
            if r.clicked() {
                self.queue.push(Command::Settings);
            }
            r.on_hover_text(theme::kbd("Settings (⌘,)"));
        });
    }

    fn explorer(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let Some(p) = &self.project else {
            ui.label(RichText::new(t("No project open")).color(pal.text_weak));
            if ui.button(t("Open demo project")).clicked() {
                self.queue.push(Command::NewDemoProject);
            }
            return;
        };
        ui.horizontal(|ui| {
            theme::section(ui, t("Explorer"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+").on_hover_text(theme::kbd("New cell (⌘N)")).clicked() {
                    self.queue.push(Command::NewCellDialog);
                }
            });
        });
        ui.horizontal(|ui| {
            let l = ui.label(RichText::new(t("Filter")).small().color(pal.text_weak));
            let te = egui::TextEdit::singleline(&mut self.explorer_filter).hint_text(t("cell name")).desired_width(f32::INFINITY);
            let resp = ui.add(te).labelled_by(l.id);
            if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                self.explorer_filter.clear();
            }
        });
        let filter = self.explorer_filter.trim().to_lowercase();
        if !filter.is_empty() {
            let mut any = false;
            for lib in &p.libraries {
                let hits: Vec<&Cell> = lib.cells.iter().filter(|c| c.name.to_lowercase().contains(&filter)).collect();
                if hits.is_empty() {
                    continue;
                }
                any = true;
                ui.label(RichText::new(&lib.name).small().strong().color(pal.text_weak));
                for cell in hits {
                    let r = CellRef::new(&lib.name, &cell.name);
                    let active = self.active.as_ref() == Some(&r);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let (ir, painter) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
                        icons::paint(&painter, ir.rect, Icon::Cell, if active { pal.accent } else { pal.text_weak });
                        let resp = ui.selectable_label(active, &cell.name);
                        if resp.clicked() {
                            self.selected_lib = None;
                            self.queue.push(Command::OpenCell(r.clone(), None));
                        }
                        let resp = Self::cell_menu(Some(p), &mut self.queue, &resp, &r, None);
                        resp.on_hover_text(cell_tip(cell));
                    });
                }
            }
            if !any {
                ui.label(RichText::new("No cell matches").color(pal.text_weak));
            }
            return;
        }
        for lib in &p.libraries {
            // Libraries of generated PCell variants start folded: they are many, and rarely opened.
            let generated = !lib.cells.is_empty() && lib.cells.iter().all(|c| c.pcell.is_some());
            let id = ui.make_persistent_id(("lib", &lib.name));
            let mut toggle = false;
            let selected = self.selected_lib.as_deref() == Some(lib.name.as_str());
            let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, !generated)
                .show_header(ui, |ui| {
                    let (r, painter) = ui.allocate_painter(Vec2::splat(16.0), Sense::hover());
                    icons::paint(&painter, r.rect, Icon::Library, if selected { pal.accent } else { pal.text_weak });
                    let resp = ui.selectable_label(selected, RichText::new(&lib.name).strong());
                    if resp.clicked() {
                        self.selected_lib = Some(lib.name.clone());
                        toggle = true;
                    }
                    if resp.secondary_clicked() {
                        self.selected_lib = Some(lib.name.clone());
                    }
                    let gds = lib.storage == LibStorage::Gds;
                    if gds {
                        ui.label(RichText::new("GDS").small().color(pal.text_weak));
                    }
                    resp.context_menu(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} · {} cell(s) · {}",
                                lib.name,
                                lib.cells.len(),
                                if gds { "GDSII file" } else { "JSON" }
                            ))
                            .small()
                            .color(pal.text_weak),
                        );
                        ui.separator();
                        if ui.button("New cell…").clicked() {
                            self.queue.push(Command::NewCellInLibrary(lib.name.clone()));
                        }
                        let tops = p.library_tops(&lib.name);
                        ui.add_enabled_ui(!tops.is_empty(), |ui| {
                            ui.menu_button("Open top cell layout", |ui| {
                                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for t in &tops {
                                        if ui.button(&t.cell).clicked() {
                                            self.queue.push(Command::OpenCell(t.clone(), Some(Workspace::Layout)));
                                        }
                                    }
                                });
                            });
                        });
                        ui.separator();
                        if ui.add_enabled(!tops.is_empty(), egui::Button::new("Export GDSII…")).clicked() {
                            self.queue.push(Command::ExportLibraryStream(lib.name.clone(), StreamFormat::Gds));
                        }
                        if ui.add_enabled(!tops.is_empty(), egui::Button::new("Export OASIS…")).clicked() {
                            self.queue.push(Command::ExportLibraryStream(lib.name.clone(), StreamFormat::Oasis));
                        }
                        let mut store_gds = gds;
                        if ui
                            .checkbox(&mut store_gds, "Store as one GDSII file")
                            .on_hover_text("Save the library's layouts in a single .gds file instead of JSON cell directories")
                            .changed()
                        {
                            let storage = if store_gds { LibStorage::Gds } else { LibStorage::Json };
                            self.queue.push(Command::SetLibraryStorage(lib.name.clone(), storage));
                        }
                        ui.separator();
                        if ui.button("Rename…").clicked() {
                            self.queue.push(Command::RenameLibraryDialog(lib.name.clone()));
                        }
                        if ui.button("Delete…").clicked() {
                            self.queue.push(Command::DeleteLibraryConfirm(lib.name.clone()));
                        }
                    });
                });
            if toggle {
                header.toggle();
            }
            header.body(|ui| {
                for cell in &lib.cells {
                    let r = CellRef::new(&lib.name, &cell.name);
                    let active = self.active.as_ref() == Some(&r);
                    let cid = ui.make_persistent_id(("cell", &r));
                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), cid, active)
                        .show_header(ui, |ui| {
                            let (ir, painter) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
                            icons::paint(&painter, ir.rect, Icon::Cell, if active { pal.accent } else { pal.text_weak });
                            let resp = ui.selectable_label(active, &cell.name);
                            if resp.clicked() {
                                self.selected_lib = None;
                                self.queue.push(Command::OpenCell(r.clone(), None));
                            }
                            let resp = Self::cell_menu(Some(p), &mut self.queue, &resp, &r, None);
                            resp.on_hover_text(cell_tip(cell));
                        })
                        .body(|ui| {
                            for (vk, icon, color, ws) in [
                                (ViewKind::Schematic, Icon::Schematic, pal.schematic, Workspace::Schematic),
                                (ViewKind::Simulation, Icon::Simulation, pal.simulation, Workspace::Simulation),
                                (ViewKind::Layout, Icon::Layout, pal.layout, Workspace::Layout),
                            ] {
                                let exists = cell.has_view(vk);
                                ui.horizontal(|ui| {
                                    let (ir, painter) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
                                    icons::paint(
                                        &painter,
                                        ir.rect,
                                        icon,
                                        if exists { color } else { pal.text_weak.gamma_multiply(0.5) },
                                    );
                                    let text = RichText::new(vk.label()).color(if exists { pal.text } else { pal.text_weak });
                                    let is_open = active && self.workspace == ws;
                                    if ui.selectable_label(is_open, text).clicked() {
                                        self.queue.push(if exists {
                                            Command::OpenCell(r.clone(), Some(ws))
                                        } else {
                                            Command::CreateView(r.clone(), vk)
                                        });
                                    }
                                    if !exists {
                                        ui.label(RichText::new("+ create").small().color(pal.text_weak));
                                    }
                                });
                            }
                        });
                }
            });
        }
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for t in self.tabs.clone() {
                let active = self.active.as_ref() == Some(&t);
                egui::Frame::new()
                    .fill(if active { pal.bg_canvas } else { pal.bg_panel })
                    .corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                    .inner_margin(egui::Margin { left: 10, right: 4, top: 4, bottom: 4 })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let c = if active { self.workspace.color(&pal) } else { pal.text_weak };
                            let (r, p) = ui.allocate_painter(Vec2::splat(8.0), Sense::hover());
                            p.circle_filled(r.rect.center(), 3.5, c);
                            let resp = ui.add(
                                egui::Label::new(RichText::new(&t.cell).color(if active { pal.text } else { pal.text_weak }))
                                    .sense(Sense::click()),
                            );
                            if resp.clicked() {
                                self.queue.push(Command::OpenCell(t.clone(), None));
                            }
                            if resp.middle_clicked() {
                                self.queue.push(Command::CloseTab(t.clone()));
                            }
                            let resp = resp.on_hover_text(format!("{t}\nMiddle-click to close, right-click for more"));
                            Self::cell_menu(self.project.as_ref(), &mut self.queue, &resp, &t, Some(self.tabs.len()));
                            if ui.add(egui::Button::new(RichText::new("×").color(pal.text_weak)).frame(false).small()).clicked()
                            {
                                self.queue.push(Command::CloseTab(t.clone()));
                            }
                        });
                    });
            }
        });
    }

    /// Right-click menu of a cell, shared by the explorer and the tab strip (`n_tabs` adds the tab items).
    fn cell_menu(
        p: Option<&Project>,
        q: &mut Vec<Command>,
        resp: &egui::Response,
        r: &CellRef,
        n_tabs: Option<usize>,
    ) -> egui::Response {
        let views: Vec<(ViewKind, bool)> = p.and_then(|p| p.cell(r)).map_or_else(Vec::new, |c| {
            [ViewKind::Schematic, ViewKind::Simulation, ViewKind::Layout].into_iter().map(|vk| (vk, c.has_view(vk))).collect()
        });
        resp.context_menu(|ui| {
            ui.label(RichText::new(r.to_string()).small().strong());
            ui.separator();
            for (vk, exists) in &views {
                let ws = match vk {
                    ViewKind::Schematic => Workspace::Schematic,
                    ViewKind::Simulation => Workspace::Simulation,
                    _ => Workspace::Layout,
                };
                let text = if *exists { tf("Open {}", &[t(vk.label())]) } else { tf("Create {} view", &[t(vk.label())]) };
                if ui.button(text).clicked() {
                    q.push(if *exists { Command::OpenCell(r.clone(), Some(ws)) } else { Command::CreateView(r.clone(), *vk) });
                }
            }
            if views.iter().any(|(vk, e)| *vk == ViewKind::Schematic && *e) {
                ui.separator();
                if ui.add(egui::Button::new(t("Run Simulation")).shortcut_text("F5")).clicked() {
                    q.push(Command::OpenCell(r.clone(), None));
                    q.push(Command::RunSimulation);
                }
                if ui.button(t("Place in Active Schematic")).clicked() {
                    q.push(Command::PlaceCell(r.clone()));
                }
            }
            ui.separator();
            if ui.button(t("Copy Name")).clicked() {
                ui.ctx().copy_text(r.to_string());
            }
            if ui
                .button(t("Copy as New Cell…"))
                .on_hover_text(t("Start a new design from this one: every view, specs and notes"))
                .clicked()
            {
                q.push(Command::CopyCellDialog(r.clone()));
            }
            if ui.button(t("Rename…")).clicked() {
                q.push(Command::RenameCellDialog(r.clone()));
            }
            if ui.button(t("Delete…")).clicked() {
                q.push(Command::DeleteCellConfirm(r.clone()));
            }
            if let Some(n_tabs) = n_tabs {
                ui.separator();
                if ui.button(t("Close Tab")).clicked() {
                    q.push(Command::CloseTab(r.clone()));
                }
                if ui.add_enabled(n_tabs > 1, egui::Button::new(t("Close Other Tabs"))).clicked() {
                    q.push(Command::CloseOtherTabs(r.clone()));
                }
                if ui.button(t("Close All Tabs")).clicked() {
                    q.push(Command::CloseAllTabs);
                }
            }
        });
        resp.clone()
    }

    fn no_view_placeholder(&mut self, ui: &mut egui::Ui, r: &CellRef, vk: ViewKind) {
        let pal = theme::palette(ui.ctx());
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(RichText::new(format!("{} has no {} view", r.cell, vk.label())).size(16.0).color(pal.text_weak));
                ui.add_space(8.0);
                if ui.button(format!("Create {} view", vk.label())).clicked() {
                    self.queue.push(Command::CreateView(r.clone(), vk));
                }
            });
        });
    }

    fn no_cell_placeholder(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(theme::kbd("Open a cell from the Explorer, or create one with ⌘N")).size(15.0).color(pal.text_weak),
            );
        });
    }

    fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let ws = self.workspace;
        if ws == Workspace::Home {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_canvas)).show(ui, |ui| match &self.project {
                Some(p) => home_view::project_overview(ui, p, &mut self.hier, &mut self.queue),
                None => home_view::welcome(ui, &self.settings.recent, &mut self.queue),
            });
            return;
        }
        let Some(r) = self.active.clone() else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(pal.bg_canvas))
                .show(ui, |ui| self.no_cell_placeholder(ui));
            return;
        };
        let Some(project) = self.project.as_mut() else { return };
        let tech = project.tech.clone();
        let bulk_nets = [&project.settings.nmos_bulk, &project.settings.pmos_bulk].map(|n| n.clone().unwrap_or_default());
        let Some(cell) = project.cell_mut(&r) else {
            self.queue.push(Command::CloseTab(r));
            return;
        };
        let mut changed = false;
        match ws {
            Workspace::Schematic => {
                let op = self.sims.get(&r).and_then(|s| s.op_annotation());
                let ed = self.sch_eds.entry(r.clone()).or_default();
                ed.mos_body_pin = self.settings.mos_body_pin;
                ed.bulk_nets = bulk_nets.clone();
                if let Some(sch) = cell.schematic.as_mut() {
                    theme::aside(egui::Panel::left("sch_tools").resizable(true).default_size(220.0), ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| device_palette(ui, ed));
                    });
                    self.settings.mos_body_pin = ed.mos_body_pin;
                    for (k, (new, old)) in ed.bulk_nets.iter().zip(&bulk_nets).enumerate() {
                        if new != old {
                            self.queue.push(Command::DefaultBulk(k == 1, new.clone()));
                        }
                    }
                    let notes = &mut cell.notes;
                    let specs = &cell.sim.sweep.specs;
                    let q = &mut self.queue;
                    theme::aside(egui::Panel::right("sch_inspector").resizable(true).default_size(250.0), ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            if ed.selection.is_empty() {
                                changed |= crate::guide_view::design_panel(ui, &r, notes, specs, q);
                            }
                            changed |= ed.inspector(ui, sch);
                        });
                    });
                    egui::Panel::top("sch_toolbar").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (t, icon, tip) in [
                                (SchTool::Select, Icon::Pointer, "Select (Esc)"),
                                (SchTool::Wire, Icon::Wire, "Draw wire (W)"),
                                (SchTool::Label, Icon::NetLabel, "Net label (T)"),
                            ] {
                                if icons::button(ui, icon, ed.tool == t, true, tip).clicked() {
                                    ed.set_tool(t);
                                }
                            }
                            icons::divider(ui);
                            let tip = theme::kbd("Undo (⌘Z)");
                            if icons::button(ui, Icon::Undo, false, ed.history.can_undo(), &tip).clicked() {
                                changed |= ed.undo(sch);
                            }
                            let tip = theme::kbd("Redo (⇧⌘Z)");
                            if icons::button(ui, Icon::Redo, false, ed.history.can_redo(), &tip).clicked() {
                                changed |= ed.redo(sch);
                            }
                            icons::divider(ui);
                            let any = !ed.selection.is_empty();
                            let inst = ed.selection.iter().any(|o| matches!(o, SchObj::Instance(_)));
                            if icons::button(ui, Icon::Rotate, false, inst, "Rotate selection (Space)").clicked() {
                                changed |= ed.rotate_selection(sch, false);
                            }
                            if icons::button(ui, Icon::Mirror, false, inst, "Mirror selection (X)").clicked() {
                                changed |= ed.rotate_selection(sch, true);
                            }
                            let tip = theme::kbd("Duplicate selection (⇧⌘D)");
                            if icons::button(ui, Icon::Duplicate, false, any, &tip).clicked() {
                                changed |= ed.duplicate_selection(sch);
                            }
                            if icons::button(ui, Icon::Delete, false, any, "Delete selection (Del)").clicked() {
                                changed |= ed.delete_selection(sch);
                            }
                            icons::divider(ui);
                            if icons::button(ui, Icon::Fit, false, true, "Fit to window (F)").clicked() {
                                ed.request_fit();
                            }
                            icons::toggle(ui, Icon::Grid, &mut ed.show_grid, "Show grid");
                            if op.is_some() {
                                icons::toggle(ui, Icon::Annotate, &mut ed.annotate_op, "Annotate operating point");
                            } else {
                                icons::button(
                                    ui,
                                    Icon::Annotate,
                                    false,
                                    false,
                                    "Annotate operating point (run an OP simulation first)",
                                );
                            }
                            icons::divider(ui);
                            ui.label(RichText::new(ed.tool.name()).color(pal.schematic));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(RichText::new("▶ Simulate").color(pal.simulation)).on_hover_text("F5").clicked() {
                                    self.queue.push(Command::RunSimulation);
                                }
                            });
                        });
                    });
                    egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                        let out = ed.canvas(ui, sch, op.as_ref());
                        changed |= out.changed;
                        for req in out.requests {
                            self.queue.push(match req {
                                SchRequest::Simulate => Command::RunSimulation,
                                SchRequest::Check => Command::CheckSchematic(false),
                                SchRequest::ExportNetlist => Command::ExportNetlist,
                                SchRequest::PlaceCell => Command::PlaceCellDialog,
                                SchRequest::Descend(id) => Command::DescendSchematic(id),
                                SchRequest::Return => Command::ReturnSchematic,
                            });
                        }
                    });
                } else {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(pal.bg_canvas))
                        .show(ui, |ui| self.no_view_placeholder(ui, &r, ViewKind::Schematic));
                }
            }
            Workspace::Layout => {
                let Some(mut lay) = cell.layout.take() else {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(pal.bg_canvas))
                        .show(ui, |ui| self.no_view_placeholder(ui, &r, ViewKind::Layout));
                    return;
                };
                // The edited layout is taken out of the project while the editor runs, so the
                // project can resolve instances immutably (and cannot resolve the cell into itself).
                let l = &mut lay;
                let src: &Project = project;
                let placeable: Vec<CellRef> = src.all_cells().filter(|(_, c)| c.layout.is_some()).map(|(r, _)| r).collect();
                let context = eip_context(&self.lay_return, &r);
                let ed = self.lay_eds.entry(r.clone()).or_default();
                let orient = context.as_ref().map_or(ics_core::Trans::IDENTITY, |c| c.to_root);
                if ed.view.orient != orient {
                    ed.view.set_orient(orient);
                }
                let mut cx =
                    LayCtx { cell: &r, src, hier: &mut self.hier, tech: &tech, cells: &placeable, context: context.as_ref() };
                let mut open: Option<CellRef> = None;
                {
                    theme::aside(egui::Panel::left("lay_tools").resizable(true).default_size(230.0), ui, |ui| {
                        if let Some(o) = ed.layer_palette(ui, l, &mut cx) {
                            open = Some(o);
                        }
                    });
                    theme::aside(egui::Panel::right("lay_inspector").resizable(true).default_size(250.0), ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let (c, o) = ed.inspector(ui, l, &mut cx);
                            changed |= c;
                            open = open.take().or(o);
                        });
                    });
                    egui::Panel::top("lay_toolbar").show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let tip = theme::kbd("Undo (u / ⌘Z)");
                            let undo = icons::button(ui, Icon::Undo, false, ed.history.can_undo(), &tip);
                            let tip = theme::kbd("Redo (Shift+U / ⇧⌘Z)");
                            let redo = icons::button(ui, Icon::Redo, false, ed.history.can_redo(), &tip);
                            if (undo.clicked() && ed.undo(l)) || (redo.clicked() && ed.redo(l)) {
                                changed = true;
                                cx.hier.invalidate(&r);
                            }
                            icons::divider(ui);
                            for (i, t) in LayTool::ALL.into_iter().enumerate() {
                                // Groups: select / edit / create / modify / measure.
                                if [1, 4, 10, 12].contains(&i) {
                                    icons::divider(ui);
                                }
                                if icons::button(ui, t.icon(), ed.tool == t, true, &format!("{} ({})", t.name(), t.key()))
                                    .clicked()
                                {
                                    ed.set_tool(t);
                                }
                            }
                            icons::divider(ui);
                            if icons::button(ui, Icon::Fit, false, true, "Fit to window (F)").clicked() {
                                ed.request_fit();
                            }
                            if icons::button(ui, Icon::ZoomBox, false, true, "Zoom box: drag or click a box to zoom into (Z)")
                                .clicked()
                            {
                                ed.arm_zoom_box();
                            }
                            if icons::button(ui, Icon::PrevView, false, ed.can_step_view(true), "Previous view (W)").clicked() {
                                ed.step_view(true);
                            }
                            icons::divider(ui);
                            icons::toggle(ui, Icon::Grid, &mut ed.show_grid, "Show grid");
                            icons::toggle(
                                ui,
                                Icon::Gravity,
                                &mut ed.gravity,
                                "Gravity: snap the cursor to nearby vertices and edges (G)",
                            );
                            icons::toggle(
                                ui,
                                Icon::Drd,
                                &mut ed.drd,
                                "Design-rule-driven editing: show spacing and width violations while drawing and moving",
                            );
                            icons::toggle(
                                ui,
                                Icon::Magnet,
                                &mut ed.magnet,
                                "Magnet: pull drawn and moved shapes to exactly the minimum spacing from their neighbours",
                            );
                            if icons::toggle(
                                ui,
                                Icon::LiveDrc,
                                &mut ed.live.enabled,
                                "Live DRC: check the rule deck around every edit; Ctrl+. offers quick fixes",
                            )
                            .changed()
                            {
                                ed.live.reset();
                            }
                            icons::toggle(
                                ui,
                                Icon::Constraints,
                                &mut ed.show_constraints,
                                "Constraints: show symmetry axes, alignments and fixed items",
                            );
                            icons::toggle(
                                ui,
                                Icon::Flylines,
                                &mut ed.show_flylines,
                                "Flylines: unrouted connections from the schematic",
                            );
                            icons::divider(ui);
                            ed.snap_control(ui, &tech);
                            icons::divider(ui);
                            if icons::button(ui, Icon::Export, false, true, "Export this cell as GDSII").clicked() {
                                self.queue.push(Command::ExportGds);
                            }
                            icons::divider(ui);
                            ui.label(RichText::new(ed.tool.name()).color(pal.layout));
                            ui.label(RichText::new(format!("on {}", tech.layers[ed.active_layer].name)).color(pal.text_weak));
                            if let Some(c) = cx.context {
                                icons::divider(ui);
                                ui.label(RichText::new(format!("Editing in place in {}", c.root.cell)).color(pal.accent));
                                if icons::button(ui, Icon::Return, false, true, "Return: leave edit-in-place (Shift+B)").clicked()
                                {
                                    ed.requests.push(LayRequest::Return);
                                }
                            }
                        });
                    });
                    egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                        changed |= ed.canvas(ui, l, &mut cx).changed;
                    });
                }
                let mut log: Vec<(bool, String)> = vec![];
                let mut touched = false;
                let mut handoff: Option<(CellRef, View2D)> = None;
                let mut refit: Option<CellRef> = None;
                for req in std::mem::take(&mut ed.requests) {
                    match req {
                        LayRequest::Open(c) => open = Some(c),
                        LayRequest::Descend { cell, inst, elem, trans, in_place: true } => {
                            handoff = Some((cell.clone(), View2D { orient: ed.view.orient.compose(&trans), ..ed.view }));
                            self.lay_return.push(Descent {
                                parent: r.clone(),
                                child: cell.clone(),
                                at: Some((inst, elem, trans)),
                            });
                            log.push((true, format!("Editing {cell} in place (Shift+B returns)")));
                            open = Some(cell);
                        }
                        LayRequest::Descend { cell, in_place: false, .. } => {
                            self.lay_return.push(Descent { parent: r.clone(), child: cell.clone(), at: None });
                            log.push((true, format!("Descended into {cell} (Shift+B returns)")));
                            refit = Some(cell.clone());
                            open = Some(cell);
                        }
                        LayRequest::Return => match self.lay_return.pop() {
                            Some(d) => {
                                if let Some((_, _, t)) = d.at.filter(|_| d.child == r) {
                                    handoff = Some((
                                        d.parent.clone(),
                                        View2D { orient: ed.view.orient.compose(&t.inverse()), ..ed.view },
                                    ));
                                }
                                open = Some(d.parent);
                            }
                            None => log.push((false, "Return: already at the top cell".into())),
                        },
                        LayRequest::OpenRuleDesigner => self.queue.push(Command::RuleDesigner),
                        LayRequest::UpdateFromSource(remove) => self.queue.push(Command::UpdateFromSource(remove)),
                        LayRequest::EditLayerDisplay(name) => self.queue.push(Command::LayerDisplay(Some(name))),
                        LayRequest::SetDeck(text) => match project.set_drc_deck(text) {
                            Ok(msg) => {
                                changed = true;
                                log.push((true, msg));
                            }
                            Err(e) => log.push((false, e)),
                        },
                        LayRequest::ReplaceWithPCell { shape, pcell } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                lay.remove(shape);
                                ed.selection = [lay.add_instance(cr, ics_core::Trans::IDENTITY, None)].into();
                                touched = true;
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::PlacePCell { pcell, trans, array } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                ed.selection = [lay.add_instance(cr, trans, array)].into();
                                touched = true;
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::SetPCell { inst, pcell, trans } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                if let Some(i) = lay.instance_mut(inst) {
                                    i.cell = cr;
                                    i.trans = trans.unwrap_or(i.trans);
                                    touched = true;
                                }
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::MakeCell { ids, origin } => {
                            let mut k = 1;
                            let name = loop {
                                let n = format!("{}_sub{k}", r.cell);
                                if project.cell(&CellRef::new(&r.lib, &n)).is_none() {
                                    break n;
                                }
                                k += 1;
                            };
                            ed.history.push(&lay);
                            let mut c = Cell::new(&name);
                            c.description = format!("Extracted from {}", r.cell);
                            c.layout = Some(lay.extract(&ids, origin));
                            match project.add_cell(&r.lib, c) {
                                Ok(cr) => {
                                    log.push((true, format!("Moved {} item(s) into new cell {cr}", ids.len())));
                                    ed.selection = [lay.add_instance(cr, ics_core::Trans::new(origin.0, origin.1), None)].into();
                                    touched = true;
                                }
                                Err(e) => {
                                    ed.history.undo(&mut lay);
                                    log.push((false, e));
                                }
                            }
                        }
                    }
                }
                if touched {
                    ed.layout_changed();
                    self.hier.invalidate(&r);
                    changed = true;
                }
                if let Some((c, v)) = handoff {
                    self.lay_eds.entry(c).or_default().adopt_view(v);
                }
                if let Some(c) = refit {
                    self.lay_eds.entry(c).or_default().request_fit();
                }
                if let Some(c) = project.cell_mut(&r) {
                    c.layout = Some(lay);
                }
                for (ok, m) in log {
                    if ok {
                        self.ok(m);
                    } else {
                        self.error(m);
                    }
                }
                if let Some(o) = open {
                    self.queue.push(Command::OpenCell(o, Some(Workspace::Layout)));
                }
            }
            Workspace::Simulation => {
                if cell.schematic.is_none() {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(pal.bg_canvas))
                        .show(ui, |ui| self.no_view_placeholder(ui, &r, ViewKind::Schematic));
                    return;
                }
                let st = self.sims.entry(r.clone()).or_default();
                let mut run = false;
                let mut tool = None;
                theme::aside(egui::Panel::left("sim_setup").resizable(true).default_size(270.0), ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for (label, hint, cmd) in [
                            ("Sweep...", "Corner and parametric sweeps", Command::SweepManager),
                            ("Monte Carlo...", "Process and mismatch statistics", Command::MonteCarlo),
                            ("Optimizer...", "Size devices to meet specs", Command::Optimizer),
                            ("Verify...", "Run every test of the project and check its specs", Command::Verification),
                        ] {
                            if ui.small_button(label).on_hover_text(hint).clicked() {
                                tool = Some(cmd);
                            }
                        }
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let (c, rr) = sim_view::setup_panel(ui, cell, st, &self.ngspice, tech.models.as_ref());
                        changed |= c;
                        run = rr;
                    });
                });
                if run {
                    self.queue.push(Command::RunSimulation);
                }
                self.queue.extend(tool);
                let deck = st.show_netlist.then(|| {
                    let nl = ics_core::netlist::netlist_for(cell.schematic.as_ref().unwrap(), &tech, &cell.sim.corner);
                    nl.deck(&r.to_string(), &cell.sim)
                });
                if let Some(deck) = deck {
                    theme::aside(egui::Panel::right("sim_netlist").resizable(true).default_size(340.0), ui, |ui| {
                        theme::section(ui, "Netlist");
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(deck).monospace().size(11.5)).selectable(true));
                        });
                    });
                }
                let mut msg = None;
                egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_canvas).inner_margin(8)).show(ui, |ui| {
                    msg = sim_view::results_panel(ui, st);
                });
                if let Some(m) = msg {
                    self.info(m);
                }
            }
            Workspace::Home => unreachable!(),
        }
        if changed {
            self.mark_dirty();
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            let c = self.workspace.color(&pal);
            let (r, p) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
            icons::paint(&p, r.rect, self.workspace.icon(), c);
            ui.label(RichText::new(self.workspace.name()).small().color(c));
            ui.separator();
            if let Some(r) = &self.active {
                match self.workspace {
                    Workspace::Schematic => {
                        if let Some(ed) = self.sch_eds.get(r) {
                            if let Some(c) = ed.cursor {
                                ui.label(RichText::new(format!("x {}  y {}", c.x, c.y)).small().monospace());
                            }
                            ui.label(
                                RichText::new(format!("zoom {:.0}%", ed.view.scale / 22.0 * 100.0)).small().color(pal.text_weak),
                            );
                            ui.label(RichText::new(format!("{} selected", ed.selection.len())).small().color(pal.text_weak));
                            if matches!(ed.tool, SchTool::Wire) {
                                ui.label(
                                    RichText::new("click to route · / flips corner · Esc ends").small().color(pal.text_weak),
                                );
                            }
                        }
                    }
                    Workspace::Layout => {
                        if let (Some(ed), Some(p)) = (self.lay_eds.get(r), &self.project) {
                            if let Some(c) = ed.cursor {
                                ui.label(
                                    RichText::new(format!("X: {}  Y: {}", um(c.0, &p.tech), um(c.1, &p.tech)))
                                        .small()
                                        .monospace(),
                                );
                                if let Some(a) = ed.anchor() {
                                    let (dx, dy) = (c.0 - a.0, c.1 - a.1);
                                    let dist = ((dx as f64).hypot(dy as f64)).round() as i64;
                                    ui.label(
                                        RichText::new(format!(
                                            "dX: {}  dY: {}  Dist: {}",
                                            um(dx, &p.tech),
                                            um(dy, &p.tech),
                                            um(dist, &p.tech)
                                        ))
                                        .small()
                                        .monospace()
                                        .color(pal.text_weak),
                                    );
                                }
                            }
                            let sel = if ed.partial_mode || !ed.part.is_empty() {
                                format!("Sel(P): {}", ed.part.len())
                            } else {
                                format!("Sel: {}", ed.selection.len())
                            };
                            ui.label(RichText::new(sel).small().monospace().color(pal.text_weak));
                            ui.label(RichText::new(format!("Cmd: {}", ed.tool.name())).small().monospace().color(pal.layout));
                            ui.separator();
                            let [ml, mm, mr] = ed.mouse_hints();
                            ui.label(RichText::new(format!("mouse L: {ml}   M: {mm}   R: {mr}")).small().color(pal.text_weak))
                                .on_hover_text("What each mouse button does now");
                            let s = ed.stats;
                            let txt = format!(
                                "{} shapes drawn{}",
                                fmt_count(s.drawn as u64),
                                if s.truncated { " (limit)" } else { "" }
                            );
                            ui.label(RichText::new(txt).small().color(if s.truncated { pal.warning } else { pal.text_weak }));
                            if let Some((what, t)) = ed.verify.running() {
                                ui.separator();
                                ui.add(egui::Spinner::new().size(12.0));
                                ui.label(RichText::new(format!("Running {}… {:.1} s", what.name(), t.as_secs_f64())).small());
                            }
                        }
                    }
                    _ => {}
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).small().color(pal.text_weak));
                if let Some(r) = self.updater.available() {
                    let txt = match self.updater.state {
                        update::State::Installed { .. } => "Restart to update".to_string(),
                        _ => format!("Update {} available", r.version),
                    };
                    if ui.link(RichText::new(txt).small().color(pal.accent)).clicked() {
                        self.queue.push(Command::CheckForUpdates);
                    }
                }
                ui.separator();
                if let Some(b) = &self.bridge {
                    let hover = match &b.last {
                        Some((t, tool)) => {
                            format!("{} tool call(s) from ics mcp; last: {tool}, {:.0} s ago", b.calls, t.elapsed().as_secs_f64())
                        }
                        None => "Waiting for tool calls from ics mcp (Claude Code)".to_string(),
                    };
                    ui.label(RichText::new("● MCP live").small().color(pal.ok)).on_hover_text(hover);
                    ui.separator();
                    if let Some(j) = &b.job {
                        let queued = if b.waiting.is_empty() { String::new() } else { format!(" ({} queued)", b.waiting.len()) };
                        let txt = format!("MCP: {} running… {:.1} s{queued}", j.tool, j.started.elapsed().as_secs_f64());
                        ui.label(RichText::new(txt).small());
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.separator();
                    }
                }
                let (txt, col) = match &self.ngspice {
                    Some(v) => (format!("● {v}"), pal.ok),
                    None => ("○ ngspice not found".to_string(), pal.text_weak),
                };
                ui.label(RichText::new(txt).small().color(col));
                ui.separator();
                if let Some(p) = &self.project {
                    ui.label(RichText::new(format!("PDK {}", p.tech.name)).small().color(pal.text_weak));
                    ui.separator();
                }
                if ui
                    .add(
                        egui::Label::new(RichText::new(if self.show_console { "▾ Console" } else { "▸ Console" }).small())
                            .sense(Sense::click()),
                    )
                    .clicked()
                {
                    self.show_console = !self.show_console;
                }
                // A warning the hidden console would swallow: show it here for a few seconds.
                let console_shown = self.show_console && self.bottom_tab == BottomTab::Console;
                if let Some((text, at)) = &self.flash
                    && !console_shown
                {
                    let age = at.elapsed();
                    if age < FLASH_TIME {
                        let l = ui
                            .add(
                                egui::Label::new(RichText::new(format!("⚠ {text}")).small().color(pal.warning))
                                    .truncate()
                                    .sense(Sense::click()),
                            )
                            .on_hover_text("Click to show the console");
                        if l.clicked() {
                            self.show_console = true;
                            self.bottom_tab = BottomTab::Console;
                        }
                        ui.ctx().request_repaint_after(FLASH_TIME - age);
                    }
                }
            });
        });
    }

    fn console(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let findings = self.active_findings();
        let problems: Vec<String> = self
            .netlist_active()
            .map(|(_, w)| w)
            .unwrap_or_default()
            .into_iter()
            .filter(|w| !w.starts_with("Unconnected pin") && !w.starts_with("Duplicate instance name"))
            .collect();
        let count = problems.len() + findings.iter().filter(|f| f.severity != ics_core::sch_check::Severity::Info).count();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Console, "Console");
            if ui
                .selectable_value(&mut self.bottom_tab, BottomTab::Ciw, "CIW")
                .on_hover_text("SKILL command interpreter")
                .clicked()
            {
                self.ciw.request_focus();
            }
            let ptxt = if count == 0 { "Problems".to_string() } else { format!("Problems ({count})") };
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Problems, ptxt);
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Netlist, "Netlist");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").clicked() {
                    self.show_console = false;
                }
                if self.bottom_tab == BottomTab::Console && ui.small_button("Clear").clicked() {
                    self.log.clear();
                }
                if self.bottom_tab == BottomTab::Ciw && ui.small_button("Clear").clicked() {
                    self.ciw.clear();
                }
            });
        });
        ui.separator();
        match self.bottom_tab {
            BottomTab::Ciw => {
                if let Some(src) = self.ciw.ui(ui) {
                    self.queue.push(Command::CiwEval(src));
                }
            }
            BottomTab::Console => {
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    for l in &self.log {
                        let (tag, c) = match l.level {
                            Level::Info => ("INFO", pal.text_weak),
                            Level::Ok => ("DONE", pal.ok),
                            Level::Warn => ("WARN", pal.warning),
                            Level::Error => ("FAIL", pal.danger),
                        };
                        ui.horizontal_top(|ui| {
                            ui.label(
                                RichText::new(format!("{:>7.2}s", l.t))
                                    .monospace()
                                    .small()
                                    .color(pal.text_weak.gamma_multiply(0.7)),
                            );
                            ui.label(RichText::new(tag).monospace().small().color(c));
                            ui.add(egui::Label::new(RichText::new(&l.text).monospace().size(12.0)).wrap());
                        });
                    }
                });
            }
            BottomTab::Problems => {
                let mut pick = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if problems.is_empty() && findings.is_empty() {
                        ui.label(RichText::new("No problems detected in the active schematic.").color(pal.text_weak));
                    }
                    for f in &findings {
                        use ics_core::sch_check::Severity;
                        let (tag, c) = match f.severity {
                            Severity::Error => ("ERROR", pal.danger),
                            Severity::Warning => ("WARN", pal.warning),
                            Severity::Info => ("INFO", pal.text_weak),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tag).monospace().small().color(c));
                            ui.label(RichText::new(f.code).monospace().small().color(pal.text_weak));
                            let r = ui.add(egui::Label::new(RichText::new(&f.message).color(c)).sense(egui::Sense::click()));
                            if r.on_hover_text("Select in the schematic").clicked() {
                                pick = Some(f.instances.clone());
                            }
                        });
                    }
                    for w in problems {
                        ui.label(RichText::new(format!("⚠ {w}")).color(pal.warning));
                    }
                });
                if let Some(ids) = pick
                    && let Some(r) = self.active.clone()
                {
                    self.sch_eds.entry(r).or_default().selection = ids.into_iter().map(SchObj::Instance).collect();
                    self.workspace = Workspace::Schematic;
                }
            }
            BottomTab::Netlist => {
                let deck = self.netlist_active().map(|d| d.0).unwrap_or_else(|| "* active cell has no schematic".into());
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(deck).monospace().size(12.0)).selectable(true));
                });
            }
        }
    }

    fn palette_commands(&self) -> Vec<(String, &'static str, Command)> {
        let examples =
            examples::TOPICS.iter().map(|t| (format!("Open example: {}", t.title), "", Command::OpenExample(t.name.into())));
        let mut v: Vec<(String, &'static str, Command)> = vec![
            ("Run simulation".into(), "F5", Command::RunSimulation),
            ("Save project".into(), "⌘S", Command::Save),
            ("Save project as…".into(), "⇧⌘S", Command::SaveAs),
            ("Open project…".into(), "⌘O", Command::OpenProjectDialog),
            ("New project…".into(), "", Command::NewProjectDialog),
            ("New demo project".into(), "", Command::NewDemoProject),
            ("Save example projects to a folder…".into(), "", Command::SaveExamples),
            ("New cell…".into(), "⌘N", Command::NewCellDialog),
            ("Run DRC".into(), "", Command::RunDrc),
            ("Extract nets".into(), "", Command::ExtractNets),
            ("Schematic from layout (pixels: PD / TG / RST / SF / SEL)".into(), "", Command::SchematicFromLayout),
            ("Pixel circuit: conversion gain, full well, lag, noise, dark current".into(), "", Command::PixelCircuit),
            ("Run LVS".into(), "", Command::RunLvs),
            ("IR drop / EM analysis".into(), "", Command::IrDrop(None)),
            ("DRC rule designer".into(), "", Command::RuleDesigner),
            ("PCell designer".into(), "", Command::PCellDesigner),
            ("PCell designer: make a PCell from the active layout".into(), "", Command::PCellFromActive),
            ("MOS symbol: body pin (4-terminal)".into(), "", Command::MosSymbol(true)),
            ("MOS symbol: arrow on source, body hidden (3-terminal)".into(), "", Command::MosSymbol(false)),
            ("DRC deck: import from a file".into(), "", Command::ImportDrcDeck(None)),
            ("DRC deck: export to a file".into(), "", Command::ExportDrcDeck(None)),
            ("DRC deck: link a shared deck file (common to several projects)".into(), "", Command::LinkDrcDeck(None)),
            ("DRC deck: reload the shared deck".into(), "", Command::ReloadDrcDeck),
            ("DRC deck: unlink the shared deck".into(), "", Command::UnlinkDrcDeck),
            ("Stream layer map (GDSII / OASIS layer numbers)".into(), "", Command::StreamMap),
            ("Layer display (fill, stipple, outline, line style)".into(), "", Command::LayerDisplay(None)),
            ("Dummy fill (metal density)".into(), "", Command::DummyFill),
            ("OPC (optical proximity correction)".into(), "", Command::Opc),
            ("Waveform calculator".into(), "", Command::Calculator),
            ("Pixel optics (QE, crosstalk, photocurrent)".into(), "", Command::PixelOptics),
            ("Run pixel optics model".into(), "", Command::RunPixelOptics),
            ("Pixel device model into the technology's photodiode".into(), "", Command::ApplyPixelDevice),
            ("Load measured QE…".into(), "", Command::LoadMeasuredQe(None)),
            ("Fit pixel stack to measured QE".into(), "", Command::FitPixelStack),
            ("Pixel stack of the open cell from the PDK cross section".into(), "", Command::PixelStackFromPdk),
            ("Corner / sweep manager".into(), "", Command::SweepManager),
            ("Verification (all tests)".into(), "", Command::Verification),
            ("Run verification".into(), "", Command::RunVerification),
            ("Monte Carlo (process and mismatch of device models)".into(), "", Command::MonteCarlo),
            ("Run Monte Carlo".into(), "", Command::RunMonteCarlo),
            ("Silicon correlation (measured curves, model fitting)".into(), "", Command::SiliconCorrelation),
            ("Process emulation (cross-section of the layout)".into(), "", Command::ProcessEmulation),
            ("Process data (WAT / PCM, SPC, corners)".into(), "", Command::ProcessData),
            ("Fit the correlation parameters to the measurements".into(), "", Command::FitSilicon),
            ("Stop Monte Carlo".into(), "", Command::StopMonteCarlo),
            ("Optimizer (size devices to meet specs)".into(), "", Command::Optimizer),
            ("Results browser (compare runs)".into(), "", Command::ResultsBrowser),
            ("Check schematic".into(), "", Command::CheckSchematic(false)),
            ("Place cell instance (hierarchical schematic)".into(), "Shift+I", Command::PlaceCellDialog),
            ("Return to the parent schematic".into(), "Shift+B", Command::ReturnSchematic),
            ("Update cell instance pins".into(), "", Command::UpdateCellInstances),
            ("Edit symbol pins (side and order)".into(), "", Command::EditSymbolDialog),
            ("Check and save".into(), "", Command::CheckSchematic(true)),
            ("Net browser".into(), "", Command::NetBrowser),
            ("Hierarchy browser".into(), "", Command::HierarchyBrowser),
            ("Create guard ring".into(), "", Command::GuardRing),
            ("Create via array".into(), "", Command::ViaArray),
            ("Matched placement (common centroid / interdigitated)".into(), "", Command::MatchedPlacement),
            ("Run corner / parametric sweep".into(), "", Command::RunSweep),
            ("Run sign-off runset (KLayout / Calibre / netgen)".into(), "", Command::RunSignoff(None)),
            ("Import sign-off results (Calibre / KLayout / netgen)".into(), "", Command::ImportSignoffResults(None)),
            ("Generate layout from schematic".into(), "", Command::GenerateLayout),
            ("Route flylines".into(), "", Command::RouteFlylines),
            ("Check layout against source (schematic)".into(), "", Command::CheckAgainstSource),
            ("Update components and nets from schematic".into(), "", Command::UpdateFromSource(false)),
            ("Update from schematic and delete extra devices".into(), "", Command::UpdateFromSource(true)),
            ("Export cell GDSII…".into(), "", Command::ExportGds),
            ("Export library GDSII…".into(), "", Command::ExportLibraryGds),
            ("Export cell OASIS…".into(), "", Command::ExportOasis),
            ("Export library OASIS…".into(), "", Command::ExportLibraryOasis),
            ("Import GDSII / OASIS…".into(), "", Command::ImportGds(None)),
            ("Import GDSII / OASIS with Virtuoso layer map…".into(), "", Command::ImportGdsWithLayerMap(None, None)),
            ("Export cell LEF abstract…".into(), "", Command::ExportLefDef(false)),
            ("Export cell DEF…".into(), "", Command::ExportLefDef(true)),
            ("Import LEF abstracts…".into(), "", Command::ImportLefDef(false, None)),
            ("Import DEF placement…".into(), "", Command::ImportLefDef(true, None)),
            ("Import technology (KLayout .lyp)…".into(), "", Command::ImportTech),
            ("Load PDK…".into(), "", Command::LoadPdk(None)),
            ("Load SKILL (.il)…".into(), "", Command::LoadSkill(None)),
            ("Manage extensions…".into(), "", Command::Extensions),
            ("Reload extensions".into(), "", Command::ReloadExtensions),
            ("SKILL command interpreter (CIW)".into(), "", Command::Ciw),
            ("Design copilot (AI chat)".into(), "", Command::Copilot),
            ("AI models (provider, model, API key)…".into(), "", Command::AiModels),
            ("Export SPICE netlist…".into(), "", Command::ExportNetlist),
            ("Import CDL netlist (Virtuoso)…".into(), "", Command::ImportCdl(None)),
            ("Import Virtuoso library (dump or cds.lib)…".into(), "", Command::ImportVirtuoso(None)),
            ("Save Virtuoso dump script (icsDump.il)…".into(), "", Command::SaveDumpSkill(None)),
            ("Export Virtuoso schematic (SKILL)…".into(), "", Command::ExportSkill),
            ("Toggle light / dark theme".into(), "", Command::ToggleTheme),
            ("Toggle console".into(), "⌘J", Command::ToggleConsole),
            ("Fit view".into(), "F", Command::FitView),
            ("Settings".into(), "⌘,", Command::Settings),
            ("Keyboard shortcuts".into(), "F1", Command::Shortcuts),
            ("Start the interactive tutorial".into(), "", Command::Tutorial),
            ("Open user guide (web)".into(), "", Command::UserGuide),
            ("About IC Studio".into(), "", Command::About),
            ("Check for updates".into(), "", Command::CheckForUpdates),
            ("Add IC Studio MCP server to Claude Code".into(), "", Command::AddMcpToClaudeCode),
            ("Copy MCP server config".into(), "", Command::CopyMcpConfig),
        ];
        v.extend(examples);
        if let Some(r) = &self.active {
            v.push((format!("Copy {r} as a new cell (start a new design from it)…"), "", Command::CopyCellDialog(r.clone())));
            v.push((
                format!("Derive a new design from {r} (new targets, sized and laid out)…"),
                "",
                Command::DeriveDialog(r.clone()),
            ));
        }
        if crate::i18n::korean_available() {
            v.push(("Language: English".into(), "", Command::SetLanguage(crate::i18n::Lang::En)));
            v.push((
                format!("Language: Korean ({})", crate::i18n::Lang::Ko.label()),
                "",
                Command::SetLanguage(crate::i18n::Lang::Ko),
            ));
        }
        v.push(if self.settings.beginner {
            ("Beginner mode: off (show every tool in the menus)".into(), "", Command::BeginnerMode(false))
        } else {
            ("Beginner mode: on (menus with the everyday design flow)".into(), "", Command::BeginnerMode(true))
        });
        for (e, c) in self.extensions.commands() {
            v.push((format!("{}: {}", e.manifest.extension.name, c.title), "", Command::RunExtension(e.id().to_string(), c.id)));
        }
        for (label, file) in self.extensions.pdks() {
            v.push((format!("Load PDK: {label}"), "", Command::LoadPdk(Some(file))));
        }
        if self.workspace == Workspace::Schematic && self.active.is_some() {
            for a in SchAction::PALETTE {
                v.push((format!("Schematic: {}", a.name()), "", Command::Sch(*a)));
            }
        }
        if self.workspace == Workspace::Layout && self.active.is_some() {
            for (menu, items) in LayAction::MENUS {
                for a in items.iter().flatten() {
                    v.push((format!("Layout {menu}: {}", a.name()), a.key(), Command::Lay(*a)));
                }
            }
        }
        for (i, w) in Workspace::ALL.iter().enumerate() {
            let sc: &'static str = ["⌘1", "⌘2", "⌘3", "⌘4"][i];
            v.push((format!("Go to {} workspace", w.name()), sc, Command::SetWorkspace(*w)));
        }
        if let Some(p) = &self.project
            && let Some(lib) = self.palette_library()
            && let Some(l) = p.libraries.iter().find(|l| l.name == lib)
        {
            let name = |what: &str| format!("Library {lib}: {what}");
            v.push((name("new cell…"), "", Command::NewCellInLibrary(lib.clone())));
            v.push((name("export GDSII…"), "", Command::ExportLibraryStream(lib.clone(), StreamFormat::Gds)));
            v.push((name("export OASIS…"), "", Command::ExportLibraryStream(lib.clone(), StreamFormat::Oasis)));
            let (storage, how) = match l.storage {
                LibStorage::Json => (LibStorage::Gds, "store as one GDSII file"),
                LibStorage::Gds => (LibStorage::Json, "store as JSON cell directories"),
            };
            v.push((name(how), "", Command::SetLibraryStorage(lib.clone(), storage)));
            v.push((name("rename…"), "", Command::RenameLibraryDialog(lib.clone())));
            v.push((name("delete…"), "", Command::DeleteLibraryConfirm(lib.clone())));
            for t in p.library_tops(&lib) {
                v.push((
                    format!("Library {lib}: open top cell {} · layout", t.cell),
                    "",
                    Command::OpenCell(t, Some(Workspace::Layout)),
                ));
            }
        }
        if let Some(p) = &self.project {
            for (r, c) in p.all_cells() {
                if c.schematic.is_some() {
                    v.push((format!("Open {} · schematic", r), "", Command::OpenCell(r.clone(), Some(Workspace::Schematic))));
                }
                if c.layout.is_some() {
                    v.push((format!("Open {} · layout", r), "", Command::OpenCell(r.clone(), Some(Workspace::Layout))));
                }
            }
        }
        v
    }

    /// The library that library commands of the palette act on: the one selected in the
    /// Explorer, else the active cell's.
    fn palette_library(&self) -> Option<String> {
        let p = self.project.as_ref()?;
        self.selected_lib
            .clone()
            .filter(|l| p.libraries.iter().any(|x| &x.name == l))
            .or_else(|| self.active.as_ref().map(|r| r.lib.clone()))
    }

    fn command_palette(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.palette.take() else { return };
        let pal = theme::palette(ctx);
        let q = st.query.to_lowercase();
        let items: Vec<(String, &'static str, Command)> = self
            .palette_commands()
            .into_iter()
            .filter(|(n, _, _)| q.split_whitespace().all(|w| n.to_lowercase().contains(w)))
            .collect();
        let mut close = false;
        let mut chosen: Option<Command> = None;
        let (up, down, enter, esc) = ctx.input(|i| {
            (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Enter), i.key_pressed(Key::Escape))
        });
        if down {
            st.sel = (st.sel + 1).min(items.len().saturating_sub(1));
        }
        if up {
            st.sel = st.sel.saturating_sub(1);
        }
        if esc {
            close = true;
        }
        if enter && let Some(it) = items.get(st.sel) {
            chosen = Some(it.2.clone());
        }
        let modal = egui::Modal::new(egui::Id::unique("cmd_palette")).show(ctx, |ui| {
            ui.set_width(520.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .hint_text("Type a command or cell name…")
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(15.0)),
            );
            if !r.has_focus() {
                r.request_focus();
            }
            if r.changed() {
                st.sel = 0;
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, (name, sc, cmd)) in items.iter().enumerate() {
                    let selected = i == st.sel;
                    let (resp, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 26.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, name));
                    if selected || resp.hovered() {
                        p.rect_filled(resp.rect, 5.0, if selected { pal.accent.gamma_multiply(0.3) } else { pal.bg_hover });
                    }
                    p.text(
                        resp.rect.left_center() + Vec2::new(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(13.5),
                        pal.text,
                    );
                    p.text(
                        resp.rect.right_center() - Vec2::new(10.0, 0.0),
                        Align2::RIGHT_CENTER,
                        theme::kbd(sc),
                        FontId::monospace(11.5),
                        pal.text_weak,
                    );
                    if selected {
                        resp.scroll_to_me(None);
                    }
                    if resp.clicked() {
                        chosen = Some(cmd.clone());
                    }
                }
                if items.is_empty() {
                    ui.label(RichText::new("No matching commands").color(pal.text_weak));
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(c) = chosen {
            self.queue.push(c);
            close = true;
        }
        if !close {
            self.palette = Some(st);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dialog.take() else { return };
        let pal = theme::palette(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            match &mut dlg {
                Dialog::NewProject { name, dir, demo, error } => {
                    ui.heading(t("New project"));
                    ui.add_space(8.0);
                    egui::Grid::new("np").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(t("Name"));
                        ui.text_edit_singleline(name);
                        ui.end_row();
                        ui.label(t("Location"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(dir).desired_width(220.0));
                            if ui.button(t("…")).clicked()
                                && let Some(d) = rfd::FileDialog::new().pick_folder()
                            {
                                *dir = d.display().to_string();
                            }
                        });
                        ui.end_row();
                        ui.label(t("Template"));
                        ui.horizontal(|ui| {
                            ui.radio_value(demo, false, t("Empty"));
                            ui.radio_value(demo, true, t("Demo designs"));
                        });
                        ui.end_row();
                    });
                    ui.label(
                        RichText::new(tf("Will be created at {}", &[&Path::new(dir.as_str()).join(name.as_str()).display().to_string()]))
                            .small()
                            .color(pal.text_weak),
                    );
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Create")).strong()).clicked() {
                            if !ics_core::project::valid_name(name) {
                                *error = "Name may contain letters, digits, _ and - only".into();
                            } else {
                                let root = Path::new(dir.as_str()).join(name.as_str());
                                if root.join(ics_core::project::MANIFEST).exists() {
                                    *error = "A project already exists there".into();
                                } else {
                                    let mut p = if *demo { examples::demo_project() } else { Project::new(name.as_str()) };
                                    p.name = name.clone();
                                    match p.save_as(&root) {
                                        Ok(()) => {
                                            self.set_project(p);
                                            self.remember_recent(&root);
                                            self.ok(format!("Created project at {}", root.display()));
                                            close = true;
                                        }
                                        Err(e) => *error = e.to_string(),
                                    }
                                }
                            }
                        }
                    });
                }
                Dialog::NewCell { lib, name, description, schematic, layout, error } => {
                    ui.heading(t("New cell"));
                    ui.add_space(8.0);
                    egui::Grid::new("nc").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(t("Library"));
                        egui::ComboBox::from_id_salt("nc_lib").selected_text(lib.as_str()).show_ui(ui, |ui| {
                            if let Some(p) = &self.project {
                                for l in &p.libraries {
                                    ui.selectable_value(lib, l.name.clone(), &l.name);
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(t("Cell name"));
                        let r = ui.text_edit_singleline(name);
                        if name.is_empty() && !r.has_focus() {
                            r.request_focus();
                        }
                        ui.end_row();
                        ui.label(t("Description"));
                        ui.text_edit_singleline(description);
                        ui.end_row();
                        ui.label(t("Views"));
                        ui.horizontal(|ui| {
                            ui.checkbox(schematic, t("schematic"));
                            ui.checkbox(layout, t("layout"));
                        });
                        ui.end_row();
                    });
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Create")).strong()).clicked() || enter {
                            let mut c = Cell::new(name.trim());
                            c.description = description.clone();
                            if *schematic {
                                c.schematic = Some(Schematic::default());
                            }
                            if *layout {
                                c.layout = Some(Layout::default());
                            }
                            let ws = if *schematic { Workspace::Schematic } else { Workspace::Layout };
                            match self.project.as_mut().map(|p| {
                                if !p.libraries.iter().any(|l| l.name == *lib) {
                                    p.add_library(lib)?;
                                }
                                p.add_cell(lib, c)
                            }) {
                                Some(Ok(r)) => {
                                    self.info(format!("Created cell {r}"));
                                    self.dirty = true;
                                    self.queue.push(Command::OpenCell(r, Some(ws)));
                                    close = true;
                                }
                                Some(Err(e)) => *error = e,
                                None => close = true,
                            }
                        }
                    });
                }
                Dialog::RenameCell { cell, name, error } => {
                    ui.heading(t("Rename cell"));
                    ui.text_edit_singleline(name);
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(t("Rename")).clicked()
                            && let Some(p) = self.project.as_mut()
                        {
                            match p.rename_cell(cell, name.trim()) {
                                Ok(new) => {
                                    self.hier.clear();
                                    let old = cell.clone();
                                    for t in &mut self.tabs {
                                        if *t == old {
                                            *t = new.clone();
                                        }
                                    }
                                    if self.active.as_ref() == Some(&old) {
                                        self.active = Some(new.clone());
                                    }
                                    if let Some(e) = self.sch_eds.remove(&old) {
                                        self.sch_eds.insert(new.clone(), e);
                                    }
                                    if let Some(e) = self.lay_eds.remove(&old) {
                                        self.lay_eds.insert(new.clone(), e);
                                    }
                                    if let Some(e) = self.sims.remove(&old) {
                                        self.sims.insert(new.clone(), e);
                                    }
                                    self.dirty = true;
                                    close = true;
                                }
                                Err(e) => *error = e,
                            }
                        }
                    });
                }
                Dialog::CopyCell { from, lib, name, error } => {
                    ui.heading(t("Copy as new cell"));
                    ui.label(
                        RichText::new(tf(
                            "Start a new design from {}: the schematic, layout, simulation setup, specs and notes are copied.",
                            &[&from.to_string()],
                        ))
                        .color(pal.text_weak),
                    );
                    ui.add_space(8.0);
                    egui::Grid::new("cc").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        let l = ui.label(t("Library"));
                        egui::ComboBox::from_id_salt("cc_lib")
                            .selected_text(lib.as_str())
                            .show_ui(ui, |ui| {
                                if let Some(p) = &self.project {
                                    for l in p.libraries.iter().filter(|l| l.name != ics_core::pcell::PCELL_LIB) {
                                        ui.selectable_value(lib, l.name.clone(), &l.name);
                                    }
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label(t("New name"));
                        ui.text_edit_singleline(name).labelled_by(l.id);
                        ui.end_row();
                    });
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Copy")).strong()).clicked()
                            && let Some(p) = self.project.as_mut()
                        {
                            match p.copy_cell(from, lib, name.trim()) {
                                Ok(new) => {
                                    let ws = if p.cell(&new).is_some_and(|c| c.schematic.is_some()) {
                                        Workspace::Schematic
                                    } else {
                                        Workspace::Layout
                                    };
                                    self.dirty = true;
                                    self.ok(format!("Copied {from} to {new}"));
                                    self.queue.push(Command::OpenCell(new, Some(ws)));
                                    close = true;
                                }
                                Err(e) => *error = e,
                            }
                        }
                    });
                }
                Dialog::ConfirmDelete(r) => {
                    ui.heading("Delete cell?");
                    ui.label(format!("'{r}' and all of its views will be deleted; its files go when the project is saved."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new("Delete").color(pal.danger)).clicked() {
                            let r = r.clone();
                            if let Some(p) = self.project.as_mut() {
                                self.hier.clear();
                                match p.delete_cell(&r) {
                                    Ok(()) => self.warn(format!("Deleted {r}")),
                                    Err(e) => self.error(format!("Delete failed: {e}")),
                                }
                            }
                            self.queue.push(Command::CloseTab(r));
                            self.dirty = true;
                            close = true;
                        }
                    });
                }
                Dialog::RenameLibrary { lib, name, error } => {
                    ui.heading("Rename library");
                    let l = ui.label("New name");
                    let r = ui.text_edit_singleline(name).labelled_by(l.id);
                    if !r.has_focus() && error.is_empty() && name == lib {
                        r.request_focus();
                    }
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if (ui.button("Rename").clicked() || enter)
                            && let Some(p) = self.project.as_mut()
                        {
                            let new = name.trim().to_string();
                            match p.rename_library(lib, &new) {
                                Ok(()) => {
                                    let old = lib.clone();
                                    self.library_changed(&old, Some(&new));
                                    self.info(format!("Renamed library '{old}' to '{new}'"));
                                    self.dirty = true;
                                    close = true;
                                }
                                Err(e) => *error = e,
                            }
                        }
                    });
                }
                Dialog::ConfirmDeleteLibrary(lib) => {
                    ui.heading("Delete library?");
                    let (cells, users) = self.project.as_ref().map_or((0, 0), |p| {
                        (p.libraries.iter().find(|l| &l.name == lib).map_or(0, |l| l.cells.len()), p.library_users(lib))
                    });
                    ui.label(format!(
                        "Library '{lib}' and its {cells} cell(s) will be deleted; the files go when the project is saved."
                    ));
                    if users > 0 {
                        ui.colored_label(pal.danger, format!("{users} instance(s) in other libraries place its cells."));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new("Delete").color(pal.danger)).clicked() {
                            let lib = lib.clone();
                            if let Some(p) = self.project.as_mut() {
                                match p.delete_library(&lib) {
                                    Ok(()) => {
                                        self.library_changed(&lib, None);
                                        self.warn(format!("Deleted library '{lib}'"));
                                    }
                                    Err(e) => self.error(format!("Delete failed: {e}")),
                                }
                            }
                            self.dirty = true;
                            close = true;
                        }
                    });
                }
                Dialog::Unsaved { then } => {
                    ui.heading("Unsaved changes");
                    ui.label("The current project has unsaved changes.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new("Discard").color(pal.danger)).clicked() {
                            self.dirty = false;
                            if let Some(root) = self.project.as_ref().and_then(|p| p.root.as_deref()) {
                                crate::recovery::discard(root);
                            }
                            self.queue.push((**then).clone());
                            close = true;
                        }
                        if ui.button(RichText::new("Save").strong()).clicked() {
                            if self.save(false) {
                                self.queue.push((**then).clone());
                            } else if let Some(Dialog::SaveConflict { then: after, .. }) = &mut self.dialog {
                                *after = Some(then.clone());
                            }
                            close = true;
                        }
                    });
                }
                Dialog::PlaceCell { filter } => {
                    ui.heading("Place cell instance");
                    let l = ui.label("Filter");
                    ui.text_edit_singleline(filter).labelled_by(l.id);
                    let here = self.active.clone();
                    let f = filter.to_ascii_lowercase();
                    let cells: Vec<(CellRef, usize)> = self
                        .project
                        .as_ref()
                        .map(|p| {
                            p.all_cells()
                                .filter(|(r, c)| {
                                    Some(r) != here.as_ref()
                                        && r.lib != ics_core::pcell::PCELL_LIB
                                        && c.schematic.as_ref().is_some_and(|s| !s.ports().is_empty())
                                        && r.to_string().to_ascii_lowercase().contains(&f)
                                })
                                .map(|(r, c)| {
                                    let n = c.schematic.as_ref().map_or(0, |s| s.ports().len());
                                    (r, n)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    ui.label(
                        RichText::new("Cells whose schematic has pins (labels marked as pins)").small().color(pal.text_weak),
                    );
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        if cells.is_empty() {
                            ui.label(RichText::new("No cell with schematic pins").small());
                        }
                        for (r, n) in cells {
                            if ui.button(format!("{r}  ({n} pins)")).clicked() {
                                self.queue.push(Command::PlaceCell(r));
                                close = true;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                }
                Dialog::EditSymbol { cell, pins } => {
                    use ics_core::schematic::{PinSide, SymbolSpec};
                    ui.heading(format!("Symbol pins of {cell}"));
                    ui.label(RichText::new("The side and order of each pin on the box other schematics place").small().color(pal.text_weak));
                    let mut swap: Option<(usize, usize)> = None;
                    egui::Grid::new("symbol_pins").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                        let n = pins.len();
                        for (k, (name, side)) in pins.iter_mut().enumerate() {
                            let l = ui.label(RichText::new(name.as_str()).monospace());
                            egui::ComboBox::from_id_salt(("pin_side", k))
                                .selected_text(side.name())
                                .show_ui(ui, |ui| {
                                    for s in PinSide::ALL {
                                        ui.selectable_value(side, s, s.name());
                                    }
                                })
                                .response
                                .labelled_by(l.id);
                            if ui.add_enabled(k > 0, egui::Button::new("Up")).clicked() {
                                swap = Some((k, k - 1));
                            }
                            if ui.add_enabled(k + 1 < n, egui::Button::new("Down")).clicked() {
                                swap = Some((k, k + 1));
                            }
                            ui.end_row();
                        }
                    });
                    if let Some((a, b)) = swap {
                        pins.swap(a, b);
                    }
                    // Preview of the box.
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(360.0, 160.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 6.0, pal.bg_canvas);
                    let ghost = ics_core::Instance {
                        id: 0,
                        kind: DeviceKind::Cell,
                        name: String::new(),
                        pos: ics_core::Pt::new(0, 0),
                        orient: Default::default(),
                        params: Default::default(),
                        cell: Some(cell.clone()),
                        ports: pins.iter().map(|(n, _)| (n.clone(), PortDir::InOut)).collect(),
                        symbol: Some(SymbolSpec { pins: pins.clone() }),
                    };
                    let b = ghost.bbox();
                    let scale = ((rect.width() - 40.0) / (b.2 - b.0)).min((rect.height() - 30.0) / (b.3 - b.1)).clamp(4.0, 40.0);
                    let mut view = crate::canvas::View2D::new(scale as f64, false, 1.0, 100.0);
                    view.center = (((b.0 + b.2) / 2.0) as f64, ((b.1 + b.3) / 2.0) as f64);
                    crate::schematic_view::draw_symbol(ui.painter(), &view, rect, &ghost, pal.symbol, 1.4);
                    for (pin, at) in ghost.pin_positions() {
                        let q = view.to_screen(rect, at.x as f64, at.y as f64);
                        ui.painter().text(q, egui::Align2::CENTER_BOTTOM, pin, egui::FontId::proportional(11.0), pal.text_weak);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        let reset = ui.button("Default").clicked();
                        if (ui.button(RichText::new("Apply").strong()).clicked() || reset)
                            && let Some(p) = self.project.as_mut()
                            && let Some(sch) = p.cell_mut(cell).and_then(|c| c.schematic.as_mut())
                        {
                            let default = SymbolSpec::arrange(None, &sch.ports());
                            let spec = (!reset && *pins != default).then(|| SymbolSpec { pins: pins.clone() });
                            if sch.symbol != spec {
                                self.sch_eds.entry(cell.clone()).or_default().history.push(sch);
                                sch.symbol = spec;
                                self.mark_dirty();
                            }
                            let n = self.refresh_cell_instances();
                            self.ok(format!("Symbol of {cell} updated; {n} placed instance(s) follow it"));
                            close = true;
                        }
                    });
                }
                Dialog::Recover { dir, root, saved } => {
                    ui.heading("Recover unsaved changes?");
                    let ago = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs())
                        .saturating_sub(*saved);
                    let when = if ago < 120 {
                        format!("{ago} s ago")
                    } else if ago < 7200 {
                        format!("{} min ago", ago / 60)
                    } else {
                        format!("{} h ago", ago / 3600)
                    };
                    ui.label(format!(
                        "IC Studio closed with unsaved changes to this project. An automatic copy from {when} is newer than the saved files."
                    ));
                    ui.label(
                        RichText::new("Recover opens the copy; Save then writes it to the project. Discard deletes the copy.")
                            .small()
                            .color(pal.text_weak),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Discard copy").color(pal.danger)).clicked() {
                            crate::recovery::discard(root);
                            close = true;
                        }
                        if ui.button(RichText::new("Recover").strong()).clicked() {
                            match crate::recovery::restore(dir, root) {
                                Ok(p) => {
                                    self.set_project(p);
                                    self.dirty = true;
                                    self.ok("Recovered the unsaved changes: save to keep them".into());
                                }
                                Err(e) => self.error(format!("Cannot read the recovery copy: {e}")),
                            }
                            close = true;
                        }
                    });
                }
                Dialog::SaveConflict { files, then } => {
                    ui.heading("Files changed on disk");
                    ui.label(
                        "Another program (git, the ics command line, an AI agent) changed these files after the \
                         project was opened, so nothing was saved:",
                    );
                    let root = self.project.as_ref().and_then(|p| p.root.clone()).unwrap_or_default();
                    for f in files.iter().take(8) {
                        let shown = f.strip_prefix(&root).unwrap_or(f);
                        ui.label(RichText::new(shown.display().to_string()).monospace().small());
                    }
                    if files.len() > 8 {
                        ui.label(RichText::new(format!("and {} more", files.len() - 8)).small());
                    }
                    ui.label(
                        RichText::new(
                            "Overwrite replaces them with the project in this window. To keep their version, \
                             cancel and reopen the project.",
                        )
                        .small()
                        .color(pal.text_weak),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new("Overwrite").color(pal.danger)).clicked() {
                            if self.save_overwriting()
                                && let Some(t) = then.take()
                            {
                                self.queue.push(*t);
                            }
                            close = true;
                        }
                    });
                }
                Dialog::Settings => {
                    ui.heading("Settings");
                    ui.add_space(8.0);
                    egui::Grid::new("settings").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                        ui.label("Theme");
                        ui.horizontal(|ui| {
                            if ui.radio(self.settings.dark, "Dark").clicked() && !self.settings.dark {
                                self.queue.push(Command::ToggleTheme);
                            }
                            if ui.radio(!self.settings.dark, "Light").clicked() && self.settings.dark {
                                self.queue.push(Command::ToggleTheme);
                            }
                        });
                        ui.end_row();
                        ui.label("UI scale");
                        if ui.add(egui::Slider::new(&mut self.settings.ui_scale, 0.75..=1.75).step_by(0.05)).drag_stopped() {
                            ctx.set_zoom_factor(self.settings.ui_scale);
                        }
                        ui.end_row();
                        ui.label("ngspice");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.settings.ngspice_path).desired_width(220.0));
                            if ui.button("Detect").clicked() {
                                self.ngspice = ics_sim::ngspice::detect(&self.settings.ngspice_path);
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        match &self.ngspice {
                            Some(v) => ui.colored_label(pal.ok, format!("✔ {v}")),
                            None => ui.colored_label(pal.text_weak, "not found — built-in engine will be used"),
                        };
                        ui.end_row();
                        let il = ui.label("Interaction");
                        egui::ComboBox::from_id_salt("interaction")
                            .selected_text(self.settings.interaction.name())
                            .show_ui(ui, |ui| {
                                for p in Profile::ALL {
                                    ui.selectable_value(&mut self.settings.interaction, p, p.name()).on_hover_text(p.describe());
                                }
                            })
                            .response
                            .labelled_by(il.id)
                            .on_hover_text(self.settings.interaction.describe());
                        ui.end_row();
                        let ml = ui.label("MOS symbol");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.settings.mos_body_pin, true, "Body pin (4-terminal)")
                                .labelled_by(ml.id)
                                .on_hover_text("New NMOS / PMOS devices have a body pin to wire");
                            ui.radio_value(&mut self.settings.mos_body_pin, false, "Arrow on source (3-terminal)")
                                .labelled_by(ml.id)
                                .on_hover_text("New NMOS / PMOS devices hide the body: it is on ground (NMOS) or vdd (PMOS), set by the bulk parameter");
                        });
                        ui.end_row();
                        ui.label("Startup");
                        ui.checkbox(&mut self.settings.reopen_last, "Reopen last project");
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.settings.check_updates, "Check for updates");
                        ui.end_row();
                        ui.label("AI");
                        if ui.button(format!("AI Models… ({})", self.settings.llm.describe())).clicked() {
                            self.ai_models.open = true;
                        }
                        ui.end_row();
                        ui.label("AI agents (MCP)").on_hover_text(
                            "Let AI agents such as Claude Code drive IC Studio. They start the ics command line tool with ics mcp; \
                             while this window is open they work on its project.",
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .button("Add to Claude Code")
                                .on_hover_text("Runs: claude mcp add --scope user ic-studio -- <folder>/ics mcp")
                                .clicked()
                            {
                                self.queue.push(Command::AddMcpToClaudeCode);
                            }
                            if ui
                                .button("Copy MCP config")
                                .on_hover_text("JSON for other MCP clients (Cursor, Claude Desktop, ...)")
                                .clicked()
                            {
                                self.queue.push(Command::CopyMcpConfig);
                            }
                        });
                        ui.end_row();
                    });
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        ctx.set_zoom_factor(self.settings.ui_scale);
                        close = true;
                    }
                }
                Dialog::Shortcuts => {
                    ui.heading("Keyboard shortcuts");
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.75).show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            for half in SHORTCUTS.chunks(2) {
                                ui.vertical(|ui| {
                                    ui.set_width(400.0);
                                    for (title, rows) in half {
                                        ui.add_space(6.0);
                                        ui.label(RichText::new(*title).strong().color(pal.accent));
                                        egui::Grid::new(title).num_columns(2).min_col_width(170.0).spacing([12.0, 4.0]).show(
                                            ui,
                                            |ui| {
                                                for (keys, what) in *rows {
                                                    ui.label(RichText::new(keys.replace("Mod+", MOD)).monospace());
                                                    ui.label(*what);
                                                    ui.end_row();
                                                }
                                            },
                                        );
                                    }
                                });
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open user guide").clicked() {
                            self.queue.push(Command::UserGuide);
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Update => {
                    ui.heading("Software update");
                    ui.label(RichText::new(format!("Installed version {}", update::CURRENT)).color(pal.text_weak));
                    ui.add_space(8.0);
                    let release_notes = |ui: &mut egui::Ui, r: &update::Release| {
                        if !r.notes.trim().is_empty() {
                            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                ui.label(RichText::new(r.notes.trim()).small());
                            });
                        }
                        ui.hyperlink_to("Release page", &r.page);
                    };
                    match &self.updater.state {
                        update::State::Idle | update::State::Checking(_) => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label("Checking GitHub for a newer release…");
                            });
                        }
                        update::State::UpToDate => {
                            ui.colored_label(pal.ok, "IC Studio is up to date.");
                        }
                        update::State::Available(r) => {
                            ui.label(RichText::new(format!("IC Studio {} is available.", r.version)).strong());
                            release_notes(ui, r);
                            ui.add_space(6.0);
                            if r.url.is_none() {
                                ui.colored_label(pal.warning, "No build for this platform; download it from the release page.");
                            } else if ui.button(RichText::new("Download and install").strong()).clicked() {
                                self.queue.push(Command::InstallUpdate);
                            }
                        }
                        update::State::Downloading { release, .. } => {
                            ui.label(format!("Downloading IC Studio {}…", release.version));
                            match self.updater.progress() {
                                Some(f) => ui.add(egui::ProgressBar::new(f).show_percentage()),
                                None => ui.add(egui::Spinner::new()),
                            };
                        }
                        update::State::Installed { release, .. } => {
                            ui.colored_label(pal.ok, format!("IC Studio {} is installed.", release.version));
                            ui.label("Restart to start using it.");
                            ui.add_space(6.0);
                            if ui.button(RichText::new("Restart now").strong()).clicked() {
                                self.queue.push(Command::RestartToUpdate);
                                close = true;
                            }
                        }
                        update::State::Failed(e) => {
                            ui.colored_label(pal.danger, format!("Update failed: {e}"));
                            if ui.button("Try again").clicked() {
                                self.queue.push(Command::CheckForUpdates);
                            }
                        }
                    }
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                }
                Dialog::About => {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("IC Studio").size(26.0).strong());
                        ui.label(format!("version {}", env!("CARGO_PKG_VERSION")));
                        ui.add_space(6.0);
                        ui.label("A modern, open full-custom IC design environment.");
                        ui.label(RichText::new("Rust · egui/wgpu · ics-core · ics-sim · ngspice").small().color(pal.text_weak));
                        ui.add_space(6.0);
                        ui.hyperlink_to("User guide", GUIDE_URL);
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if modal.should_close() && !matches!(dlg, Dialog::Unsaved { .. }) {
            close = true;
        }
        if !close && self.dialog.is_none() {
            self.dialog = Some(dlg);
        }
    }

    /// Checks GitHub for a newer release in the background, if enabled in Settings.
    fn update_toast(&mut self, ctx: &egui::Context) {
        if !matches!(self.updater.state, update::State::Available(_)) {
            self.update_toast = None;
        }
        let Some(v) = self.update_toast.clone() else { return };
        let pal = theme::palette(ctx);
        egui::Area::new(egui::Id::unique("update_toast"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -36.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(12).show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Update available").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                                self.update_toast = None;
                            }
                        });
                    });
                    ui.label(
                        RichText::new(format!("IC Studio {v} is available (installed {}).", update::CURRENT))
                            .color(pal.text_weak),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Update now").clicked() {
                            self.update_toast = None;
                            self.queue.push(Command::InstallUpdate);
                        }
                        if ui.button("Release notes").clicked() {
                            self.update_toast = None;
                            self.dialog = Some(Dialog::Update);
                        }
                        if ui.button("Later").clicked() {
                            self.update_toast = None;
                        }
                    });
                });
            });
    }

    /// Test hook: behave as if the start-up check found `r`.
    #[doc(hidden)]
    pub fn announce_update(&mut self, r: update::Release) {
        self.update_toast = Some(r.version.clone());
        self.updater.state = update::State::Available(r);
    }

    /// Writes the example projects that are missing from `dir` (existing ones are kept) and
    /// reports it. Returns whether it succeeded.
    pub fn write_examples(&mut self, dir: &Path) -> bool {
        match examples::write_topics(dir, false) {
            Ok(written) if written.is_empty() => {
                self.info(format!("The example projects are already in {}", dir.display()));
                true
            }
            Ok(written) => {
                self.ok(format!("Wrote example project(s) {} to {}", written.join(", "), dir.display()));
                true
            }
            Err(e) => {
                self.error(format!("Could not write the example projects to {}: {e}", dir.display()));
                false
            }
        }
    }

    /// After an install or update (a version that has not written them yet), adds the example
    /// projects missing from `Documents/IC Studio/Examples`, as the installer does. Examples
    /// already there are kept. Skipped in debug builds.
    pub fn sync_examples_on_start(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        if cfg!(debug_assertions) || self.settings.examples_version == version {
            return;
        }
        let Some(dir) = examples_dir() else { return };
        if self.write_examples(&dir) {
            self.settings.examples_version = version.into();
        }
    }

    pub fn check_updates_on_start(&mut self, ctx: &egui::Context) {
        if self.settings.check_updates {
            self.updater.check(ctx, false);
        }
    }

    /// What the user has done of the tutorial's steps.
    fn tutorial_progress(&self) -> crate::tutorial::Progress {
        let inverter = self.project.as_ref().and_then(|p| {
            p.all_cells().find(|(_, c)| c.name == "cmos_inverter" && c.schematic.is_some() && c.layout.is_some()).map(|(r, _)| r)
        });
        let Some(r) = inverter.clone() else {
            return crate::tutorial::Progress {
                workspace: Some(self.workspace),
                palette: self.palette.is_some(),
                ..Default::default()
            };
        };
        let lay = self.lay_eds.get(&r).map(|e| &e.verify);
        crate::tutorial::Progress {
            on_inverter: self.active.as_ref() == Some(&r),
            selected: self.sch_eds.get(&r).is_some_and(|e| !e.selection.is_empty()),
            simulated: self.sims.get(&r).is_some_and(|s| s.results.is_some()),
            calculator: self.calc.open,
            drc: lay.is_some_and(|v| v.drc.is_some()),
            lvs: lay.is_some_and(|v| v.lvs.is_some()),
            derived: self
                .project
                .as_ref()
                .is_some_and(|p| p.all_cells().any(|(_, c)| c.notes.starts_with("Sized by the optimizer"))),
            palette: self.palette.is_some(),
            workspace: Some(self.workspace),
            inverter,
        }
    }

    #[doc(hidden)]
    pub fn tutorial(&self) -> &crate::tutorial::Tutorial {
        &self.tutorial
    }

    /// Queue a command, exactly as a menu item or shortcut would. This is the entry point
    /// for scripting / automation front-ends.
    pub fn command(&mut self, c: Command) {
        self.queue.push(c);
    }

    /// Serve MCP tool calls from `ics mcp` (live mode) on this window's project.
    pub fn attach_bridge(&mut self, rx: Receiver<Pending>) {
        self.bridge = Some(Bridge { rx, calls: 0, last: None, waiting: VecDeque::new(), job: None });
        self.info("MCP live mode: tool calls from ics mcp run on the open project".into());
    }

    /// Answer bridge requests in arrival order. Tools in [`tools::BACKGROUND`] run one at a
    /// time on a worker thread against a snapshot; a request behind a waiting one waits too, so
    /// every call sees the effects of the calls before it.
    fn serve_bridge(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.bridge else { return };
        b.waiting.extend(b.rx.try_iter());
        let done = match b.job.as_ref().map(|j| j.rx.try_recv()) {
            Some(Ok(out)) => Some(out),
            Some(Err(TryRecvError::Disconnected)) => Some((ToolResult::error("internal error in a background tool call"), None)),
            _ => None,
        };
        if let Some((res, verified)) = done
            && let Some(job) = b.job.take()
        {
            let _ = job.reply.send(res.clone());
            let target = job.cell.filter(|r| self.project.as_ref().is_some_and(|p| p.cell(r).is_some()));
            if let (Some(v), Some(r)) = (verified, &target)
                && let Some(ed) = self.lay_eds.get_mut(r)
            {
                ed.install_verified(v, job.generation);
            }
            self.mcp_done(&job.tool, target, &res, Some(job.started.elapsed()));
        }
        loop {
            let Some(b) = &mut self.bridge else { return };
            let Some(next) = b.waiting.front() else { return };
            let ext = self.extensions.has_tool(&next.tool);
            let background = ext || tools::BACKGROUND.contains(&next.tool.as_str());
            if background && b.job.is_some() {
                return;
            }
            let p = b.waiting.pop_front().expect("front exists");
            if ext {
                self.start_ext_job(ctx, p);
                continue;
            }
            match self.mcp_snapshot(&p.arguments).filter(|_| background) {
                Some((snap, args, cell)) => self.start_mcp_job(ctx, p, snap, args, cell),
                None => {
                    let res = self.mcp_call(&p.tool, &p.arguments);
                    let _ = p.reply.send(res);
                }
            }
        }
    }

    /// The project part a background call needs, with `cell` rewritten to `lib/cell` so it
    /// names the same cell in the snapshot. `None` (run inline) without a project or for an
    /// unknown cell.
    fn mcp_snapshot(&self, args: &McpArgs) -> Option<McpSnapshot> {
        let p = self.project.as_ref()?;
        let mut args = args.clone();
        let cell = match args.get("cell").filter(|v| !v.is_null()) {
            Some(v) => Some(tools::find(p, v.as_str()?)?),
            None => None,
        };
        if let Some(r) = &cell {
            args.insert("cell".into(), r.to_string().into());
        }
        Some((tools::snapshot(p, cell.as_ref(), None), args, cell))
    }

    fn start_mcp_job(&mut self, ctx: &egui::Context, p: Pending, snap: Project, args: McpArgs, cell: Option<CellRef>) {
        let generation = match &cell {
            Some(r) if matches!(p.tool.as_str(), "run_drc" | "extract" | "run_lvs") => {
                self.lay_eds.entry(r.clone()).or_default().verify.generation()
            }
            _ => 0,
        };
        let (tx, rx) = mpsc::channel();
        let (tool, dirty, ctx) = (p.tool.clone(), self.dirty, ctx.clone());
        std::thread::spawn(move || {
            let mut state = tools::State::in_gui(Some(snap), dirty);
            let res = ics_mcp::call_tool(&mut state, &tool, &args);
            let _ = tx.send((res, state.take_verified()));
            ctx.request_repaint();
        });
        self.info(match &cell {
            Some(r) => format!("MCP · {} {r} running in the background", p.tool),
            None => format!("MCP · {} running in the background", p.tool),
        });
        if let Some(b) = &mut self.bridge {
            b.job = Some(McpJob { tool: p.tool, cell, generation, started: Instant::now(), rx, reply: p.reply });
        }
    }

    /// Run a tool on the UI thread, directly on the open project.
    fn mcp_call(&mut self, tool: &str, args: &McpArgs) -> ToolResult {
        let cell = args.get("cell").and_then(|v| v.as_str()).map(str::to_string);
        let replaces = matches!(tool, "open_project" | "new_project");
        if replaces && self.dirty {
            return ToolResult::error(
                "the IC Studio window has unsaved changes: save or discard them there before opening another project",
            );
        }
        // Snapshots of every layout and schematic: whatever the tool changes becomes one undo
        // step per cell, and the editors redraw it.
        let edits = !replaces && !MCP_READ_ONLY.contains(&tool);
        let lay_before = self.project.as_ref().filter(|_| edits).map(layout_snapshot);
        let sch_before = self.project.as_ref().filter(|_| edits).map(schematic_snapshot);
        let mut state = tools::State::in_gui(self.project.take(), self.dirty);
        let res = ics_mcp::call_tool(&mut state, tool, args);
        let (project, dirty) = state.into_parts();
        match project {
            Some(p) if !res.is_error && replaces => {
                let root = p.root.clone();
                self.set_project(p);
                if let Some(root) = root {
                    self.remember_recent(&root);
                }
            }
            p => self.project = p,
        }
        self.dirty = dirty;
        let mut target = cell.as_deref().and_then(|c| self.project.as_ref().and_then(|p| tools::find(p, c)));
        if !res.is_error {
            let mut changed = vec![];
            if let Some(before) = lay_before {
                changed = self.layouts_edited(before);
            }
            if let Some(before) = sch_before {
                self.schematics_edited(before);
            }
            if tool == "run_skill" {
                target = changed.into_iter().next().filter(|_| self.active.is_none());
            }
            if matches!(tool, "copy_cell" | "derive_design")
                && let Some(new) =
                    serde_json::from_str::<serde_json::Value>(&res.text).ok().and_then(|v| v["cell"].as_str().map(str::to_string))
            {
                target = self.project.as_ref().and_then(|p| tools::find(p, &new));
            }
        }
        if !res.is_error
            && matches!(tool, "pcell_design_create" | "pcell_design_edit")
            && let (Some(p), Some(r)) = (&self.project, &target)
        {
            self.pcell_designer.sync(p, r);
        }
        self.mcp_done(tool, target, &res, None);
        res
    }

    /// Log a finished call and show its cell in the matching workspace.
    fn mcp_done(&mut self, tool: &str, target: Option<CellRef>, res: &ToolResult, took: Option<Duration>) {
        let summary = mcp_summary(tool, res);
        let took = took.map_or(String::new(), |d| format!(" ({:.1} s)", d.as_secs_f64()));
        let line = match &target {
            Some(r) => format!("MCP · {tool} {r} -> {summary}{took}"),
            None => format!("MCP · {tool} -> {summary}{took}"),
        };
        if res.is_error {
            self.warn(line);
        } else {
            self.info(line);
        }
        if let Some(b) = &mut self.bridge {
            b.calls += 1;
            b.last = Some((Instant::now(), tool.to_string()));
        }
        if let Some(r) = target.filter(|_| !res.is_error) {
            let ws = match tool {
                "simulate" | "set_simulation" => Workspace::Simulation,
                "get_netlist" | "write_schematic" | "copy_cell" | "derive_design" => Workspace::Schematic,
                _ => Workspace::Layout,
            };
            self.queue.push(Command::OpenCell(r.clone(), Some(ws)));
            match tool {
                "simulate" => self.queue.push(Command::RunSimulation),
                "generate_layout" | "flylines" => {
                    let ed = self.lay_eds.entry(r).or_default();
                    ed.show_flylines = true;
                    ed.request_fit();
                }
                _ => {}
            }
        }
    }

    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    /// The open project for automation and tests (mark it dirty after editing).
    pub fn project_mut(&mut self) -> Option<&mut Project> {
        self.project.as_mut()
    }

    /// Settings > Interaction.
    pub fn set_interaction(&mut self, p: Profile) {
        self.settings.interaction = p;
    }

    pub fn active_ref(&self) -> Option<&CellRef> {
        self.active.as_ref()
    }

    pub fn workspace(&self) -> Workspace {
        self.workspace
    }

    /// Console messages, oldest first.
    pub fn console_lines(&self) -> impl Iterator<Item = &str> {
        self.log.iter().map(|l| l.text.as_str())
    }

    /// True while any background simulation is running (used by UI tests).
    pub fn is_simulating(&self) -> bool {
        self.sims.values().any(|s| s.is_running())
    }

    /// The schematic editor of a cell, once it was opened (used by UI tests).
    pub fn schematic_editor(&self, r: &CellRef) -> Option<&SchematicEditor> {
        self.sch_eds.get(r)
    }

    /// The layout editor of a cell, once it was opened (used by UI tests).
    pub fn layout_editor(&self, r: &CellRef) -> Option<&LayoutEditor> {
        self.lay_eds.get(r)
    }

    pub fn layout_editor_mut(&mut self, r: &CellRef) -> Option<&mut LayoutEditor> {
        self.lay_eds.get_mut(r)
    }

    /// True while a simulation, a verification step or an MCP tool call is still running.
    pub fn is_busy(&self) -> bool {
        self.is_simulating()
            || self.lay_eds.values().any(LayoutEditor::is_verifying)
            || self.bridge.as_ref().is_some_and(|b| b.job.is_some() || !b.waiting.is_empty())
            || self.rule_designer.is_checking()
            || self.pcell_designer.is_checking()
            || self.sweep.is_running()
            || self.silicon.is_running()
            || self.process_data.is_running()
            || self.copilot.busy()
            || self.optimizer.is_running()
            || self.monte_carlo.is_running()
            || self.verification.is_running()
            || self.pixel_circuit.is_running()
    }

    fn pcell_designer_window(&mut self, ctx: &egui::Context) {
        let active = self.active_layout();
        let (lib, def) = match self.pcell_designer.window(ctx, self.project.as_ref(), active.as_ref()) {
            Some(PCellAction::Publish { lib, def }) => (lib, *def),
            Some(PCellAction::SetRoles { poly, diffusion }) => {
                self.command(Command::SetDeviceRoles(poly, diffusion));
                return;
            }
            None => return,
        };
        let Some(p) = self.project.as_mut() else { return };
        match p.publish_template(&lib, def) {
            Ok((master, updated, warnings)) => {
                let mut msg = format!("Saved PCell {master}");
                if updated > 0 {
                    msg.push_str(&format!(", {updated} placed variant(s) updated"));
                }
                for w in &warnings {
                    self.warn(format!("PCell Designer: {w}"));
                }
                self.pcell_designer.saved(msg.clone(), true);
                self.hier.clear();
                self.mark_dirty();
                self.ok(msg);
            }
            Err(e) => {
                self.pcell_designer.saved(e.clone(), false);
                self.error(format!("PCell Designer: {e}"));
            }
        }
    }

    pub fn pcell_designer(&self) -> &PCellDesigner {
        &self.pcell_designer
    }

    pub fn pcell_designer_mut(&mut self) -> &mut PCellDesigner {
        &mut self.pcell_designer
    }

    fn rule_designer_window(&mut self, ctx: &egui::Context) {
        let active = self.active_layout();
        let Some(action) = self.rule_designer.window(ctx, self.project.as_ref(), active.as_ref()) else { return };
        let (DesignerAction::Apply(text) | DesignerAction::ApplyAndRun(text)) = &action;
        let Some(p) = self.project.as_mut() else { return };
        match p.set_drc_deck(text.clone()) {
            Ok(msg) => self.ok(format!("Rule Designer: {msg}")),
            Err(e) => return self.error(format!("Rule Designer: {e}")),
        }
        for ed in self.lay_eds.values_mut() {
            ed.verify.deck = None;
        }
        self.mark_dirty();
        if matches!(action, DesignerAction::ApplyAndRun(_)) {
            self.queue.push(Command::RunDrc);
        }
    }

    /// The technology's deck was replaced from a file: drop the decks the editors and the
    /// designer hold.
    fn drc_deck_changed(&mut self) {
        for ed in self.lay_eds.values_mut() {
            ed.verify.deck = None;
        }
        if let Some(p) = &self.project {
            self.rule_designer.tech_deck_changed(&p.tech);
        }
        self.mark_dirty();
    }

    fn deck_dialog(title: &str) -> rfd::FileDialog {
        rfd::FileDialog::new().set_title(title).add_filter("DRC deck", &["deck", "drc", "svrf", "rul", "cal", "txt"])
    }

    fn import_drc_deck(&mut self, path: Option<PathBuf>) {
        let Some(p) = self.project.as_ref() else { return self.warn("Open a project first".into()) };
        let Some(path) = path.or_else(|| Self::deck_dialog("Import a DRC deck").pick_file()) else { return };
        // Parse first so a deck with errors is not stored.
        let notes = match ics_core::drc::load_deck(&path, &p.tech) {
            Ok(d) => d.notes,
            Err(e) => return self.error(format!("Cannot import the DRC deck: {e}")),
        };
        let text = match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| ics_core::drc::deck_file_text(&t, &path))
        {
            Ok((t, _)) => t,
            Err(e) => return self.error(format!("Cannot import the DRC deck: {e}")),
        };
        let Some(p) = self.project.as_mut() else { return };
        let unlinked = p.unlink_drc_deck();
        p.tech.drc_deck = Some(text);
        self.drc_deck_changed();
        for n in notes {
            self.info(format!("DRC deck: {n}"));
        }
        let also = unlinked.map_or(String::new(), |f| format!(" (unlinked the shared deck {})", f.display()));
        self.ok(format!("Imported the DRC deck {} into the technology{also}", path.display()));
    }

    fn export_drc_deck(&mut self, path: Option<PathBuf>) {
        let Some(p) = self.project.as_ref() else { return self.warn("Open a project first".into()) };
        let text = p.tech.deck_text();
        let name = format!("{}.deck", p.tech.name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_"));
        let Some(path) = path.or_else(|| Self::deck_dialog("Export the DRC deck").set_file_name(name).save_file()) else {
            return;
        };
        match std::fs::write(&path, text) {
            Ok(()) => self.ok(format!("Wrote the DRC deck to {}", path.display())),
            Err(e) => self.error(format!("Cannot write the DRC deck: {e}")),
        }
    }

    fn link_drc_deck(&mut self, path: Option<PathBuf>) {
        let Some(p) = self.project.as_ref() else { return self.warn("Open a project first".into()) };
        let Some(path) = path.or_else(|| Self::deck_dialog("Link a shared DRC deck").pick_file()) else { return };
        if let Err(e) = ics_core::drc::load_deck(&path, &p.tech) {
            return self.error(format!("Cannot link the DRC deck: {e}"));
        }
        let Some(p) = self.project.as_mut() else { return };
        match p.link_drc_deck(&path) {
            Ok(()) => {
                self.drc_deck_changed();
                self.ok(format!(
                    "Linked the shared DRC deck {}: it is re-read when the project opens and applying a deck writes it",
                    path.display()
                ));
            }
            Err(e) => self.error(format!("Cannot link the DRC deck: {e}")),
        }
    }

    fn layer_display_window(&mut self, ctx: &egui::Context) {
        let Some(layers) = self.display.window(ctx, self.project.as_ref()) else { return };
        let Some(p) = self.project.as_mut() else { return };
        let n = crate::display_view::apply_display(&mut p.tech, &layers);
        self.mark_dirty();
        self.ok(format!("Layer display: {n} layer(s) changed"));
    }

    fn stream_map_window(&mut self, ctx: &egui::Context) {
        let Some(rows) = self.stream_map.window(ctx, self.project.as_ref()) else { return };
        let Some(p) = self.project.as_mut() else { return };
        p.tech.stream_map = rows;
        let problems = ics_core::stream_map::problems(&p.tech);
        let n = p.tech.stream_map.len();
        self.mark_dirty();
        self.ok(format!("Stream layer map: {n} row(s) applied to the technology"));
        for w in problems {
            self.warn(format!("Stream layer map: {w}"));
        }
    }

    /// Checker findings of the active schematic.
    fn active_findings(&self) -> Vec<ics_core::sch_check::Finding> {
        self.active_cell().and_then(|c| c.schematic.as_ref()).map_or(vec![], ics_core::sch_check::check)
    }

    fn check_schematic(&mut self, save: bool) {
        let Some(r) = self.active.clone() else { return };
        if self.active_cell().is_none_or(|c| c.schematic.is_none()) {
            self.warn(format!("{r} has no schematic to check"));
            return;
        }
        use ics_core::sch_check::Severity;
        self.checks = self.active_findings();
        let n = |s: Severity| self.checks.iter().filter(|f| f.severity == s).count();
        let (e, w) = (n(Severity::Error), n(Severity::Warning));
        let msg = format!("Check {r}: {e} error(s), {w} warning(s)");
        if e > 0 {
            self.error(msg);
            self.bottom_tab = BottomTab::Problems;
        } else if w > 0 {
            self.warn(msg);
        } else {
            self.ok(msg);
        }
        if e + w > 0 {
            self.show_console = true;
            self.bottom_tab = BottomTab::Problems;
        }
        if save && e == 0 {
            self.queue.push(Command::Save);
        } else if save {
            self.warn("Not saved: fix the schematic errors first".into());
        }
    }

    pub fn last_check(&self) -> &[ics_core::sch_check::Finding] {
        &self.checks
    }

    /// Bounding box of the selection in the active layout.
    fn selection_bbox(&mut self) -> Option<ics_core::IRect> {
        let r = self.active_layout()?;
        let p = self.project.as_ref()?;
        let l = p.cell(&r)?.layout.as_ref()?;
        let sel = &self.lay_eds.get(&r)?.selection;
        if sel.is_empty() {
            return None;
        }
        let idx = self.hier.get_for(&r, l, p);
        let shapes = l.shapes.iter().filter(|s| sel.contains(&s.id)).map(|s| s.bbox());
        let insts =
            l.instances.iter().enumerate().filter(|(_, i)| sel.contains(&i.id)).filter_map(|(k, _)| idx.instance_bbox(l, k));
        shapes.chain(insts).reduce(|a, b| ics_core::IRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
    }

    fn results_window(&mut self, ctx: &egui::Context) {
        if !self.results_open {
            return;
        }
        let active = self.active.clone();
        let sim = active.as_ref().and_then(|r| self.sims.get_mut(r));
        let mut open = true;
        crate::float::tool_window(ctx, "results_browser", "Results Browser", [420.0, 520.0], &mut open, |ui| {
            let pal = theme::palette(ui.ctx());
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                ui.label(RichText::new("Results Browser").strong().color(pal.text_strong));
                ui.label(
                    RichText::new("Checked runs are overlaid (dashed) on the Simulation plots and listed in the measurements.")
                        .small()
                        .color(pal.text_weak),
                );
                ui.separator();
                match sim {
                    Some(s) if s.results.is_some() => {
                        let res = s.results.as_ref().unwrap();
                        ui.label(
                            RichText::new(format!("{} · {} analyses", res.backend, res.datasets.len()))
                                .small()
                                .color(pal.text_weak),
                        );
                        crate::sim_view::runs_ui(ui, &mut s.label, &mut s.history);
                    }
                    _ => {
                        ui.label(RichText::new("Simulate the active cell first.").color(pal.text_weak));
                    }
                }
            });
        });
        self.results_open = open;
    }

    fn net_window(&mut self, ctx: &egui::Context) {
        let active = self.active_layout();
        let ed = active.as_ref().and_then(|r| self.lay_eds.get_mut(r));
        if let Some(NetAction::Extract) = self.nets.window(ctx, self.project.as_ref(), active.as_ref(), ed) {
            self.queue.push(Command::ExtractNets);
        }
    }

    fn hier_window(&mut self, ctx: &egui::Context) {
        if let Some(HierAction::Open(r, ws)) = self.hier_browser.window(ctx, self.project.as_ref(), self.active.as_ref()) {
            self.queue.push(Command::OpenCell(r, Some(ws)));
        }
    }

    /// MOS devices placed in the active layout, by schematic device name.
    fn layout_devices(&self) -> Vec<String> {
        let (Some(r), Some(p)) = (self.active_layout(), self.project.as_ref()) else { return vec![] };
        let Some(l) = p.cell(&r).and_then(|c| c.layout.as_ref()) else { return vec![] };
        let mut out: Vec<String> = l
            .instances
            .iter()
            .filter(|i| matches!(p.cell(&i.cell).and_then(|c| c.pcell.as_ref()), Some(ics_core::pcell::PCell::Mos { .. })))
            .filter_map(|i| i.name.as_deref())
            .filter(|n| !n.starts_with(ics_core::sdl::DUMMY))
            .map(|n| ics_core::sdl::device_of(n).to_string())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The first SKILL form waiting for the user, as a dialog: OK sets its fields and runs its
    /// callback in the CIW session, Cancel drops it.
    fn skill_form_window(&mut self, ctx: &egui::Context) {
        use ics_core::skill::FieldKind;
        let Some((form, fields)) = self.skill_forms.first_mut() else { return };
        let mut ok = false;
        let mut cancel = false;
        let mut open = true;
        egui::Window::new(form.title.clone())
            .id(egui::Id::unique(format!("skill_form_{}", form.symbol)))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("skill_form_fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for (k, f) in fields.iter_mut().enumerate() {
                        match &f.kind {
                            FieldKind::Label => {
                                ui.label(&f.prompt);
                                ui.label("");
                            }
                            FieldKind::Bool => {
                                ui.label("");
                                let mut on = f.value == "t";
                                if ui.checkbox(&mut on, &f.prompt).changed() {
                                    f.value = if on { "t".into() } else { "nil".into() };
                                }
                            }
                            FieldKind::Choice(choices) => {
                                let l = ui.label(&f.prompt);
                                let choices = choices.clone();
                                egui::ComboBox::from_id_salt(("skill_form_choice", k))
                                    .selected_text(f.value.clone())
                                    .show_ui(ui, |ui| {
                                        for c in choices {
                                            ui.selectable_value(&mut f.value, c.clone(), c);
                                        }
                                    })
                                    .response
                                    .labelled_by(l.id);
                            }
                            FieldKind::String | FieldKind::Float | FieldKind::Int => {
                                let l = ui.label(&f.prompt);
                                ui.text_edit_singleline(&mut f.value).labelled_by(l.id);
                            }
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ok = ui.button("OK").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if !(ok || cancel || !open) {
            return;
        }
        let (form, fields) = self.skill_forms.remove(0);
        if !ok {
            self.ciw.push(CiwKind::Output, format!("{}: cancelled", form.title));
            return;
        }
        self.skill_start(ics_core::skill::Task::FormOk(form, fields), SkillJobKind::Form);
    }

    /// Answer the first displayed SKILL form as if its dialog's OK (or Cancel) was pressed with
    /// `edits` (field name, value text) applied. For tests and automation.
    pub fn answer_skill_form(&mut self, ok: bool, edits: &[(&str, &str)]) -> bool {
        if self.skill_forms.is_empty() || self.skill_job.is_some() {
            return false;
        }
        let (form, mut fields) = self.skill_forms.remove(0);
        for (n, v) in edits {
            if let Some(f) = fields.iter_mut().find(|f| f.name == *n) {
                f.value = v.to_string();
            }
        }
        if ok {
            let current = self.active.clone();
            let Some(p) = self.project.as_mut() else { return false };
            let before = layout_snapshot(p);
            let res = self.ciw.session.form_ok(p, &form, &fields, current.as_ref());
            self.ciw.show(&res);
            self.layouts_edited(before);
            return res.is_ok();
        }
        true
    }

    /// Libraries offered for a `cds.lib` import, while the choice is open.
    pub fn pending_virtuoso_libs(&self) -> Vec<String> {
        self.virtuoso_pick.as_ref().map(|(_, l, _)| l.clone()).unwrap_or_default()
    }

    /// Titles of the SKILL forms waiting for the user.
    pub fn pending_skill_forms(&self) -> Vec<String> {
        self.skill_forms.iter().map(|(f, _)| f.title.clone()).collect()
    }

    /// Library choice for a `cds.lib` import.
    fn virtuoso_pick_window(&mut self, ctx: &egui::Context) {
        let Some((path, libs, found)) = &self.virtuoso_pick else { return };
        let mut pick = None;
        let mut open = true;
        egui::Window::new("Import Virtuoso Library")
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Libraries in {}:", path.display()));
                ui.label(
                    egui::RichText::new(if *found {
                        "Virtuoso runs in batch mode to dump the library you pick."
                    } else {
                        "Virtuoso was not found (set ICS_VIRTUOSO or add it to the PATH)."
                    })
                    .small()
                    .weak(),
                );
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for l in libs {
                        if ui.button(l).clicked() {
                            pick = Some(l.clone());
                        }
                    }
                });
            });
        if let Some(lib) = pick {
            let (path, _, _) = self.virtuoso_pick.take().expect("shown");
            self.queue.push(Command::ImportVirtuosoLib(path, lib));
        } else if !open {
            self.virtuoso_pick = None;
        }
    }

    fn gen_window(&mut self, ctx: &egui::Context) {
        if !self.gens.open {
            return;
        }
        let active = self.active_layout();
        let sel = self.selection_bbox();
        let devices = self.layout_devices();
        if let Some(a) = self.gens.window(ctx, self.project.as_ref(), active.as_ref(), sel, &devices) {
            self.apply_gen(a);
        }
    }

    /// Run a layout generator on the active layout (undoable).
    pub fn apply_gen(&mut self, a: GenAction) {
        let Some(r) = self.active_layout() else {
            self.warn("Open a layout cell first".into());
            return;
        };
        let Some(p) = self.project.as_mut() else { return };
        let before = p.cell(&r).and_then(|c| c.layout.clone()).unwrap_or_default();
        let res: Result<String, String> = match a {
            GenAction::Match(spec) => {
                ics_core::matching::apply(p, &r, &spec).map(|names| format!("placed {} unit(s)", names.len()))
            }
            GenAction::PlaceLike { reference, route } => place_like(p, &reference, &r, route),
            // A contacted, strapped ring is placed as a guard ring PCell, so it stays editable
            // (stretch a side, or change its parameters).
            GenAction::GuardRing(g) if g.contacts && g.metal => {
                let grid = p.tech.grid.max(1);
                let (x0, y0) = (g.area.x0.div_euclid(grid) * grid, g.area.y0.div_euclid(grid) * grid);
                let (x1, y1) = ((g.area.x1 + grid - 1).div_euclid(grid) * grid, (g.area.y1 + grid - 1).div_euclid(grid) * grid);
                let pc = ics_core::pcell::PCell::GuardRing { well: g.well, w: x1 - x0, h: y1 - y0, width: g.width, gap: g.gap };
                p.ensure_pcell(&pc).and_then(|cell| {
                    let l = p.cell_mut(&r).and_then(|c| c.layout.as_mut()).ok_or("no layout")?;
                    l.add_instance(cell, ics_core::Trans::new(x0, y0), None);
                    Ok(format!("placed {} (stretch a side to resize it)", pc.summary(p.tech.dbu_per_um)))
                })
            }
            other => {
                let shapes = match other {
                    GenAction::GuardRing(g) => ics_core::layout_gen::guard_ring(&p.tech, &g),
                    GenAction::ViaArray(v) => ics_core::layout_gen::via_array(&p.tech, &v),
                    GenAction::ViaOverlaps { via, area } => {
                        ics_core::layout_gen::via_arrays_at_overlaps(&before, &*p, &p.tech, &via, area)
                    }
                    GenAction::Match(_) | GenAction::PlaceLike { .. } => unreachable!(),
                };
                shapes.and_then(|shapes| {
                    if shapes.is_empty() {
                        return Err("nothing to create (no overlap of the via's layers)".into());
                    }
                    let n = shapes.len();
                    let l = p.cell_mut(&r).and_then(|c| c.layout.as_mut()).ok_or("no layout")?;
                    for (layer, g) in shapes {
                        l.add(layer, g);
                    }
                    Ok(format!("added {n} shape(s)"))
                })
            }
        };
        match res {
            Ok(m) => {
                let ed = self.lay_eds.entry(r.clone()).or_default();
                ed.history.push(&before);
                ed.layout_changed();
                self.hier.invalidate(&r);
                self.mark_dirty();
                self.ok(format!("Layout generator on {r}: {m}"));
            }
            Err(e) => self.error(format!("Layout generator: {e}")),
        }
    }

    pub fn layout_generators(&mut self) -> &mut LayoutGenerators {
        &mut self.gens
    }

    pub fn net_browser(&self) -> &NetBrowser {
        &self.nets
    }

    pub fn hierarchy_browser(&mut self) -> &mut HierBrowser {
        &mut self.hier_browser
    }

    fn fill_window(&mut self, ctx: &egui::Context) {
        let active = self.active_layout();
        if self.fill.window(ctx, self.project.as_ref(), active.as_ref()) {
            self.add_fill();
        }
    }

    /// The active cell when it has a layout.
    fn active_layout(&self) -> Option<CellRef> {
        self.active.clone().filter(|r| self.project.as_ref().and_then(|p| p.cell(r)).is_some_and(|c| c.layout.is_some()))
    }

    fn add_fill(&mut self) {
        let Some(r) = self.active_layout() else {
            self.warn("Open a layout cell to add fill".into());
            return;
        };
        let Some(p) = self.project.as_mut() else { return };
        if self.fill.params.is_none() {
            self.fill.show(&p.tech);
            self.fill.open = false;
        }
        let rep = match self.fill.run(p, &r) {
            Ok(rep) => rep,
            Err(e) => {
                self.error(format!("Dummy fill: {e}"));
                return;
            }
        };
        if rep.rects.is_empty() {
            self.info(format!("Dummy fill: nothing to add to {r} ({})", crate::fill_view::summary(&rep)));
            return;
        }
        let layer = self.fill.params.as_ref().map_or(0, |f| f.layer);
        let Some(lay) = p.cell_mut(&r).and_then(|c| c.layout.as_mut()) else { return };
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.history.push(lay);
        for rect in &rep.rects {
            lay.add(layer, ics_core::Geometry::Rect(*rect));
        }
        ed.layout_changed();
        self.hier.invalidate(&r);
        self.mark_dirty();
        self.ok(format!("Dummy fill on {r}: {}", crate::fill_view::summary(&rep)));
    }

    fn opc_window(&mut self, ctx: &egui::Context) {
        let active = self.active_layout();
        if self.opc.window(ctx, self.project.as_ref(), active.as_ref()) {
            self.apply_opc();
        }
    }

    fn apply_opc(&mut self) {
        let Some(r) = self.active_layout() else {
            self.warn("Open a layout cell to run OPC".into());
            return;
        };
        let Some(p) = self.project.as_mut() else { return };
        if self.opc.params.is_none() {
            self.opc.show(&p.tech);
            self.opc.open = false;
        }
        let rep = match self.opc.run(p, &r) {
            Ok(rep) => rep,
            Err(e) => {
                self.error(format!("OPC: {e}"));
                return;
            }
        };
        let layer = self.opc.params.as_ref().map_or(0, |o| o.layer);
        let summary = ics_core::opc::summary(&rep, p.tech.dbu_per_um);
        let (out, added) = match ics_core::opc::mask_layer(&mut p.tech, layer) {
            Ok(v) => v,
            Err(e) => {
                self.error(format!("OPC: {e}"));
                return;
            }
        };
        let name = p.tech.layers[out].name.clone();
        if added {
            self.opc_layers.0.push(name.clone());
            self.opc_layers.1.clear();
        }
        let Some(lay) = p.cell_mut(&r).and_then(|c| c.layout.as_mut()) else { return };
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.history.push(lay);
        ics_core::opc::write_mask(lay, &rep.mask, out);
        ed.layout_changed();
        if added {
            self.hier.clear();
        } else {
            self.hier.invalidate(&r);
        }
        self.mark_dirty();
        self.ok(format!("OPC on {r} → {name}: {summary}"));
    }

    /// After a layout undo / redo: drops trailing OPC mask layers that no shape uses any more
    /// (undo) and restores those that shapes use again (redo). Layers are referenced by index,
    /// so only the last layers are touched. Returns whether the technology changed.
    fn sync_opc_layers(p: &mut Project, (added, parked): &mut (Vec<String>, Vec<ics_core::LayerDef>)) -> bool {
        let used = |p: &Project, k: usize| {
            p.all_cells().any(|(_, c)| c.layout.as_ref().is_some_and(|l| l.shapes.iter().any(|s| s.layer == k)))
        };
        let mut changed = false;
        while let Some(last) = p.tech.layers.last()
            && added.last() == Some(&last.name)
            && !used(p, p.tech.layers.len() - 1)
        {
            added.pop();
            parked.extend(p.tech.layers.pop());
            changed = true;
        }
        while !parked.is_empty() && used(p, p.tech.layers.len()) {
            let def = parked.pop().expect("not empty");
            added.push(def.name.clone());
            p.tech.layers.push(def);
            changed = true;
        }
        changed
    }

    pub fn opc_tool(&mut self) -> &mut OpcTool {
        &mut self.opc
    }

    pub fn pixel_optics(&self) -> &PixelOptics {
        &self.pixel
    }

    pub fn pixel_circuit(&self) -> &PixelCircuitView {
        &self.pixel_circuit
    }

    pub fn pixel_circuit_mut(&mut self) -> &mut PixelCircuitView {
        &mut self.pixel_circuit
    }

    pub fn pixel_optics_mut(&mut self) -> &mut PixelOptics {
        &mut self.pixel
    }

    pub fn calculator(&self) -> &Calculator {
        &self.calc
    }

    pub fn sim_state(&self, r: &CellRef) -> Option<&SimState> {
        self.sims.get(r)
    }

    pub fn fill_tool(&mut self) -> &mut FillTool {
        &mut self.fill
    }

    pub fn layer_display_editor(&mut self) -> &mut DisplayEditor {
        &mut self.display
    }

    pub fn stream_map_editor(&mut self) -> &mut StreamMapEditor {
        &mut self.stream_map
    }

    pub fn rule_designer(&self) -> &RuleDesigner {
        &self.rule_designer
    }

    pub fn rule_designer_mut(&mut self) -> &mut RuleDesigner {
        &mut self.rule_designer
    }

    fn extensions_menu(extensions: &ics_mcp::ext::Registry, ui: &mut egui::Ui, has_project: bool, q: &mut Vec<Command>) {
        ui.menu_button("Extensions", |ui| {
            if ui.button("Manage Extensions…").clicked() {
                q.push(Command::Extensions);
            }
            let pdks = extensions.pdks();
            let with_commands: Vec<_> =
                extensions.extensions.iter().filter(|e| e.usable() && !e.manifest.commands.is_empty()).collect();
            if !with_commands.is_empty() || !pdks.is_empty() {
                ui.separator();
            }
            for e in with_commands {
                ui.menu_button(&e.manifest.extension.name, |ui| {
                    for c in &e.manifest.commands {
                        if menu_item(ui, has_project, &c.title, NEED_PROJECT) {
                            q.push(Command::RunExtension(e.id().to_string(), c.id.clone()));
                        }
                    }
                });
            }
            if !pdks.is_empty() {
                ui.menu_button("Load PDK", |ui| {
                    for (label, file) in pdks {
                        if menu_item(ui, has_project, &label, NEED_PROJECT) {
                            q.push(Command::LoadPdk(Some(file)));
                        }
                    }
                });
            }
        });
    }

    fn derive_window(&mut self, ctx: &egui::Context) {
        use crate::derive_view::DeriveAction;
        if let Some(DeriveAction::Start(from, plan, _layout)) = self.derive.window(ctx) {
            let Some(p) = self.project.as_mut() else { return };
            match ics_core::derive::create(p, &from, &plan) {
                Ok(to) => {
                    self.mark_dirty();
                    let backend = self.project.as_ref().and_then(|p| p.cell(&to)).map(|c| c.sim.backend).unwrap_or_default();
                    let opts = self.sim_options(backend);
                    if let Some(p) = self.project.as_ref() {
                        self.derive.start(ctx, p, to.clone(), opts);
                    }
                    self.info(format!("Derive: copied {from} to {to}, sizing it"));
                }
                Err(e) => self.derive.set_error(e),
            }
        }
    }

    fn optimizer_window(&mut self, ctx: &egui::Context) {
        use crate::optimizer_view::OptAction;
        let (action, run) = self.optimizer.window(ctx, self.project.as_ref());
        match action {
            Some(OptAction::Changed(s)) => {
                if let Some(c) = self.optimizer.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                    c.sim.optimize = s;
                    self.mark_dirty();
                }
            }
            Some(OptAction::Apply(r, values)) => {
                let setup = self.optimizer.setup.clone();
                if let Some(c) = self.project.as_mut().and_then(|p| p.cell_mut(&r))
                    && let Some(sch) = c.schematic.as_ref()
                {
                    c.schematic = Some(ics_sim::optimize::apply_values(sch, &setup, &values));
                    self.mark_dirty();
                    let text: Vec<String> = setup
                        .vars
                        .iter()
                        .zip(&values)
                        .map(|(v, x)| format!("{}={}", v.target, ics_core::units::format_eng(*x, "")))
                        .collect();
                    self.ok(format!("Optimizer: applied {} to {r}", text.join(" ")));
                }
            }
            None => {}
        }
        if run {
            self.queue.push(Command::RunOptimizer);
        }
    }

    pub fn optimizer(&mut self) -> &mut crate::optimizer_view::Optimizer {
        &mut self.optimizer
    }

    pub fn copilot_mut(&mut self) -> &mut crate::copilot::Copilot {
        &mut self.copilot
    }

    fn copilot_reply(&mut self, ctx: &egui::Context, reply: crate::copilot::Reply) {
        self.copilot.open = true;
        let context = self.copilot_context();
        let dirs: Vec<PathBuf> = self.project.as_ref().and_then(|p| p.root.clone()).into_iter().collect();
        self.copilot.reply(ctx, reply, &context, &self.settings.llm, &dirs);
    }

    /// What the copilot is told about the session.
    fn copilot_context(&self) -> String {
        let Some(p) = &self.project else { return "No project is open.".into() };
        let mut s = format!(
            "Open project: {} (technology {}, {} cells{}).",
            p.name,
            p.tech.name,
            p.all_cells().count(),
            if self.dirty { ", unsaved edits" } else { "" }
        );
        if let Some(r) = &self.active {
            s += &format!(" Active cell: {r} in the {} workspace; \"this cell\" means it.", self.workspace.name());
            if let Some(c) = p.cell(r) {
                if !c.notes.trim().is_empty() {
                    let notes: String = c.notes.trim().chars().take(1500).collect();
                    s += &format!("\nDesign notes of {r}:\n{notes}");
                }
                if !c.sim.sweep.specs.is_empty() {
                    s += &format!("\nSpecs of {r}:");
                    for sp in &c.sim.sweep.specs {
                        s += &format!("\n- {} = {} ({}): {}", sp.name, sp.expr, sp.analysis, crate::guide_view::spec_limits(sp));
                    }
                }
            }
        }
        s += "
Cells (views):";
        for (r, c) in p.all_cells().take(60) {
            let views: Vec<&str> =
                [(c.schematic.is_some(), "schematic"), (c.layout.is_some(), "layout"), (c.pcell.is_some(), "pcell")]
                    .into_iter()
                    .filter_map(|(has, v)| has.then_some(v))
                    .collect();
            s += &format!(
                "
- {r}: {}",
                views.join(", ")
            );
            if !c.description.trim().is_empty() {
                s += &format!(" ({})", c.description.trim());
            }
        }
        s
    }

    pub fn ciw(&self) -> &Ciw {
        &self.ciw
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match &self.project {
            Some(p) => format!("{}{} — IC Studio", p.name, if self.dirty { " •" } else { "" }),
            None => "IC Studio".to_string(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

impl eframe::App for IcStudio {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(e) = self.autosave.tick(self.project.as_ref(), self.dirty) {
            self.warn(format!("Autosave failed: {e}"));
        }
        // Window close with unsaved work.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Unsaved { then: Box::new(Command::Quit) });
        }
        if let Some(ev) = self.updater.poll() {
            let verbose = self.updater.verbose;
            match ev {
                update::Event::UpToDate if verbose => self.info(format!("IC Studio {} is up to date", update::CURRENT)),
                update::Event::Available(v) => {
                    self.info(format!("IC Studio {v} is available (Help › Check for Updates)"));
                    if !verbose {
                        self.update_toast = Some(v);
                    }
                }
                update::Event::Installed(v) => self.ok(format!("IC Studio {v} installed; restart to use it")),
                update::Event::Failed(e) if verbose => self.warn(format!("Update: {e}")),
                _ => {}
            }
        }
        // Background simulation completion.
        let mut events = vec![];
        for (r, st) in self.sims.iter_mut() {
            if let Some(ev) = st.poll() {
                events.push((r.clone(), ev));
            }
        }
        for (r, SimEvent::Finished { ok, message, log }) in events {
            for l in log.lines().filter(|l| !l.trim().is_empty()).take(60) {
                self.info(format!("  {l}"));
            }
            if ok {
                self.ok(format!("{r}: {message}"));
            } else {
                self.error(format!("{r}: {message}"));
            }
        }
        let mut verified = vec![];
        for (r, ed) in self.lay_eds.iter_mut() {
            if let Some(m) = ed.poll_verification() {
                verified.push(format!("{r}: {m}"));
            }
        }
        for m in verified {
            self.info(m);
        }
        if let Some(m) = self.sweep.poll() {
            let cell = self.sweep.cell.as_ref().map_or(String::new(), |c| c.to_string());
            if self.sweep.status().is_some_and(|s| s.0) {
                self.ok(format!("Sweep {cell}: {m}"))
            } else {
                self.warn(format!("Sweep {cell}: {m}"))
            }
        }
        if let Some((ok, m)) = self.optimizer.poll() {
            if ok { self.ok(m) } else { self.warn(m) }
        }
        if let Some((from, to, res, layout)) = self.derive.poll() {
            let out = match (res, self.project.as_mut()) {
                (Ok(report), Some(p)) => ics_sim::derive::finish(p, &from, &to, &report, layout),
                (Err(e), _) => Err(e),
                (_, None) => Err("the project was closed".into()),
            };
            self.hier.clear();
            self.mark_dirty();
            if let Some(e) = self.lay_eds.get_mut(&to) {
                e.request_fit();
            }
            match &out {
                Ok(o) => {
                    let m = format!("Derive: {to} {}", if o.passed { "meets every target" } else { "misses some targets" });
                    if o.passed {
                        self.ok(m)
                    } else {
                        self.warn(m)
                    }
                    self.queue.push(Command::OpenCell(to.clone(), Some(Workspace::Schematic)));
                }
                Err(e) => self.warn(format!("Derive: {e}")),
            }
            self.derive.finished(to, out);
        }
        if let Some((ok, m)) = self.monte_carlo.poll() {
            if ok { self.ok(m) } else { self.warn(m) }
        }
        if let Some((ok, m)) = self.verification.poll() {
            if ok { self.ok(m) } else { self.warn(m) }
        }
        if let Some((ok, m)) = self.silicon.poll() {
            if ok { self.ok(m) } else { self.warn(m) }
        }
        if let Some((ok, m)) = self.process_data.poll() {
            if ok { self.ok(m) } else { self.warn(m) }
        }
        self.copilot.poll();
        if let Some(rx) = &self.copilot.calls {
            // The copilot's file arguments must stay in the project's folder (or ICS_MCP_ROOTS),
            // as `ics mcp` keeps an outside agent's in its working directory.
            let roots = ics_mcp::confine::Roots::for_project(self.project.as_ref().and_then(|p| p.root.as_deref()));
            let calls: Vec<Pending> = rx
                .try_iter()
                .filter_map(|mut c| match roots.check(&c.arguments, None) {
                    Ok(a) => {
                        c.arguments = a;
                        Some(c)
                    }
                    Err(e) => {
                        let _ = c.reply.send(ToolResult::error(e));
                        None
                    }
                })
                .collect();
            if !calls.is_empty() {
                self.bridge_queue().extend(calls);
            }
        }
        self.serve_extensions();
        self.serve_bridge(ctx);
        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if self.dialog.is_none() && self.palette.is_none() && self.skill_job.is_none() {
            self.shortcuts(ctx);
        }
        self.update_title(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.settings.interaction.set(&ctx);
        let pal = theme::palette(&ctx);
        egui::Panel::top("menubar")
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(8, 5)))
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("statusbar")
            .exact_size(24.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(10, 3)))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("activity")
            .exact_size(56.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).stroke(Stroke::new(1.0, pal.border)))
            .show(ui, |ui| self.activity_bar(ui));
        theme::aside(
            egui::Panel::left("explorer")
                .resizable(true)
                .default_size(230.0)
                .size_range(160.0..=420.0)
                .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(10)),
            ui,
            |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.explorer(ui));
            },
        );
        if self.show_console {
            egui::Panel::bottom("console")
                .resizable(true)
                .default_size(160.0)
                .size_range(60.0..=500.0)
                .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin::symmetric(10, 6)))
                .show(ui, |ui| self.console(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel)).show(ui, |ui| {
            crate::float::set_work_area(ui.ctx(), ui.max_rect());
            if self.workspace != Workspace::Home && !self.tabs.is_empty() {
                egui::Panel::top("tabs")
                    .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 4,
                        bottom: 0,
                    }))
                    .show(ui, |ui| self.tab_strip(ui));
            }
            self.workspace_ui(ui);
        });
        self.sync_probe();

        self.command_palette(&ctx);
        self.dialogs(&ctx);
        self.update_toast(&ctx);
        self.rule_designer_window(&ctx);
        self.pcell_designer_window(&ctx);
        self.optimizer_window(&ctx);
        self.derive_window(&ctx);
        self.copilot.active_cell = self.active.as_ref().map(|r| r.cell.clone());
        if let Some(r) = self.copilot.window(&ctx, &self.settings.llm) {
            self.copilot_reply(&ctx, r);
        }
        if std::mem::take(&mut self.copilot.open_settings) {
            self.ai_models.open = true;
        }
        let dirs: Vec<PathBuf> = self.project.as_ref().and_then(|p| p.root.clone()).into_iter().collect();
        self.ai_models.window(&ctx, &mut self.settings.llm, &dirs);
        self.stream_map_window(&ctx);
        self.extensions_window(&ctx);
        self.layer_display_window(&ctx);
        self.fill_window(&ctx);
        self.opc_window(&ctx);
        if self.pixel_circuit.window(&ctx)
            && let Some(p) = self.project.as_ref()
        {
            self.pixel_circuit.start(Some(&ctx), p);
        }
        let active = self.active.clone();
        let sim = active.as_ref().and_then(|r| self.sims.get_mut(r));
        self.calc.window(&ctx, active.as_ref(), sim);
        match self.pixel.window(&ctx) {
            Some(PixelAction::Load) => {
                if let Some(path) = rfd::FileDialog::new().add_filter("Pixel stack", &["toml"]).pick_file() {
                    match self.pixel.load(path) {
                        Ok(()) => self.pixel.run(Some(&ctx), self.project.as_ref()),
                        Err(e) => self.warn(e),
                    }
                }
            }
            Some(PixelAction::Run) => self.pixel.run(Some(&ctx), self.project.as_ref()),
            Some(PixelAction::FromPdk) => self.pixel_stack_from_pdk(),
            Some(PixelAction::SetSource { source, amps }) => self.set_photocurrent(&source, amps),
            Some(PixelAction::ApplyDevice) => self.apply_pixel_device(),
            Some(PixelAction::LoadMeasured) => self.queue.push(Command::LoadMeasuredQe(None)),
            Some(PixelAction::Fit) => self.pixel.start_fit(Some(&ctx), self.project.as_ref()),
            None => {}
        }
        self.sweep_window(&ctx);
        self.monte_carlo_window(&ctx);
        self.verification_window(&ctx);
        self.silicon_window(&ctx);
        self.process_window(&ctx);
        self.process_data_window(&ctx);
        self.results_window(&ctx);
        self.net_window(&ctx);
        self.hier_window(&ctx);
        self.gen_window(&ctx);
        self.skill_form_window(&ctx);
        self.virtuoso_pick_window(&ctx);
        self.poll_virtuoso_job();
        self.virtuoso_job_window(&ctx);
        self.skill_job_window(&ctx);
        let progress = self.tutorial_progress();
        self.tutorial.track(&progress);
        let cmds = self.tutorial.window(&ctx, &progress);
        self.queue.extend(cmds);

        // While a SKILL script runs on a copy of the project, commands wait: its result replaces
        // the project when it ends.
        let cmds = if self.skill_job.is_none() {
            std::mem::take(&mut self.queue)
        } else {
            let (cancel, rest) = std::mem::take(&mut self.queue).into_iter().partition(|c| matches!(c, Command::CancelSkill));
            self.queue = rest;
            cancel
        };
        for c in cmds {
            self.run_command(&ctx, c);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.console = self.show_console;
        if let Ok(s) = serde_json::to_string(&self.settings) {
            storage.set_string(SETTINGS_KEY, s);
        }
    }
}

/// Layout by example, then optionally routing: the generator behind "By Example".
fn place_like(p: &mut Project, reference: &CellRef, target: &CellRef, route: bool) -> Result<String, String> {
    let rep = ics_core::by_example::place_like(p, reference, target)?;
    let mut msg = format!(
        "placed {} device(s), {} tap(s) and {} well / implant shape(s) like {reference}",
        rep.placed, rep.taps, rep.shapes
    );
    for n in &rep.notes {
        msg += &format!("; {n}");
    }
    if route && let Some(sch) = p.cell(target).and_then(|c| c.schematic.clone()) {
        let mut lay = p.cell_mut(target).and_then(|c| c.layout.take()).unwrap_or_default();
        let res = ics_core::route::route(&sch, &mut lay, p, &p.tech);
        if let Some(c) = p.cell_mut(target) {
            c.layout = Some(lay);
        }
        msg += &format!("; {}", res?.summary());
    }
    Ok(msg)
}
