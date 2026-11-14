//! Application shell: workspaces, project explorer, tabs, commands, dialogs and persistence.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Key, KeyboardShortcut, Modifiers, RichText, Sense, Stroke, Vec2};
use ics_core::{Cell, CellRef, Layout, Project, SchObj, Schematic, ViewKind, examples};
use ics_mcp::bridge::Pending;
use ics_mcp::{ToolResult, tools};
use ics_sim::SimOptions;
use serde::{Deserialize, Serialize};

use crate::calc_view::Calculator;
use crate::canvas::Profile;
use crate::canvas::View2D;
use crate::ciw::{Ciw, CiwKind};
use crate::fill_view::FillTool;
use crate::gen_view::{GenAction, GenTab, LayoutGenerators};
use crate::hier_view::{HierAction, HierBrowser};
use crate::home_view;
use crate::icons::{self, Icon};
use crate::layout_view::{EipContext, LayAction, LayCtx, LayRequest, LayTool, LayoutEditor, Verify, fmt_count, um};
use crate::net_view::{NetAction, NetBrowser};
use crate::rule_designer::{DesignerAction, RuleDesigner};
use crate::schematic_view::{SchTool, SchematicEditor, device_palette};
use crate::sim_view::{self, SimEvent, SimState};
use crate::stream_map_view::StreamMapEditor;
use crate::sweep_view::{SweepAction, SweepManager};
use crate::theme::{self, Palette};
use crate::update;
use ics_core::gds::StreamFormat;
use ics_core::hier::Hier;

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
    OpenProjectDialog,
    OpenProject(PathBuf),
    Save,
    SaveAs,
    CloseProject,
    NewCellDialog,
    OpenCell(CellRef, Option<Workspace>),
    CloseTab(CellRef),
    RenameCellDialog(CellRef),
    DeleteCellConfirm(CellRef),
    CreateView(CellRef, ViewKind),
    SetWorkspace(Workspace),
    RunSimulation,
    /// Open the Waveform Calculator on the active cell's results.
    Calculator,
    /// Evaluate a calculator expression on the active cell's shown dataset and plot waveforms.
    CalcPlot(String),
    /// Open the Corner / Sweep Manager on the active cell.
    SweepManager,
    /// Run the active cell's corner / parametric sweep.
    RunSweep,
    /// Open the Results Browser (earlier runs of the active cell, for comparison).
    ResultsBrowser,
    /// Check the active schematic (Check and Save: `true` also saves when it has no errors).
    CheckSchematic(bool),
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
    RunLvs,
    /// IR drop / EM analysis of the active layout: of the named net, else the Verify panel's.
    IrDrop(Option<String>),
    /// Open the DRC Rule Designer window on the technology's deck.
    RuleDesigner,
    /// Open the Stream Layer Map editor (GDSII / OASIS layer numbers on import and export).
    StreamMap,
    /// Open the Dummy Fill tool.
    DummyFill,
    /// Add dummy fill with the Dummy Fill tool's parameters to the active cell.
    AddFill,
    /// Run an external sign-off runset of the technology on the active cell: the one named
    /// (or uniquely matched), else the one selected in the Sign-off section.
    RunSignoff(Option<String>),
    GenerateLayout,
    RouteFlylines,
    /// Import a GDSII or OASIS file (detected from its contents); `None` asks for the file.
    ImportGds(Option<PathBuf>),
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
    /// Replace the technology with a PDK file (see `ics_core::pdk`); `None` asks for the file.
    LoadPdk(Option<PathBuf>),
    ExportNetlist,
    ToggleTheme,
    ToggleConsole,
    Settings,
    Shortcuts,
    UserGuide,
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
        }
    }
}

enum Dialog {
    NewProject { name: String, dir: String, demo: bool, error: String },
    NewCell { lib: String, name: String, description: String, schematic: bool, layout: bool, error: String },
    RenameCell { cell: CellRef, name: String, error: String },
    ConfirmDelete(CellRef),
    Unsaved { then: Box<Command> },
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

/// MCP tools that change layouts or the project; their edits invalidate editor state.
const MCP_EDITS: [&str; 13] = [
    "add_rect",
    "load_pdk",
    "add_path",
    "add_label",
    "place_instance",
    "place_pcell",
    "delete_items",
    "generate_layout",
    "import_gds",
    "import_lef",
    "import_def",
    "open_project",
    "new_project",
];

/// Every cell layout, to tell afterwards which ones a SKILL script changed.
fn layout_snapshot(p: &Project) -> HashMap<CellRef, Layout> {
    p.all_cells().filter_map(|(r, c)| Some((r, c.layout.clone()?))).collect()
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
        "route" => format!("{} connection(s), {} flyline(s) left", n("connections"), n("unrouted_flylines")),
        "delete_items" => format!("deleted {}", len("deleted")),
        "write_schematic" => format!("{} device(s), {} net(s)", n("devices"), len("nets")),
        "add_rect" | "add_path" | "add_label" | "place_instance" | "place_pcell" => format!("id {}", v["id"]),
        "open_project" | "new_project" | "project_info" => format!("project '{}'", v["name"].as_str().unwrap_or("?")),
        "import_gds" | "import_lef" => format!("library '{}'", v["library"].as_str().unwrap_or("?")),
        "import_def" => format!("cell '{}'", v["cell"].as_str().unwrap_or("?")),
        "run_skill" => format!("{} cell(s) modified", len("modified")),
        _ => "ok".into(),
    }
}

const GUIDE_URL: &str = "https://geniuskey.github.io/ic-studio/";

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
            ("Mod+A / Mod+D", "Select all / duplicate (layout: deselect all)"),
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
            ("Ctrl+J", "Flip sideways (also while moving)"),
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
            ("Shift+F / Mod+F", "All levels, top level only"),
            ("0 … 8, 9", "Drawn hierarchy depth, 9 = all"),
            ("Right-to-left box", "Select everything the box touches"),
        ],
    ),
];

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
    sch_eds: HashMap<CellRef, SchematicEditor>,
    lay_eds: HashMap<CellRef, LayoutEditor>,
    /// Cells descended into (Shift+X) or edited in place (x), for Shift+B (return).
    lay_return: Vec<Descent>,
    hier: Hier,
    /// Device names last selected in either editor, for cross-probing.
    probe: Option<(CellRef, BTreeSet<String>)>,
    sims: HashMap<CellRef, SimState>,
    log: Vec<LogLine>,
    started: Instant,
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
    copilot: crate::copilot::Copilot,
    ai_models: crate::llm::AiSettingsWindow,
    updater: crate::update::Updater,
    optimizer: crate::optimizer_view::Optimizer,
    stream_map: StreamMapEditor,
    fill: FillTool,
    calc: Calculator,
    sweep: SweepManager,
    results_open: bool,
    checks: Vec<ics_core::sch_check::Finding>,
    nets: NetBrowser,
    hier_browser: HierBrowser,
    gens: LayoutGenerators,
    ciw: Ciw,
}

const SETTINGS_KEY: &str = "ic-studio-settings";

impl IcStudio {
    pub fn new(cc: &eframe::CreationContext<'_>, args: crate::LaunchArgs) -> Self {
        let settings: Settings =
            cc.storage.and_then(|s| s.get_string(SETTINGS_KEY)).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        theme::setup_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, settings.dark);
        cc.egui_ctx.set_zoom_factor(settings.ui_scale);
        let ngspice = ics_sim::ngspice::detect(&settings.ngspice_path);
        let mut app = Self {
            project: None,
            dirty: false,
            workspace: Workspace::Home,
            tabs: vec![],
            active: None,
            sch_eds: HashMap::new(),
            lay_eds: HashMap::new(),
            lay_return: vec![],
            hier: Hier::default(),
            probe: None,
            sims: HashMap::new(),
            log: vec![],
            started: Instant::now(),
            show_console: true,
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
            copilot: Default::default(),
            ai_models: Default::default(),
            updater: Default::default(),
            optimizer: Default::default(),
            stream_map: StreamMapEditor::default(),
            fill: FillTool::default(),
            calc: Calculator::default(),
            sweep: SweepManager::default(),
            results_open: false,
            checks: vec![],
            nets: NetBrowser::default(),
            hier_browser: HierBrowser::default(),
            gens: LayoutGenerators::default(),
            ciw: Ciw::default(),
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
        app
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
        self.push_log(Level::Warn, t);
    }
    fn error(&mut self, t: String) {
        self.push_log(Level::Error, t);
        self.show_console = true;
        self.bottom_tab = BottomTab::Console;
    }

    // ----- project management -----
    fn set_project(&mut self, p: Project) {
        self.project = Some(p);
        self.ciw.reset();
        self.dirty = false;
        self.tabs.clear();
        self.active = None;
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
            Ok(p) => {
                let root = p.root.clone().unwrap();
                self.info(format!("Opened project '{}' ({})", p.name, root.display()));
                self.set_project(p);
                self.remember_recent(&root);
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
        match p.save() {
            Ok(()) => {
                let root = p.root.clone().unwrap();
                let name = p.name.clone();
                self.dirty = false;
                self.ok(format!("Saved '{name}' to {}", root.display()));
                self.remember_recent(&root);
                true
            }
            Err(e) => {
                self.error(format!("Save failed: {e}"));
                false
            }
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
        SimOptions { backend, ngspice_exe: self.settings.ngspice_path.clone() }
    }

    fn netlist_active(&self) -> Option<(String, Vec<String>)> {
        let r = self.active.as_ref()?;
        let cell = self.active_cell()?;
        let sch = cell.schematic.as_ref()?;
        let nl = ics_core::netlist::netlist_for(sch, &self.project.as_ref()?.tech, &cell.sim.corner);
        Some((nl.deck(&r.to_string(), &cell.sim), nl.warnings))
    }

    // ----- commands -----
    fn run_command(&mut self, ctx: &egui::Context, cmd: Command) {
        match cmd {
            Command::NewProjectDialog => {
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
                    let lib = self.active.as_ref().map(|r| r.lib.clone()).unwrap_or_else(|| p.libraries[0].name.clone());
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
            Command::RenameCellDialog(r) => {
                self.dialog = Some(Dialog::RenameCell { name: r.cell.clone(), cell: r, error: String::new() })
            }
            Command::DeleteCellConfirm(r) => self.dialog = Some(Dialog::ConfirmDelete(r)),
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
            Command::SweepManager => match (self.active.clone(), self.active_cell()) {
                (Some(r), Some(c)) if c.schematic.is_some() => {
                    let setup = c.sim.sweep.clone();
                    self.sweep.show(&r, &setup);
                }
                _ => self.warn("Open a cell with a schematic to sweep".into()),
            },
            Command::RunSweep => self.run_sweep(ctx),
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
            Command::Lay(a) => {
                if let Some(r) = self.active.clone() {
                    self.workspace = Workspace::Layout;
                    self.lay_eds.entry(r).or_default().actions.push(a);
                }
            }
            Command::RunDrc => self.verify(ctx, Verify::Drc),
            Command::ExtractNets => self.verify(ctx, Verify::Extract),
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
            Command::StreamMap => match &self.project {
                Some(p) => self.stream_map.show(&p.tech),
                None => self.warn("Open a project to edit the stream layer map".into()),
            },
            Command::DummyFill => match &self.project {
                Some(p) => self.fill.show(&p.tech),
                None => self.warn("Open a project to add dummy fill".into()),
            },
            Command::AddFill => self.add_fill(),
            Command::RunSignoff(name) => self.run_signoff(ctx, name.as_deref()),
            Command::GenerateLayout => self.generate_layout(),
            Command::RouteFlylines => self.route_flylines(),
            Command::ExportGds => self.export_stream(false, StreamFormat::Gds),
            Command::ExportLibraryGds => self.export_stream(true, StreamFormat::Gds),
            Command::ExportOasis => self.export_stream(false, StreamFormat::Oasis),
            Command::ExportLibraryOasis => self.export_stream(true, StreamFormat::Oasis),
            Command::ImportGds(path) => self.import_gds(path),
            Command::ExportLefDef(def) => self.export_lef_def(def),
            Command::ImportLefDef(def, path) => self.import_lef_def(def, path),
            Command::ImportTech => self.import_tech(),
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
            Command::ToggleConsole => self.show_console = !self.show_console,
            Command::Settings => self.dialog = Some(Dialog::Settings),
            Command::Shortcuts => self.dialog = Some(Dialog::Shortcuts),
            Command::UserGuide => ctx.open_url(egui::OpenUrl::new_tab(GUIDE_URL)),
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
        let Some(sch) = p.cell(&r).and_then(|c| c.schematic.clone()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let plan = ics_core::sdl::plan(&sch, &p.tech);
        let mut lay = p.cell_mut(&r).and_then(|c| c.layout.take()).unwrap_or_default();
        let ed = self.lay_eds.entry(r.clone()).or_default();
        ed.history.push(&lay);
        let res = ics_core::sdl::apply(p, &mut lay, &plan);
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
            Ok(names) if names.is_empty() => self.info("Every schematic device is already placed".into()),
            Ok(names) => {
                self.mark_dirty();
                self.ok(format!("Placed {} device(s): {}", names.len(), names.join(", ")));
            }
            Err(e) => self.error(format!("Generate layout: {e}")),
        }
        self.workspace = Workspace::Layout;
    }

    fn route_flylines(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(sch) = p.cell(&r).and_then(|c| c.schematic.clone()) else {
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
                .filter(|i| i.name.as_deref().is_some_and(|n| names.contains(ics_core::sdl::device_of(n))))
                .map(|i| i.id)
                .collect();
            if !ids.is_empty() {
                self.lay_eds.entry(r).or_default().selection = ids;
            }
        } else {
            let objs: BTreeSet<SchObj> =
                sch.instances.iter().filter(|i| names.contains(&i.name)).map(|i| SchObj::Instance(i.id)).collect();
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

    /// The decks of every point of the sweep draft on cell `r`.
    #[allow(clippy::type_complexity)]
    fn sweep_decks(
        &self,
        r: &CellRef,
        notes: &mut Vec<String>,
    ) -> Result<(Vec<(ics_core::sweep::SweepPoint, String)>, Vec<ics_core::Analysis>, ics_core::Backend), String> {
        let p = self.project.as_ref().ok_or("no project")?;
        let cell = p.cell(r).ok_or_else(|| format!("no cell {r}"))?;
        let sch = cell.schematic.as_ref().ok_or_else(|| format!("{r} has no schematic to sweep"))?;
        let mut setup = cell.sim.clone();
        setup.sweep = self.sweep.setup.clone();
        let analyses: Vec<_> = setup.enabled().cloned().collect();
        if analyses.is_empty() {
            return Err("no analyses enabled".into());
        }
        let points = setup.sweep.expand(&setup, sch)?;
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

    fn import_gds(&mut self, path: Option<PathBuf>) {
        let Some(p) = self.project.as_mut() else { return };
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
        let t0 = Instant::now();
        let opts = ics_core::gds::ReadOptions { add_unknown_layers: true, use_stream_map: true };
        let label = StreamFormat::detect(&data).label();
        let imp = match ics_core::gds::read_stream_with(&data, &p.tech, "gds", opts) {
            Ok(imp) => imp,
            Err(e) => {
                self.error(format!("{label} import failed: {e}"));
                return;
            }
        };
        let skipped = imp.skipped_unknown_layers;
        let added = imp.new_layers.len();
        let warnings = imp.warnings.clone();
        let n = imp.cells.len();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        match p.add_gds_library(&stem, imp) {
            Ok((lib, tops)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(format!(
                    "Imported {n} cell(s) from {} into library '{lib}' in {:.2} s",
                    path.display(),
                    t0.elapsed().as_secs_f64()
                ));
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
                if skipped > 0 {
                    self.warn(format!("{skipped} shape(s) on layers not in the technology were skipped"));
                }
                if added > 0 {
                    self.warn(format!("{added} layer(s) not in the technology were added to it"));
                }
                if let Some(r) = tops.into_iter().next() {
                    self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
                }
            }
            Err(e) => self.error(format!("{label} import failed: {e}")),
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
        let current = self.active.clone();
        let Some(p) = self.project.as_mut() else { return };
        let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter("Cadence SKILL", &["il", "ils"]).pick_file()) else {
            return;
        };
        let before = layout_snapshot(p);
        let res = self.ciw.session.load(p, &path, current.as_ref());
        self.ciw.push(CiwKind::Input, format!("load(\"{}\")", path.display().to_string().replace('\\', "/")));
        self.ciw.show(&res);
        let (output, err) = match &res {
            Ok(r) => (r.output.clone(), None),
            Err(e) => (e.output.clone(), Some(e.to_string())),
        };
        for line in output.lines() {
            self.info(format!("SKILL: {line}"));
        }
        let changed = self.layouts_edited(before);
        match err {
            None => self.ok(format!("Loaded {}: {} cell(s) modified", path.display(), changed.len())),
            Some(e) => {
                self.error(format!("SKILL error: {e}"));
                if !changed.is_empty() {
                    self.warn(format!("Edits made before the error are kept in {} cell(s)", changed.len()));
                }
            }
        }
        if self.active.is_none()
            && let Some(r) = changed.into_iter().next()
        {
            self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
        }
    }

    /// Evaluate a CIW command in the persistent SKILL session.
    fn ciw_eval(&mut self, src: &str) {
        let current = self.active.clone();
        let Some(p) = self.project.as_mut() else {
            self.ciw.push(CiwKind::Error, "*Error* no project is open");
            return;
        };
        let before = layout_snapshot(p);
        let res = self.ciw.session.eval(p, src, None, current.as_ref());
        self.ciw.show(&res);
        let changed = self.layouts_edited(before);
        if self.active.is_none()
            && let Some(r) = changed.into_iter().next()
        {
            self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
        }
    }

    /// After an external edit of any number of layouts (a SKILL script): record undo history
    /// for each changed cell that had a layout, refresh the editors and mark the project dirty.
    /// Returns the changed cells.
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
        let q = &mut self.queue;
        let has_project = self.project.is_some();
        let lay = self.workspace == Workspace::Layout && self.active.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Project…").clicked() {
                    q.push(Command::NewProjectDialog);
                }
                if ui.button("New Demo Project").clicked() {
                    q.push(Command::NewDemoProject);
                }
                if ui.add(egui::Button::new("Open Project…").shortcut_text(theme::kbd("⌘O"))).clicked() {
                    q.push(Command::OpenProjectDialog);
                }
                ui.menu_button("Open Recent", |ui| {
                    if self.settings.recent.is_empty() {
                        ui.label("No recent projects");
                    }
                    for p in &self.settings.recent {
                        if ui.button(p.display().to_string()).clicked() {
                            q.push(Command::OpenProject(p.clone()));
                        }
                    }
                });
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new("Save").shortcut_text(theme::kbd("⌘S / F2"))).clicked() {
                    q.push(Command::Save);
                }
                if ui.add_enabled(has_project, egui::Button::new("Save As…").shortcut_text(theme::kbd("⇧⌘S"))).clicked() {
                    q.push(Command::SaveAs);
                }
                if ui
                    .add_enabled(has_project, egui::Button::new("Check and Save"))
                    .on_hover_text("Check the active schematic, save if it has no errors")
                    .clicked()
                {
                    q.push(Command::CheckSchematic(true));
                }
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new("New Cell…").shortcut_text(theme::kbd("⌘N"))).clicked() {
                    q.push(Command::NewCellDialog);
                }
                ui.menu_button("Import", |ui| {
                    if ui.add_enabled(has_project, egui::Button::new("GDSII / OASIS…")).clicked() {
                        q.push(Command::ImportGds(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new("LEF abstracts…")).clicked() {
                        q.push(Command::ImportLefDef(false, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new("DEF placement…")).clicked() {
                        q.push(Command::ImportLefDef(true, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new("KLayout layer properties…")).clicked() {
                        q.push(Command::ImportTech);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("PDK…")).clicked() {
                        q.push(Command::LoadPdk(None));
                    }
                });
                ui.menu_button("Export", |ui| {
                    if ui.add_enabled(has_project, egui::Button::new("SPICE netlist…")).clicked() {
                        q.push(Command::ExportNetlist);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Cell GDSII…")).clicked() {
                        q.push(Command::ExportGds);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Cell OASIS…")).clicked() {
                        q.push(Command::ExportOasis);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Library GDSII…")).clicked() {
                        q.push(Command::ExportLibraryGds);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Library OASIS…")).clicked() {
                        q.push(Command::ExportLibraryOasis);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Cell LEF abstract…")).clicked() {
                        q.push(Command::ExportLefDef(false));
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Cell DEF…")).clicked() {
                        q.push(Command::ExportLefDef(true));
                    }
                });
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new("Close Project")).clicked() {
                    q.push(Command::CloseProject);
                }
                if ui.add(egui::Button::new("Settings…").shortcut_text(theme::kbd("⌘,"))).clicked() {
                    q.push(Command::Settings);
                }
                if ui.add(egui::Button::new("Quit").shortcut_text(theme::kbd("⌘Q"))).clicked() {
                    q.push(Command::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add(egui::Button::new("Undo").shortcut_text(theme::kbd(if lay { "u / ⌘Z" } else { "⌘Z" }))).clicked() {
                    q.push(Command::Undo);
                }
                if ui
                    .add(egui::Button::new("Redo").shortcut_text(theme::kbd(if lay { "Shift+U / ⇧⌘Z" } else { "⇧⌘Z" })))
                    .clicked()
                {
                    q.push(Command::Redo);
                }
                ui.separator();
                if ui.add(egui::Button::new("Delete").shortcut_text("Del")).clicked() {
                    q.push(Command::Delete);
                }
                if lay {
                    ui.separator();
                    lay_menu_items(ui, "Edit", q);
                }
            });
            ui.menu_button("View", |ui| {
                for (i, w) in Workspace::ALL.iter().enumerate() {
                    if ui.add(egui::Button::new(w.name()).shortcut_text(theme::kbd(&format!("⌘{}", i + 1)))).clicked() {
                        q.push(Command::SetWorkspace(*w));
                    }
                }
                ui.separator();
                if ui.add(egui::Button::new("Fit to Window").shortcut_text("F")).clicked() {
                    q.push(Command::FitView);
                }
                if ui.add(egui::Button::new("Toggle Console").shortcut_text(theme::kbd("⌘J"))).clicked() {
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
                ui.menu_button("Create", |ui| lay_menu_items(ui, "Create", q));
            }
            ui.menu_button("Simulation", |ui| {
                if ui.add(egui::Button::new("Run").shortcut_text("F5")).clicked() {
                    q.push(Command::RunSimulation);
                }
                if ui.button("Export Netlist…").clicked() {
                    q.push(Command::ExportNetlist);
                }
                ui.separator();
                if ui.button("Waveform Calculator…").clicked() {
                    q.push(Command::Calculator);
                }
                if ui.button("Corner / Sweep Manager…").clicked() {
                    q.push(Command::SweepManager);
                }
            });
            ui.menu_button("Verify", |ui| {
                if ui.button("Run DRC").clicked() {
                    q.push(Command::RunDrc);
                }
                if ui.button("Extract Nets").clicked() {
                    q.push(Command::ExtractNets);
                }
                if ui.button("Run LVS").clicked() {
                    q.push(Command::RunLvs);
                }
                if lay && ui.add(egui::Button::new("Mark Net").shortcut_text("Shift+N")).clicked() {
                    q.push(Command::Lay(LayAction::MarkNet));
                }
                if ui.button("Run Sign-off Runset").clicked() {
                    q.push(Command::RunSignoff(None));
                }
                ui.separator();
                if ui.button("Generate Layout from Schematic").clicked() {
                    q.push(Command::GenerateLayout);
                }
                if ui.button("Route Flylines").clicked() {
                    q.push(Command::RouteFlylines);
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.button("Design Copilot…").clicked() {
                    q.push(Command::Copilot);
                }
                if ui.button("AI Models…").clicked() {
                    q.push(Command::AiModels);
                }
                if ui.button("Optimizer…").clicked() {
                    q.push(Command::Optimizer);
                }
                if ui.button("Waveform Calculator…").clicked() {
                    q.push(Command::Calculator);
                }
                if ui.button("Corner / Sweep Manager…").clicked() {
                    q.push(Command::SweepManager);
                }
                if ui.button("Results Browser…").clicked() {
                    q.push(Command::ResultsBrowser);
                }
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new("Check Schematic")).clicked() {
                    q.push(Command::CheckSchematic(false));
                }
                if ui.add_enabled(has_project, egui::Button::new("Hierarchy Browser…")).clicked() {
                    q.push(Command::HierarchyBrowser);
                }
                if ui.add_enabled(has_project, egui::Button::new("Net Browser…")).clicked() {
                    q.push(Command::NetBrowser);
                }
                ui.menu_button("Layout Generators", |ui| {
                    if ui.add_enabled(has_project, egui::Button::new("Guard Ring…")).clicked() {
                        q.push(Command::GuardRing);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Via Array…")).clicked() {
                        q.push(Command::ViaArray);
                    }
                    if ui.add_enabled(has_project, egui::Button::new("Matched Placement…")).clicked() {
                        q.push(Command::MatchedPlacement);
                    }
                });
                ui.separator();
                if ui.button("DRC Rule Designer…").clicked() {
                    q.push(Command::RuleDesigner);
                }
                if ui.add_enabled(has_project, egui::Button::new("Stream Layer Map…")).clicked() {
                    q.push(Command::StreamMap);
                }
                if ui.add_enabled(has_project, egui::Button::new("Dummy Fill…")).clicked() {
                    q.push(Command::DummyFill);
                }
                ui.separator();
                if ui.button("SKILL CIW").clicked() {
                    q.push(Command::Ciw);
                }
                if ui.add_enabled(has_project, egui::Button::new("Load SKILL (.il)…")).clicked() {
                    q.push(Command::LoadSkill(None));
                }
                ui.separator();
                if ui.add(egui::Button::new("Command Palette").shortcut_text(theme::kbd("⌘K"))).clicked() {
                    q.push(Command::CommandPalette);
                }
                if ui.add(egui::Button::new("Settings…").shortcut_text(theme::kbd("⌘,"))).clicked() {
                    q.push(Command::Settings);
                }
            });
            if lay {
                ui.menu_button("Options", |ui| lay_menu_items(ui, "Options", q));
            }
            ui.menu_button("Help", |ui| {
                if ui.button("User Guide").clicked() {
                    q.push(Command::UserGuide);
                }
                if ui.add(egui::Button::new("Keyboard Shortcuts").shortcut_text("F1")).clicked() {
                    q.push(Command::Shortcuts);
                }
                if ui.add(egui::Button::new("Command Palette").shortcut_text(theme::kbd("⌘K"))).clicked() {
                    q.push(Command::CommandPalette);
                }
                ui.separator();
                if ui.button("Check for Updates…").clicked() {
                    q.push(Command::CheckForUpdates);
                }
                if ui.button("About IC Studio").clicked() {
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
                let b = egui::Button::new(RichText::new(theme::kbd("🔍  Search commands…   ⌘K")).color(pal.text_weak))
                    .min_size(Vec2::new(220.0, 22.0));
                if ui.add(b).clicked() {
                    q.push(Command::CommandPalette);
                }
            });
        });
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        ui.add_space(6.0);
        for w in Workspace::ALL {
            let active = self.workspace == w;
            let (r, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 48.0), Sense::click());
            let color = w.color(&pal);
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
            ui.label(RichText::new("No project open").color(pal.text_weak));
            if ui.button("Open demo project").clicked() {
                self.queue.push(Command::NewDemoProject);
            }
            return;
        };
        ui.horizontal(|ui| {
            theme::section(ui, "Explorer");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+").on_hover_text(theme::kbd("New cell (⌘N)")).clicked() {
                    self.queue.push(Command::NewCellDialog);
                }
            });
        });
        for lib in &p.libraries {
            let id = ui.make_persistent_id(("lib", &lib.name));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    let (r, painter) = ui.allocate_painter(Vec2::splat(16.0), Sense::hover());
                    icons::paint(&painter, r.rect, Icon::Library, pal.text_weak);
                    ui.label(RichText::new(&lib.name).strong());
                })
                .body(|ui| {
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
                                    self.queue.push(Command::OpenCell(r.clone(), None));
                                }
                                resp.context_menu(|ui| {
                                    if ui.button("Open schematic").clicked() {
                                        self.queue.push(if cell.schematic.is_some() {
                                            Command::OpenCell(r.clone(), Some(Workspace::Schematic))
                                        } else {
                                            Command::CreateView(r.clone(), ViewKind::Schematic)
                                        });
                                    }
                                    if ui.button("Open layout").clicked() {
                                        self.queue.push(if cell.layout.is_some() {
                                            Command::OpenCell(r.clone(), Some(Workspace::Layout))
                                        } else {
                                            Command::CreateView(r.clone(), ViewKind::Layout)
                                        });
                                    }
                                    ui.separator();
                                    if ui.button("Rename…").clicked() {
                                        self.queue.push(Command::RenameCellDialog(r.clone()));
                                    }
                                    if ui.button("Delete…").clicked() {
                                        self.queue.push(Command::DeleteCellConfirm(r.clone()));
                                    }
                                });
                                if !cell.description.is_empty() {
                                    resp.on_hover_text(&cell.description);
                                }
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
                            resp.on_hover_text(t.to_string());
                            if ui.add(egui::Button::new(RichText::new("×").color(pal.text_weak)).frame(false).small()).clicked()
                            {
                                self.queue.push(Command::CloseTab(t.clone()));
                            }
                        });
                    });
            }
        });
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
        let Some(cell) = project.cell_mut(&r) else {
            self.queue.push(Command::CloseTab(r));
            return;
        };
        let mut changed = false;
        match ws {
            Workspace::Schematic => {
                let op = self.sims.get(&r).and_then(|s| s.op_annotation());
                let ed = self.sch_eds.entry(r.clone()).or_default();
                if let Some(sch) = cell.schematic.as_mut() {
                    egui::Panel::left("sch_tools").resizable(true).default_size(220.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| device_palette(ui, ed));
                    });
                    egui::Panel::right("sch_inspector").resizable(true).default_size(250.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| changed |= ed.inspector(ui, sch));
                    });
                    egui::Panel::top("sch_toolbar").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(ed.tool.name()).strong().color(pal.schematic));
                            ui.separator();
                            if ui
                                .add_enabled(ed.history.can_undo(), egui::Button::new("↶"))
                                .on_hover_text(theme::kbd("Undo (⌘Z)"))
                                .clicked()
                            {
                                changed |= ed.undo(sch);
                            }
                            if ui
                                .add_enabled(ed.history.can_redo(), egui::Button::new("↷"))
                                .on_hover_text(theme::kbd("Redo (⇧⌘Z)"))
                                .clicked()
                            {
                                changed |= ed.redo(sch);
                            }
                            ui.separator();
                            if ui.button("⟳").on_hover_text("Rotate selection (Space)").clicked() {
                                changed |= ed.rotate_selection(sch, false);
                            }
                            if ui.button("⇋").on_hover_text("Mirror selection (X)").clicked() {
                                changed |= ed.rotate_selection(sch, true);
                            }
                            if ui.button("⎘").on_hover_text(theme::kbd("Duplicate selection (⌘D)")).clicked() {
                                changed |= ed.duplicate_selection(sch);
                            }
                            if ui.button("⛶").on_hover_text("Fit to window (F)").clicked() {
                                ed.request_fit();
                            }
                            ui.separator();
                            ui.checkbox(&mut ed.show_grid, "Grid");

                            ui.add_enabled(op.is_some(), egui::Checkbox::new(&mut ed.annotate_op, "Annotate OP"))
                                .on_disabled_hover_text("Run an operating point simulation first");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(RichText::new("▶ Simulate").color(pal.simulation)).on_hover_text("F5").clicked() {
                                    self.queue.push(Command::RunSimulation);
                                }
                            });
                        });
                    });
                    egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                        changed |= ed.canvas(ui, sch, op.as_ref()).changed;
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
                    egui::Panel::left("lay_tools").resizable(true).default_size(230.0).show(ui, |ui| {
                        if let Some(o) = ed.layer_palette(ui, l, &mut cx) {
                            open = Some(o);
                        }
                    });
                    egui::Panel::right("lay_inspector").resizable(true).default_size(250.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let (c, o) = ed.inspector(ui, l, &mut cx);
                            changed |= c;
                            open = open.take().or(o);
                        });
                    });
                    egui::Panel::top("lay_toolbar").show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(ed.tool.name()).strong().color(pal.layout));
                            ui.label(RichText::new(format!("on {}", tech.layers[ed.active_layer].name)).color(pal.text_weak));
                            ui.separator();
                            let undo = ui
                                .add_enabled(ed.history.can_undo(), egui::Button::new("↶"))
                                .on_hover_text(theme::kbd("Undo (⌘Z)"));
                            let redo = ui
                                .add_enabled(ed.history.can_redo(), egui::Button::new("↷"))
                                .on_hover_text(theme::kbd("Redo (⇧⌘Z)"));
                            if (undo.clicked() && ed.undo(l)) || (redo.clicked() && ed.redo(l)) {
                                changed = true;
                                cx.hier.invalidate(&r);
                            }
                            ui.separator();
                            for t in LayTool::ALL {
                                if ui
                                    .selectable_label(ed.tool == t, t.name())
                                    .on_hover_text(format!("{} ({})", t.name(), t.key()))
                                    .clicked()
                                {
                                    ed.set_tool(t);
                                }
                            }
                            ui.separator();
                            if ui.button("⛶").on_hover_text("Fit to window (F)").clicked() {
                                ed.request_fit();
                            }
                            if ui.button("Zoom box").on_hover_text("Drag or click a box to zoom into (Z)").clicked() {
                                ed.arm_zoom_box();
                            }
                            if ui
                                .add_enabled(ed.can_step_view(true), egui::Button::new("Prev view"))
                                .on_hover_text("Previous view (W)")
                                .clicked()
                            {
                                ed.step_view(true);
                            }
                            ui.separator();
                            ui.checkbox(&mut ed.show_grid, "Grid");
                            ed.snap_control(ui, &tech);
                            ui.checkbox(&mut ed.gravity, "Gravity").on_hover_text("Snap the cursor to nearby vertices and edges");
                            ui.checkbox(&mut ed.drd, "DRD").on_hover_text(
                                "Design-rule-driven editing: show spacing and width violations while drawing and moving",
                            );
                            ui.checkbox(&mut ed.show_constraints, "Constraints")
                                .on_hover_text("Show symmetry axes, alignments and fixed items");
                            ui.checkbox(&mut ed.magnet, "Magnet").on_hover_text(
                                "Pull drawn and moved shapes to exactly the minimum spacing from their neighbours",
                            );
                            if ui
                                .checkbox(&mut ed.live.enabled, "Live DRC")
                                .on_hover_text("Check the rule deck around every edit; Ctrl+. offers quick fixes")
                                .changed()
                            {
                                ed.live.reset();
                            }
                            ui.checkbox(&mut ed.show_flylines, "Flylines")
                                .on_hover_text("Unrouted connections from the schematic");
                            ui.separator();
                            if ui.button("⬇ GDS").on_hover_text("Export this cell as GDSII").clicked() {
                                self.queue.push(Command::ExportGds);
                            }
                            if let Some(c) = cx.context {
                                ui.separator();
                                ui.label(RichText::new(format!("Editing in place in {}", c.root.cell)).color(pal.accent));
                                if ui.button("Return").on_hover_text("Leave edit-in-place (Shift+B)").clicked() {
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
                        LayRequest::SetDeck(text) => {
                            project.tech.drc_deck = Some(text);
                            changed = true;
                            log.push((true, "Saved the DRC rule deck to the technology".into()));
                        }
                        LayRequest::PlacePCell { pcell, trans, array } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                ed.selection = [lay.add_instance(cr, trans, array)].into();
                                touched = true;
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::SetPCell { inst, pcell } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                if let Some(i) = lay.instance_mut(inst) {
                                    i.cell = cr;
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
                egui::Panel::left("sim_setup").resizable(true).default_size(270.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let (c, rr) = sim_view::setup_panel(ui, cell, st, &self.ngspice, tech.models.as_ref());
                        changed |= c;
                        run = rr;
                    });
                });
                if run {
                    self.queue.push(Command::RunSimulation);
                }
                let deck = st.show_netlist.then(|| {
                    let nl = ics_core::netlist::netlist_for(cell.schematic.as_ref().unwrap(), &tech, &cell.sim.corner);
                    nl.deck(&r.to_string(), &cell.sim)
                });
                if let Some(deck) = deck {
                    egui::Panel::right("sim_netlist").resizable(true).default_size(340.0).show(ui, |ui| {
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
        let mut v: Vec<(String, &'static str, Command)> = vec![
            ("Run simulation".into(), "F5", Command::RunSimulation),
            ("Save project".into(), "⌘S", Command::Save),
            ("Save project as…".into(), "⇧⌘S", Command::SaveAs),
            ("Open project…".into(), "⌘O", Command::OpenProjectDialog),
            ("New project…".into(), "", Command::NewProjectDialog),
            ("New demo project".into(), "", Command::NewDemoProject),
            ("New cell…".into(), "⌘N", Command::NewCellDialog),
            ("Run DRC".into(), "", Command::RunDrc),
            ("Extract nets".into(), "", Command::ExtractNets),
            ("Run LVS".into(), "", Command::RunLvs),
            ("IR drop / EM analysis".into(), "", Command::IrDrop(None)),
            ("DRC rule designer".into(), "", Command::RuleDesigner),
            ("Stream layer map (GDSII / OASIS layer numbers)".into(), "", Command::StreamMap),
            ("Dummy fill (metal density)".into(), "", Command::DummyFill),
            ("Waveform calculator".into(), "", Command::Calculator),
            ("Corner / sweep manager".into(), "", Command::SweepManager),
            ("Optimizer (size devices to meet specs)".into(), "", Command::Optimizer),
            ("Results browser (compare runs)".into(), "", Command::ResultsBrowser),
            ("Check schematic".into(), "", Command::CheckSchematic(false)),
            ("Check and save".into(), "", Command::CheckSchematic(true)),
            ("Net browser".into(), "", Command::NetBrowser),
            ("Hierarchy browser".into(), "", Command::HierarchyBrowser),
            ("Create guard ring".into(), "", Command::GuardRing),
            ("Create via array".into(), "", Command::ViaArray),
            ("Matched placement (common centroid / interdigitated)".into(), "", Command::MatchedPlacement),
            ("Run corner / parametric sweep".into(), "", Command::RunSweep),
            ("Run sign-off runset (KLayout / Calibre / netgen)".into(), "", Command::RunSignoff(None)),
            ("Generate layout from schematic".into(), "", Command::GenerateLayout),
            ("Route flylines".into(), "", Command::RouteFlylines),
            ("Export cell GDSII…".into(), "", Command::ExportGds),
            ("Export library GDSII…".into(), "", Command::ExportLibraryGds),
            ("Export cell OASIS…".into(), "", Command::ExportOasis),
            ("Export library OASIS…".into(), "", Command::ExportLibraryOasis),
            ("Import GDSII / OASIS…".into(), "", Command::ImportGds(None)),
            ("Export cell LEF abstract…".into(), "", Command::ExportLefDef(false)),
            ("Export cell DEF…".into(), "", Command::ExportLefDef(true)),
            ("Import LEF abstracts…".into(), "", Command::ImportLefDef(false, None)),
            ("Import DEF placement…".into(), "", Command::ImportLefDef(true, None)),
            ("Import technology (KLayout .lyp)…".into(), "", Command::ImportTech),
            ("Load PDK…".into(), "", Command::LoadPdk(None)),
            ("Load SKILL (.il)…".into(), "", Command::LoadSkill(None)),
            ("SKILL command interpreter (CIW)".into(), "", Command::Ciw),
            ("Design copilot (AI chat)".into(), "", Command::Copilot),
            ("AI models (provider, model, API key)…".into(), "", Command::AiModels),
            ("Export SPICE netlist…".into(), "", Command::ExportNetlist),
            ("Toggle light / dark theme".into(), "", Command::ToggleTheme),
            ("Toggle console".into(), "⌘J", Command::ToggleConsole),
            ("Fit view".into(), "F", Command::FitView),
            ("Settings".into(), "⌘,", Command::Settings),
            ("Keyboard shortcuts".into(), "F1", Command::Shortcuts),
            ("Open user guide (web)".into(), "", Command::UserGuide),
            ("About IC Studio".into(), "", Command::About),
            ("Check for updates".into(), "", Command::CheckForUpdates),
            ("Add IC Studio MCP server to Claude Code".into(), "", Command::AddMcpToClaudeCode),
            ("Copy MCP server config".into(), "", Command::CopyMcpConfig),
        ];
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
            r.request_focus();
            if r.changed() {
                st.sel = 0;
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, (name, sc, cmd)) in items.iter().enumerate() {
                    let selected = i == st.sel;
                    let (resp, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 26.0), Sense::click());
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
                    ui.heading("New project");
                    ui.add_space(8.0);
                    egui::Grid::new("np").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                        ui.end_row();
                        ui.label("Location");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(dir).desired_width(220.0));
                            if ui.button("…").clicked()
                                && let Some(d) = rfd::FileDialog::new().pick_folder()
                            {
                                *dir = d.display().to_string();
                            }
                        });
                        ui.end_row();
                        ui.label("Template");
                        ui.horizontal(|ui| {
                            ui.radio_value(demo, false, "Empty");
                            ui.radio_value(demo, true, "Demo designs");
                        });
                        ui.end_row();
                    });
                    ui.label(
                        RichText::new(format!("Will be created at {}", Path::new(dir.as_str()).join(name.as_str()).display()))
                            .small()
                            .color(pal.text_weak),
                    );
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new("Create").strong()).clicked() {
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
                    ui.heading("New cell");
                    ui.add_space(8.0);
                    egui::Grid::new("nc").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label("Library");
                        egui::ComboBox::from_id_salt("nc_lib").selected_text(lib.as_str()).show_ui(ui, |ui| {
                            if let Some(p) = &self.project {
                                for l in &p.libraries {
                                    ui.selectable_value(lib, l.name.clone(), &l.name);
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Cell name");
                        let r = ui.text_edit_singleline(name);
                        if name.is_empty() {
                            r.request_focus();
                        }
                        ui.end_row();
                        ui.label("Description");
                        ui.text_edit_singleline(description);
                        ui.end_row();
                        ui.label("Views");
                        ui.horizontal(|ui| {
                            ui.checkbox(schematic, "schematic");
                            ui.checkbox(layout, "layout");
                        });
                        ui.end_row();
                    });
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new("Create").strong()).clicked() || enter {
                            let mut c = Cell::new(name.trim());
                            c.description = description.clone();
                            if *schematic {
                                c.schematic = Some(Schematic::default());
                            }
                            if *layout {
                                c.layout = Some(Layout::default());
                            }
                            let ws = if *schematic { Workspace::Schematic } else { Workspace::Layout };
                            match self.project.as_mut().map(|p| p.add_cell(lib, c)) {
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
                    ui.heading("Rename cell");
                    ui.text_edit_singleline(name);
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button("Rename").clicked()
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
                Dialog::ConfirmDelete(r) => {
                    ui.heading("Delete cell?");
                    ui.label(format!("'{r}' and all of its views will be permanently deleted, including files on disk."));
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
                            self.queue.push((**then).clone());
                            close = true;
                        }
                        if ui.button(RichText::new("Save").strong()).clicked() {
                            if self.save(false) {
                                self.queue.push((**then).clone());
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
    pub fn check_updates_on_start(&mut self, ctx: &egui::Context) {
        if self.settings.check_updates {
            self.updater.check(ctx, false);
        }
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
            let background = tools::BACKGROUND.contains(&next.tool.as_str());
            if background && b.job.is_some() {
                return;
            }
            let p = b.waiting.pop_front().expect("front exists");
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
        let edits = MCP_EDITS.contains(&tool);
        let before = cell.as_deref().filter(|_| edits).and_then(|c| {
            let p = self.project.as_ref()?;
            let r = tools::find(p, c)?;
            Some((p.cell(&r)?.layout.clone()?, r))
        });
        let sch_before = cell.as_deref().filter(|_| tool == "write_schematic").and_then(|c| {
            let p = self.project.as_ref()?;
            let r = tools::find(p, c)?;
            Some((p.cell(&r)?.schematic.clone().unwrap_or_default(), r))
        });
        let skill_before = self.project.as_ref().filter(|_| tool == "run_skill").map(layout_snapshot);
        let mut state = tools::State::in_gui(self.project.take(), self.dirty);
        let res = ics_mcp::call_tool(&mut state, tool, args);
        let (project, dirty) = state.into_parts();
        match project {
            Some(p) if !res.is_error && matches!(tool, "open_project" | "new_project") => {
                if self.dirty {
                    self.warn("MCP replaced the project; unsaved changes were discarded".into());
                }
                let root = p.root.clone();
                self.set_project(p);
                if let Some(root) = root {
                    self.remember_recent(&root);
                }
            }
            p => self.project = p,
        }
        self.dirty = dirty;
        if edits && !res.is_error {
            self.hier.clear();
            if let Some(p) = &self.project {
                if let Some((lay, r)) = before
                    && p.cell(&r).and_then(|c| c.layout.as_ref()) != Some(&lay)
                {
                    self.lay_eds.entry(r).or_default().history.push(&lay);
                }
                for (r, ed) in self.lay_eds.iter_mut() {
                    ed.layout_changed();
                    let lay = p.cell(r).and_then(|c| c.layout.as_ref());
                    ed.selection.retain(|id| lay.is_some_and(|l| l.ids().any(|i| i == *id)));
                }
            }
        }
        if let Some((sch, r)) = sch_before.filter(|_| !res.is_error) {
            let ed = self.sch_eds.entry(r).or_default();
            ed.history.push(&sch);
            ed.selection.clear();
            ed.request_fit();
        }
        let mut target = cell.as_deref().and_then(|c| self.project.as_ref().and_then(|p| tools::find(p, c)));
        if let Some(before) = skill_before {
            target = self.layouts_edited(before).into_iter().next().filter(|_| self.active.is_none());
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
                "get_netlist" | "write_schematic" => Workspace::Schematic,
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

    /// The layout editor of a cell, once it was opened (used by UI tests).
    pub fn layout_editor(&self, r: &CellRef) -> Option<&LayoutEditor> {
        self.lay_eds.get(r)
    }

    /// True while a simulation, a verification step or an MCP tool call is still running.
    pub fn is_busy(&self) -> bool {
        self.is_simulating()
            || self.lay_eds.values().any(LayoutEditor::is_verifying)
            || self.bridge.as_ref().is_some_and(|b| b.job.is_some() || !b.waiting.is_empty())
            || self.rule_designer.is_checking()
            || self.sweep.is_running()
            || self.copilot.busy()
            || self.optimizer.is_running()
    }

    fn rule_designer_window(&mut self, ctx: &egui::Context) {
        let active = self.active_layout();
        let Some(action) = self.rule_designer.window(ctx, self.project.as_ref(), active.as_ref()) else { return };
        let (DesignerAction::Apply(text) | DesignerAction::ApplyAndRun(text)) = &action;
        if let Some(p) = self.project.as_mut() {
            p.tech.drc_deck = Some(text.clone());
        }
        for ed in self.lay_eds.values_mut() {
            ed.verify.deck = None;
        }
        self.mark_dirty();
        self.ok("Rule Designer: applied the DRC deck to the technology".into());
        if matches!(action, DesignerAction::ApplyAndRun(_)) {
            self.queue.push(Command::RunDrc);
        }
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

    pub fn calculator(&self) -> &Calculator {
        &self.calc
    }

    pub fn sim_state(&self, r: &CellRef) -> Option<&SimState> {
        self.sims.get(r)
    }

    pub fn fill_tool(&mut self) -> &mut FillTool {
        &mut self.fill
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
        // Window close with unsaved work.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Unsaved { then: Box::new(Command::Quit) });
        }
        if let Some(ev) = self.updater.poll() {
            let verbose = self.updater.verbose;
            match ev {
                update::Event::UpToDate if verbose => self.info(format!("IC Studio {} is up to date", update::CURRENT)),
                update::Event::Available(v) => self.info(format!("IC Studio {v} is available (Help › Check for Updates)")),
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
        self.copilot.poll();
        if let Some(rx) = &self.copilot.calls {
            let calls: Vec<Pending> = rx.try_iter().collect();
            if !calls.is_empty() {
                let b = self.bridge.get_or_insert_with(|| Bridge {
                    rx: std::sync::mpsc::channel().1,
                    calls: 0,
                    last: None,
                    waiting: VecDeque::new(),
                    job: None,
                });
                b.waiting.extend(calls);
            }
        }
        self.serve_bridge(ctx);
        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if self.dialog.is_none() && self.palette.is_none() {
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
        egui::Panel::left("explorer")
            .resizable(true)
            .default_size(230.0)
            .size_range(160.0..=420.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(10))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.explorer(ui));
            });
        if self.show_console {
            egui::Panel::bottom("console")
                .resizable(true)
                .default_size(160.0)
                .size_range(60.0..=500.0)
                .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin::symmetric(10, 6)))
                .show(ui, |ui| self.console(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel)).show(ui, |ui| {
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
        self.rule_designer_window(&ctx);
        self.optimizer_window(&ctx);
        if let Some(r) = self.copilot.window(&ctx, &self.settings.llm) {
            self.copilot_reply(&ctx, r);
        }
        if std::mem::take(&mut self.copilot.open_settings) {
            self.ai_models.open = true;
        }
        let dirs: Vec<PathBuf> = self.project.as_ref().and_then(|p| p.root.clone()).into_iter().collect();
        self.ai_models.window(&ctx, &mut self.settings.llm, &dirs);
        self.stream_map_window(&ctx);
        self.fill_window(&ctx);
        let active = self.active.clone();
        let sim = active.as_ref().and_then(|r| self.sims.get_mut(r));
        self.calc.window(&ctx, active.as_ref(), sim);
        self.sweep_window(&ctx);
        self.results_window(&ctx);
        self.net_window(&ctx);
        self.hier_window(&ctx);
        self.gen_window(&ctx);

        let cmds = std::mem::take(&mut self.queue);
        for c in cmds {
            self.run_command(&ctx, c);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
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
