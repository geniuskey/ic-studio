//! Design copilot: an agent on the model chosen in AI Models settings that works on the open project through the same tools
//! as the MCP server. Its tool calls go through the app's live-mode queue, so every edit, DRC
//! marker and waveform appears in the window as it happens, and edits stay undoable per cell.
//!
//! Besides the design tools the agent has its own: `update_todos` keeps a task list the window
//! shows, `ask_user` pauses the run until the user answers, and `exit_plan_mode` presents a plan
//! for approval. In plan mode only read-only tools run, so the agent investigates and plans first.
//!
//! Tutor mode is for learners: edits are refused, and the copilot teaches by giving one step at a
//! time with `guide_step`. The app points at the place in the window ([`crate::spot`]), shows the
//! step in the chat and answers the call when the step's check passes on the user's design, or
//! when the user presses Done or I'm stuck or writes something. `ui_state` tells the tutor what
//! the user sees.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use ics_mcp::ToolResult;
use ics_mcp::bridge::Pending;
use serde_json::{Map, Value, json};

/// How long a tutor's step waits for the user.
const STEP_TIMEOUT: Duration = Duration::from_secs(4 * 3600);
/// Tools the app answers itself (not the MCP tools): what the user sees, and a tutor's step.
pub const APP_TOOLS: [&str; 2] = ["ui_state", "guide_step"];

/// Model turns per request before the copilot pauses.
const MAX_TURNS: usize = 40;
const TOOL_TIMEOUT: Duration = Duration::from_secs(600);
/// Tools the copilot does not get: it works on the project already open in the window.
const HIDDEN: [&str; 3] = ["new_project", "open_project", "gui_status"];
/// Design tools that only read the project: the ones plan mode runs.
const READ_ONLY: [&str; 16] = [
    "project_info",
    "docs_search",
    "docs_read",
    "pcell_design_check",
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
];

use crate::i18n::{t, tf};
pub use crate::llm::Llm;
use crate::llm::LlmSettings;

/// Makes the model client for a question (a scripted one in tests).
pub type LlmFactory = Box<dyn Fn(&LlmSettings, &[std::path::PathBuf]) -> Result<Box<dyn Llm>, String>>;

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User(String),
    Assistant(String),
    Tool {
        name: String,
        args: String,
        result: String,
        error: bool,
    },
    /// The agent asks the user; the run waits for the answer.
    Question {
        text: String,
        options: Vec<String>,
    },
    /// A plan presented for approval.
    Plan(String),
    /// A tutor's step for the user to do.
    Step(String),
    Notice(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
}

/// What a paused run waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    Answer,
    Approval,
}

/// A run paused on `ask_user` / `exit_plan_mode`: the call to answer, and the responses to the
/// other calls of the same model turn, which go back together with the answer.
struct Wait {
    kind: WaitKind,
    call: String,
    index: usize,
    replies: Vec<Value>,
}

/// What the user sends: text (a request, an answer or plan feedback) or a plan approval.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Text(String),
    Approve,
}

enum Event {
    Entry(Entry),
    /// A piece of the answer text being streamed.
    Delta(String),
    /// The streamed model turn is complete; its entries follow.
    TurnEnd,
    Todos(Vec<Todo>),
    Done(Vec<Value>, Option<Wait>),
}

pub struct Copilot {
    pub open: bool,
    /// Shown inside the main window instead of its own OS window.
    pub log: Vec<Entry>,
    pub todos: Vec<Todo>,
    pub input: String,
    /// The answer text streaming in for the current model turn.
    pub streaming: String,
    /// Set when the user asks for the AI Models settings window.
    pub open_settings: bool,
    /// Investigate and plan with read-only tools; edits wait for an approved plan.
    pub plan_mode: bool,
    /// Gemini `contents` of the conversation so far.
    history: Vec<Value>,
    wait: Option<Wait>,
    events: Option<Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    /// Tool calls from the worker, for the app to run.
    pub calls: Option<Receiver<Pending>>,
    factory: LlmFactory,
    /// Installed extensions, whose tools the agent may call too.
    pub ext: ics_mcp::ext::Registry,
    /// Name of the active cell, for the starter suggestions.
    pub active_cell: Option<String>,
    /// Teach instead of doing: edits are refused and the copilot guides the user step by step.
    pub tutor: bool,
    /// The tutor's step the user is working on; the app answers it.
    pub guide: Option<Guide>,
}

/// What finishes a tutor's step, besides the user saying so.
#[derive(Debug, Clone, PartialEq)]
pub enum GuideCheck {
    /// A check on a cell, as the Learn course writes them (`my_inv simulated`).
    Design(crate::learn::Check),
    /// The user is in this workspace.
    Workspace(crate::app::Workspace),
    /// The cell is the active one (in that workspace).
    Open(String, Option<crate::app::Workspace>),
    /// The project has a cell of this name.
    Exists(String),
    /// Something is selected in the active schematic or layout.
    Selected,
}

/// How the user answered a step.
#[derive(Debug, Clone, PartialEq)]
pub enum GuideAnswer {
    Done,
    Stuck,
    Said(String),
}

/// A tutor's step waiting for the user.
pub struct Guide {
    pub instruction: String,
    pub why: String,
    /// The anchor pointed at ([`crate::spot::ANCHORS`]).
    pub target: Option<String>,
    pub check: Option<GuideCheck>,
    /// What the check says now.
    pub status: String,
    pub passed: bool,
    /// Set by the window; the app replies with it.
    pub answer: Option<GuideAnswer>,
    pub reply: Sender<ToolResult>,
}

/// Parse `guide_step` arguments: the step, or what is wrong with them.
pub fn parse_guide(args: &Map<String, Value>) -> Result<(String, String, Option<String>, Option<GuideCheck>), String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let instruction = s("instruction");
    if instruction.is_empty() {
        return Err("instruction is required: one action for the user, in plain words".into());
    }
    let target = Some(s("target")).filter(|t| !t.is_empty());
    if let Some(t) = &target
        && crate::spot::anchor(t).is_none()
    {
        return Err(format!("unknown target '{t}'; use one of the listed anchors or leave it out"));
    }
    let check = Some(s("check")).filter(|c| !c.is_empty()).map(|c| parse_guide_check(&c)).transpose()?;
    Ok((instruction, s("why"), target, check))
}

fn parse_guide_check(c: &str) -> Result<GuideCheck, String> {
    use crate::app::Workspace;
    let w: Vec<&str> = c.split_whitespace().collect();
    let ws = |n: Option<&&str>| -> Result<Option<Workspace>, String> {
        Ok(match n.map(|s| s.to_ascii_lowercase()) {
            None => None,
            Some(n) => Some(
                Workspace::ALL
                    .into_iter()
                    .find(|w| w.name().eq_ignore_ascii_case(&n))
                    .ok_or_else(|| format!("unknown workspace '{n}' (home, schematic, simulation, layout)"))?,
            ),
        })
    };
    Ok(match (w.first().copied(), w.get(1).copied()) {
        (Some("workspace"), Some(_)) => GuideCheck::Workspace(ws(w.get(1))?.unwrap_or(Workspace::Home)),
        (Some("open"), Some(cell)) => GuideCheck::Open(cell.to_string(), ws(w.get(2))?),
        (Some("selected"), None) => GuideCheck::Selected,
        (Some(cell), Some("exists")) => GuideCheck::Exists(cell.to_string()),
        _ => GuideCheck::Design(crate::learn::parse_check("", c)?),
    })
}

/// Starter requests shown while the conversation is empty: (button text, request sent), in
/// the UI language (the copilot answers in the language of the request).
pub fn suggestions(cell: Option<&str>) -> Vec<(String, String)> {
    use crate::i18n::{t, tf};
    match cell {
        Some(c) => vec![
            (tf("Explain how {} works", &[c]), tf("Explain in simple terms how {} works and what each device does.", &[c])),
            (
                tf("Derive a new design from {}", &[c]),
                tf(
                    "Start a new design from {}: copy it, ask me for the new targets, size the copy to meet them and \
                     build its layout like the original.",
                    &[c],
                ),
            ),
            (
                tf("Why does {} miss its specs?", &[c]),
                tf("Simulate {}, check its specs and explain which ones fail, why, and what I should change.", &[c]),
            ),
            (
                t("Check the operating points").to_string(),
                tf("Simulate {} and tell me whether every transistor is in the region it should be in, and why.", &[c]),
            ),
        ],
        None => vec![
            (
                t("Which example should I start from?").to_string(),
                t("I am new to IC design. Which example cell should I start from for an amplifier, a logic gate or a \
                   reference, and why?")
                .to_string(),
            ),
            (
                t("How do I start a new design?").to_string(),
                t("How do I start a new design from one of the examples in IC Studio?").to_string(),
            ),
        ],
    }
}

/// Starter requests in tutor mode.
pub fn tutor_suggestions(cell: Option<&str>) -> Vec<(String, String)> {
    use crate::i18n::{t, tf};
    let mut v = vec![
        (
            t("Teach me to design an inverter").to_string(),
            t("I am new to chip design. Teach me to design a CMOS inverter step by step: I want to place the transistors, \
               wire them, simulate and measure it myself.")
            .to_string(),
        ),
        (
            t("Help me with my Learn chapter").to_string(),
            t("Look at the Learn chapter I have open and help me through its next step.").to_string(),
        ),
    ];
    if let Some(c) = cell {
        v.push((
            tf("Walk me through {}", &[c]),
            tf("Walk me through {} part by part: show me each device on the canvas and explain what it does.", &[c]),
        ));
    }
    v
}

impl Default for Copilot {
    fn default() -> Self {
        Self {
            open: false,
            log: vec![],
            todos: vec![],
            input: String::new(),
            streaming: String::new(),
            open_settings: false,
            plan_mode: false,
            active_cell: None,
            tutor: false,
            guide: None,
            history: vec![],
            wait: None,
            events: None,
            cancel: Arc::default(),
            calls: None,
            factory: Box::new(crate::llm::client),
            ext: Default::default(),
        }
    }
}

/// The copilot's own tools, run on the worker rather than on the project.
fn agent_declarations() -> Vec<Value> {
    vec![
        json!({
            "name": "update_todos",
            "description": "Replace the task list shown to the user. Use it for any request with three or more steps: \
                write the list before starting, keep exactly one task in_progress, and mark each completed as soon as it \
                is done and verified. Send the whole list every time.",
            "parametersJsonSchema": {"type": "object", "properties": {"todos": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "The task, as a short imperative sentence."},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                },
                "required": ["content", "status"]
            }}}, "required": ["todos"]},
        }),
        json!({
            "name": "ask_user",
            "description": "Ask the user one question and wait for the answer. Use it when the request is ambiguous or \
                lacks something the tools cannot tell you (which cell, target specs, constraints, trade-offs), instead of \
                guessing. Inspect the project first so the question is concrete, and offer 2-4 likely answers as options.",
            "parametersJsonSchema": {"type": "object", "properties": {
                "question": {"type": "string"},
                "options": {"type": "array", "items": {"type": "string"}, "description": "Suggested answers; the user may also type another."}
            }, "required": ["question"]},
        }),
        json!({
            "name": "ui_state",
            "description": "What the user sees right now: the workspace, the active cell and what is selected, the last \
                simulation of the active cell (analyses, errors, transistor regions), DRC / LVS results, the Learn chapter \
                open with the state of its checks and the quizzes answered wrong, and the tutorial step. Call it first \
                when teaching, and whenever you are unsure what the user did.",
            "parametersJsonSchema": {"type": "object", "properties": {}},
        }),
        json!({
            "name": "guide_step",
            "description": format!("Give the user ONE step to do themselves and wait until it is done. The window points \
                at `target` with your instruction and shows the step in the chat with Done and I'm stuck buttons. The \
                result says how the step ended: the check passed (\"passed\"), the user pressed Done or I'm stuck, or \
                what they wrote; it includes ui_state. Checks: '<cell> exists', 'open <cell> [schematic|simulation|layout]', \
                'workspace <name>', 'selected', '<cell> schematic <devices>', '<cell> layout', '<cell> simulated', \
                '<cell> region <device> <saturation|triode|cutoff>', '<cell> measure <op|dc|ac|tran> \"<calculator \
                expression>\" <min> <max>' (SPICE numbers, \"\" for no limit), '<cell> drc', '<cell> lvs'. Targets: {}.",
                crate::spot::ANCHORS.iter().map(|a| format!("{} ({})", a.key, a.what)).collect::<Vec<_>>().join("; ")),
            "parametersJsonSchema": {"type": "object", "properties": {
                "instruction": {"type": "string", "description": "One action, in plain words and the user's language, e.g. 'Click NMOS in the Device Library, then click on the canvas to place it.'"},
                "why": {"type": "string", "description": "One sentence on why this step matters."},
                "target": {"type": "string", "enum": crate::spot::ANCHORS.iter().map(|a| a.key).collect::<Vec<_>>()},
                "check": {"type": "string", "description": "When the step is done, in the check syntax above. Leave it out when no check fits; the user then presses Done."}
            }, "required": ["instruction"]},
        }),
        json!({
            "name": "exit_plan_mode",
            "description": "In plan mode: present the finished plan for the user's approval. The plan is a numbered list \
                of concrete steps naming the cells, tools and target values, and how each result is verified. After \
                approval you leave plan mode and carry it out.",
            "parametersJsonSchema": {"type": "object", "properties": {"plan": {"type": "string"}}, "required": ["plan"]},
        }),
    ]
}

/// The MCP tools and the copilot's own tools as Gemini function declarations.
pub fn declarations() -> Vec<Value> {
    declarations_with(vec![])
}

/// [`declarations`] with extension tools (MCP definitions) added.
pub fn declarations_with(ext: Vec<Value>) -> Vec<Value> {
    ics_mcp::tools::definitions()
        .into_iter()
        .chain(ext)
        .filter(|t| !HIDDEN.contains(&t["name"].as_str().unwrap_or_default()))
        .map(|t| json!({"name": t["name"], "description": t["description"], "parametersJsonSchema": t["inputSchema"]}))
        .chain(agent_declarations())
        .collect()
}

/// Whether a design tool call leaves the project unchanged.
fn read_only(name: &str, args: &Map<String, Value>) -> bool {
    READ_ONLY.contains(&name)
        || APP_TOOLS.contains(&name)
        || (name == "optimize" && args.get("apply") != Some(&Value::Bool(true)))
}

const TUTOR: &str = "TUTOR MODE is on. The user is learning IC design, often with no background in semiconductors, \
and wants to do the work with their own hands. You are a patient tutor, not an agent: edits are refused, the user makes \
them.\n\
How to teach:\n\
1. Call ui_state first to see where the user is (workspace, open cell, Learn chapter and its checks, last simulation).\n\
2. Split the task into small steps and give ONE at a time with guide_step: a single concrete action in plain words, the \
target to point at, and a check that passes when it is done. guide_step waits for the user and tells you how the step \
ended. Never give several actions in one step.\n\
3. After each step say in one or two sentences what just happened and why it matters, then give the next step.\n\
4. Before a simulation, ask the user to predict the result (ask_user with 2-4 options), then compare with what they get.\n\
5. When the user is stuck, hint in three levels: first the idea, then where in the window, then the exact action. For a \
quiz, lead them to the answer with a question instead of giving it.\n\
6. Explain every term the first time you use it (saturation, gm, threshold, netlist ...), with short sentences and \
everyday comparisons. Use the user's own numbers from the tools and the course; never invent results.\n\
7. Let the user press the buttons (simulate, DRC, LVS) through guide_step; use the read-only tools only to diagnose.\n\
8. The course in the app (Help > Learn IC Design) covers the theory: docs_search / docs_read with pages under learn/ \
(or ko/learn/) give its text and numbers. Point the user to the right chapter.\n\
9. End with a short recap of what the user learned and one thing to try next.";

fn system_prompt(context: &str, plan_mode: bool, tutor: bool) -> String {
    if tutor {
        return format!(
            "You are the tutor of IC Studio, a full-custom IC design environment (schematic, SPICE simulation, layout, \
             DRC/LVS), working next to the user's window: through the tools you see their project and results, and \
             through guide_step you point at the window and wait for them.\n\n{TUTOR}\n\n\
             Questions about using IC Studio (menus, buttons, shortcuts): answer from the user manual with docs_search \
             and docs_read, with its exact menu paths; never invent a menu or button.\n\n\
             Keep formatting light: plain sentences, short lists, **bold** for key numbers, `inline code` for cell, net \
             and device names. No tables or headings.\n\n\
             Language: write everything (steps, questions, explanations) in the language of the user's latest message; \
             if it is Korean, write Korean throughout.\n\n{context}"
        );
    }
    let mode = if plan_mode {
        "PLAN MODE is on. Only read-only tools run (project_info, get_netlist, layout_summary, flylines, simulate, \
         run_drc, run_lvs, extract, optimize without apply, ...); edits are refused. Investigate the design, ask the \
         user about anything unclear with ask_user, then call exit_plan_mode with the plan. Do not start the work."
    } else {
        "Plan mode is off: you may edit the design."
    };
    format!(
        "You are the design copilot of IC Studio, a full-custom IC design environment (schematic, SPICE simulation, \
         layout, DRC/LVS). You are an agent: you act on the project open in the user's window through the tools, and \
         the user sees every change live. Layout coordinates are in µm with y up.\n\n\
         How to work:\n\
         1. Understand the request. Inspect before acting (project_info, get_netlist, layout_summary). If the intent is \
         vague or a needed decision is missing (which cell, target values, what to trade off), call ask_user with a \
         concrete question and options rather than guessing. Do not ask what the tools can tell you, and ask one \
         question at a time.\n\
         2. Plan. For any task with three or more steps, call update_todos with the steps before doing them, then work \
         through them in order: one task in_progress at a time, marked completed right after it is verified. Add tasks \
         you discover along the way.\n\
         3. Explain as you go. Before each group of tool calls, write one short sentence saying what you are about to \
         do and why.\n\
         4. Verify every change (run_drc, fix_drc, flylines, run_lvs, simulate). Never invent results: report what the \
         tools return. If a tool fails, read the error and try a different approach; do not repeat the same failing \
         call more than twice. If you are stuck, say so and ask the user.\n\
         5. Finish with a short report: what changed, the verified numbers, and anything left open.\n\n\
         Questions about using IC Studio (where a menu item or button is, shortcuts, how to do a task in the \
         window, settings, file formats, the PDK / technology files, the CLI, MCP or Python API): answer from the user \
         manual. Call docs_search (in the user's language; try other words or lang en when little comes back), then \
         docs_read the best one or two sections, and answer with the manual's exact menu paths (**File > Import > \
         ...**), buttons and shortcuts, naming the page and section you used. Never invent a menu, button or \
         shortcut the manual does not describe: say it is not documented. A pure question needs no task list and no \
         edits; offer to do the task with the tools when that helps.\n\n\
         Starting a new design from an existing one (the way most users, many of them new to IC design, work): \
         derive_design does the whole job in one call once the targets are known (dry_run first shows which \
         parameters it would change); use the steps below when the user wants control over them. \
         1. get_netlist the reference cell and read its notes and specs. \
         2. Ask (ask_user) for the new targets if the request does not give them, as numbers with units. \
         3. copy_cell to a new name with a note saying what it is derived from and for which targets; never edit the \
         reference itself. \
         4. Simulate the copy and compare with the targets. Choose a few parameters that move the failing specs (the \
         notes and the spec advice say which way), keep matched devices equal by giving them the same design variable, \
         and call optimize with the new specs and those vars; apply the best point. \
         5. If the reference has a layout, place_like it into the copy (route true), then fix_drc and run_lvs. \
         6. Report the new numbers next to the reference's, in plain words. \
         Explain the circuit in beginner terms when you mention a device's role (input pair, current mirror, tail \
         source, load) and when a spec fails say why and what the change does; do not assume the user knows the \
         jargon.\n\n\
         Parametric cells (PCell Designer): pcell_design_create makes one from a drawn layout (source, plus suggest \
         [contacts, fingers, width, length] for a MOS) or from nothing (add_shapes, set_params, add_ops); inspect the \
         drawing with pcell_design_check list_source before placing stretch lines and regions, refine with \
         pcell_design_edit, verify with pcell_design_check (lint and the DRC corner check must be clean), place it with \
         place_pcell master and export it with pcell_design_export_skill.\n\n\
         Do not call save_project unless the user asks to save; edits stay in the window, where the user can undo \
         them. Keep formatting light: plain sentences, '-' or numbered lists, **bold** for key numbers, `inline \
         code` for cell, net and device names, and fenced code blocks (```spice ... ```) for netlists, SPICE decks or \
         anything else the user may want to copy. No tables or headings.\n\n\
         Language: write every sentence (the notes before tool calls, questions, plans, the task list and the report) \
         in the language of the user's latest message; if it is Korean, write Korean throughout.\n\n\
         {mode}\n\n{context}"
    )
}

fn parse_todos(args: &Map<String, Value>) -> Result<Vec<Todo>, String> {
    let list = args.get("todos").and_then(Value::as_array).ok_or("todos must be a list")?;
    list.iter()
        .map(|t| {
            let content = t["content"].as_str().map(str::trim).filter(|c| !c.is_empty()).ok_or("every todo needs content")?;
            let status = match t["status"].as_str().unwrap_or("pending") {
                "pending" => TodoStatus::Pending,
                "in_progress" => TodoStatus::InProgress,
                "completed" => TodoStatus::Completed,
                s => return Err(format!("unknown status '{s}'")),
            };
            Ok(Todo { content: content.to_string(), status })
        })
        .collect()
}

/// Append `parts` as a user turn, merged into the last one if that is a user turn already.
fn push_user(history: &mut Vec<Value>, mut parts: Vec<Value>) {
    if let Some(last) = history.last_mut().filter(|c| c["role"] == "user")
        && let Some(p) = last["parts"].as_array_mut()
    {
        p.append(&mut parts);
        return;
    }
    history.push(json!({"role": "user", "parts": parts}));
}

struct Run {
    llm: Box<dyn Llm>,
    history: Vec<Value>,
    system: String,
    plan_mode: bool,
    tutor: bool,
    tools: Sender<Pending>,
    events: Sender<Event>,
    wake: egui::Context,
    cancel: Arc<AtomicBool>,
    ext: ics_mcp::ext::Registry,
}

impl Run {
    fn send(&self, e: Event) {
        let _ = self.events.send(e);
        self.wake.request_repaint();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Run a design tool in the app and wait for its result.
    fn design_tool(&self, name: &str, args: Map<String, Value>) -> Value {
        let (tx, rx) = mpsc::channel();
        let res = if self.tools.send(Pending { tool: name.to_string(), arguments: args.clone(), reply: tx }).is_err() {
            ToolResult::error("the app is gone")
        } else {
            self.wake.request_repaint();
            let start = Instant::now();
            let limit = if name == "guide_step" { STEP_TIMEOUT } else { TOOL_TIMEOUT };
            loop {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(r) => break r,
                    Err(RecvTimeoutError::Disconnected) => break ToolResult::error("the app dropped the tool call"),
                    Err(RecvTimeoutError::Timeout) if self.cancelled() => {
                        break ToolResult::error("cancelled by the user; the call may still finish in the app");
                    }
                    Err(RecvTimeoutError::Timeout) if start.elapsed() > limit => {
                        break ToolResult::error("the tool call timed out");
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        };
        self.send(Event::Entry(Entry::Tool {
            name: name.to_string(),
            args: Value::Object(args).to_string(),
            result: res.text.clone(),
            error: res.is_error,
        }));
        let key = if res.is_error { "error" } else { "result" };
        let body: Value = serde_json::from_str(&res.text).unwrap_or(Value::String(res.text));
        json!({key: body})
    }

    /// The conversation loop on a worker thread: ask the model, run the tools it calls, feed the
    /// results back, until it answers in text, asks the user, presents a plan or is stopped.
    fn converse(mut self) {
        let decl = declarations_with(self.ext.definitions());
        let mut wait: Option<Wait> = None;
        for turn in 0..=MAX_TURNS {
            if self.cancelled() {
                break;
            }
            if turn == MAX_TURNS {
                self.send(Event::Entry(Entry::Notice(format!(
                    "Paused after {MAX_TURNS} model turns. Say \"continue\" to go on."
                ))));
                break;
            }
            let req = json!({
                "systemInstruction": {"parts": [{"text": self.system}]},
                "contents": self.history,
                "tools": [{"functionDeclarations": decl}],
            });
            let (events, wake, cancel) = (self.events.clone(), self.wake.clone(), self.cancel.clone());
            let res = self.llm.stream(&req, &mut |t| {
                let _ = events.send(Event::Delta(t.to_string()));
                wake.request_repaint();
                !cancel.load(Ordering::Relaxed)
            });
            self.send(Event::TurnEnd);
            let res = match res {
                Ok(r) => r,
                Err(_) if self.cancelled() => break,
                Err(e) => {
                    self.send(Event::Entry(Entry::Error(e)));
                    break;
                }
            };
            let content = res["candidates"][0]["content"].clone();
            let parts = content["parts"].as_array().cloned().unwrap_or_default();
            if parts.is_empty() {
                let why = res["candidates"][0]["finishReason"].as_str().unwrap_or("no answer").to_string();
                self.send(Event::Entry(Entry::Error(format!("The model stopped: {why}"))));
                break;
            }
            self.history.push(json!({"role": "model", "parts": parts}));
            let mut replies = vec![];
            let mut calls = 0;
            for p in &parts {
                if p["thought"] != true
                    && let Some(t) = p["text"].as_str().filter(|t| !t.trim().is_empty())
                {
                    self.send(Event::Entry(Entry::Assistant(t.trim().to_string())));
                }
                let Some(call) = p.get("functionCall") else { continue };
                calls += 1;
                let name = call["name"].as_str().unwrap_or_default().to_string();
                let args: Map<String, Value> = call["args"].as_object().cloned().unwrap_or_default();
                let out = if wait.is_some() {
                    json!({"error": "not run: waiting for the user's reply"})
                } else if self.cancelled() {
                    json!({"error": "not run: cancelled by the user"})
                } else {
                    match name.as_str() {
                        "update_todos" => match parse_todos(&args) {
                            Ok(t) => {
                                let done = t.iter().filter(|t| t.status == TodoStatus::Completed).count();
                                let n = t.len();
                                self.send(Event::Todos(t));
                                json!({"result": format!("task list updated: {done}/{n} completed")})
                            }
                            Err(e) => json!({"error": e}),
                        },
                        "ask_user" => {
                            let text = args.get("question").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                            let options = args
                                .get("options")
                                .and_then(Value::as_array)
                                .map(|o| o.iter().filter_map(Value::as_str).map(str::to_string).collect())
                                .unwrap_or_default();
                            self.send(Event::Entry(Entry::Question { text, options }));
                            wait = Some(Wait { kind: WaitKind::Answer, call: name, index: replies.len(), replies: vec![] });
                            continue;
                        }
                        "exit_plan_mode" if self.plan_mode => {
                            let plan = args.get("plan").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                            self.send(Event::Entry(Entry::Plan(plan)));
                            wait = Some(Wait { kind: WaitKind::Approval, call: name, index: replies.len(), replies: vec![] });
                            continue;
                        }
                        "exit_plan_mode" => json!({"result": "plan mode is off; go ahead with the work"}),
                        _ if self.tutor && !read_only(&name, &args) && !self.ext.read_only(&name) => json!({"error": format!(
                            "tutor mode is on, so '{name}' did not run: the user makes the edits. Give them the step \
                             with guide_step instead."
                        )}),
                        _ if self.plan_mode && !read_only(&name, &args) && !self.ext.read_only(&name) => json!({"error": format!(
                            "plan mode is on, so '{name}' did not run: only read-only tools work. Put this step in the \
                             plan and present it with exit_plan_mode."
                        )}),
                        _ => self.design_tool(&name, args),
                    }
                };
                replies.push(json!({"functionResponse": {"name": name, "response": out}}));
            }
            if let Some(w) = &mut wait {
                w.replies = replies;
                break;
            }
            if calls == 0 {
                break;
            }
            push_user(&mut self.history, replies);
        }
        if self.cancelled() {
            self.send(Event::Entry(Entry::Notice("Stopped.".into())));
        }
        let history = std::mem::take(&mut self.history);
        self.send(Event::Done(history, wait));
    }
}

impl Copilot {
    /// Replace the model client (tests use a scripted one).
    pub fn set_llm(&mut self, factory: LlmFactory) {
        self.factory = factory;
    }

    pub fn busy(&self) -> bool {
        self.events.is_some()
    }

    /// What the paused agent waits for, if anything.
    pub fn waiting(&self) -> Option<WaitKind> {
        self.wait.as_ref().map(|w| w.kind)
    }

    /// Stop the running request after the current model turn or tool call.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Send a request, or, while the agent waits, the answer to its question or the verdict on its
    /// plan (approval, or text as feedback).
    pub fn reply(&mut self, ctx: &egui::Context, reply: Reply, context: &str, llm: &LlmSettings, dirs: &[std::path::PathBuf]) {
        if self.busy() {
            return;
        }
        let text = match &reply {
            Reply::Text(t) => Some(t.trim().to_string()).filter(|t| !t.is_empty()),
            Reply::Approve => None,
        };
        let valid = match (self.waiting(), &reply) {
            (Some(WaitKind::Approval), Reply::Approve) => true,
            _ => text.is_some(),
        };
        if !valid {
            return;
        }
        let llm = match (self.factory)(llm, dirs) {
            Ok(c) => c,
            Err(e) => {
                if let Some(t) = &text {
                    self.log.push(Entry::User(t.clone()));
                }
                self.log.push(Entry::Error(e));
                return;
            }
        };
        let parts = match self.wait.take() {
            None => {
                let t = text.unwrap_or_default();
                self.log.push(Entry::User(t.clone()));
                vec![json!({"text": t})]
            }
            Some(w) => {
                let response = match (w.kind, text) {
                    (WaitKind::Approval, None) => {
                        self.plan_mode = false;
                        self.log.push(Entry::User("Plan approved.".into()));
                        json!({"approved": true, "next": "The user approved the plan and plan mode is off. Carry it out \
                            now: put its steps in update_todos and work through them one at a time, verifying each."})
                    }
                    (WaitKind::Approval, Some(t)) => {
                        self.log.push(Entry::User(t.clone()));
                        json!({"approved": false, "feedback": t, "next": "Revise the plan with this feedback (investigate \
                            again if needed) and present it with exit_plan_mode."})
                    }
                    (WaitKind::Answer, t) => {
                        let t = t.unwrap_or_default();
                        self.log.push(Entry::User(t.clone()));
                        json!({"answer": t})
                    }
                };
                let mut parts = w.replies;
                let at = w.index.min(parts.len());
                parts.insert(at, json!({"functionResponse": {"name": w.call, "response": response}}));
                parts
            }
        };
        let mut history = std::mem::take(&mut self.history);
        push_user(&mut history, parts);
        let (tool_tx, tool_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        self.cancel = Arc::default();
        let run = Run {
            llm,
            history,
            system: system_prompt(context, self.plan_mode && !self.tutor, self.tutor),
            plan_mode: self.plan_mode && !self.tutor,
            tutor: self.tutor,
            tools: tool_tx,
            events: ev_tx,
            wake: ctx.clone(),
            cancel: self.cancel.clone(),
            ext: self.ext.clone(),
        };
        std::thread::spawn(move || run.converse());
        self.calls = Some(tool_rx);
        self.events = Some(ev_rx);
    }

    /// Pick up the worker's progress.
    pub fn poll(&mut self) {
        let Some(rx) = &self.events else { return };
        let mut done = None;
        for e in rx.try_iter() {
            match e {
                Event::Entry(x) => self.log.push(x),
                Event::Delta(t) => self.streaming.push_str(&t),
                Event::TurnEnd => self.streaming.clear(),
                Event::Todos(t) => self.todos = t,
                Event::Done(h, w) => done = Some((h, w)),
            }
        }
        if let Some((h, w)) = done {
            self.history = h;
            self.wait = w;
            self.events = None;
            self.calls = None;
            self.streaming.clear();
        }
    }

    pub fn clear(&mut self) {
        if !self.busy() {
            self.log.clear();
            self.todos.clear();
            self.history.clear();
            self.wait = None;
        }
    }

    fn todo_panel(&self, ui: &mut egui::Ui, pal: &crate::theme::Palette) {
        let done = self.todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
        egui::CollapsingHeader::new(tf("Tasks {}/{}", &[&done.to_string(), &self.todos.len().to_string()]))
            .id_salt("copilot_todos")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("copilot_todo_list").max_height(140.0).show(ui, |ui| {
                    for t in &self.todos {
                        let text = match t.status {
                            TodoStatus::Completed => egui::RichText::new(format!("[x] {}", t.content)).strikethrough().weak(),
                            TodoStatus::InProgress => {
                                egui::RichText::new(format!("[>] {}", t.content)).strong().color(pal.accent)
                            }
                            TodoStatus::Pending => egui::RichText::new(format!("[ ] {}", t.content)),
                        };
                        ui.label(text);
                    }
                });
            });
    }

    /// The conversation as plain text, for "Copy all".
    pub fn transcript(&self) -> String {
        let mut out = vec![];
        for e in &self.log {
            out.push(match e {
                Entry::User(t) => format!("You: {t}"),
                Entry::Assistant(t) => format!("Copilot: {t}"),
                Entry::Tool { name, args, result, error } => {
                    format!("> {name} {args}\n{}{result}", if *error { "error: " } else { "" })
                }
                Entry::Question { text, options } => {
                    let o: String = options.iter().map(|o| format!("\n- {o}")).collect();
                    format!("Question: {text}{o}")
                }
                Entry::Plan(t) => format!("Plan:\n{t}"),
                Entry::Step(t) => format!("Step: {t}"),
                Entry::Notice(t) => t.clone(),
                Entry::Error(t) => format!("Error: {t}"),
            });
        }
        out.join("\n\n")
    }

    /// A message in a card, with a copy button on its header line.
    fn card(
        ui: &mut egui::Ui,
        id: egui::Id,
        title: &str,
        fill: egui::Color32,
        accent: egui::Color32,
        copy: &str,
        add: impl FnOnce(&mut egui::Ui),
    ) {
        egui::Frame::new().fill(fill).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).small().strong().color(accent));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    crate::chat::copy_button(ui, id.with("copy"), "Copy", copy);
                });
            });
            add(ui);
        });
    }

    fn entry(&self, ui: &mut egui::Ui, k: usize, e: &Entry, pal: &crate::theme::Palette) {
        let id = egui::Id::unique("copilot_entry").with(k);
        match e {
            Entry::User(t) => {
                Self::card(ui, id, "You", pal.accent.gamma_multiply(0.14), pal.accent, t, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(t).color(pal.text_strong)).wrap());
                });
            }
            Entry::Assistant(t) => {
                Self::card(ui, id, "Copilot", egui::Color32::TRANSPARENT, pal.text_weak, t, |ui| {
                    crate::chat::markdown(ui, id, t, pal);
                });
            }
            Entry::Tool { name, args, result, error } => {
                let color = if *error { pal.danger } else { pal.text_weak };
                let a: String = args.chars().take(80).collect();
                let a = if a.len() < args.len() { format!("{a}…") } else { a };
                let head = egui::RichText::new(format!("{name} {a}")).monospace().small().color(color);
                egui::CollapsingHeader::new(head).id_salt(id).show(ui, |ui| {
                    let pretty = |s: &str| {
                        serde_json::from_str::<Value>(s)
                            .ok()
                            .and_then(|v| serde_json::to_string_pretty(&v).ok())
                            .unwrap_or(s.into())
                    };
                    crate::chat::code_block(ui, id.with("args"), "arguments", &pretty(args), pal);
                    crate::chat::code_block(ui, id.with("result"), if *error { "error" } else { "result" }, &pretty(result), pal);
                });
            }
            Entry::Question { text, options } => {
                Self::card(ui, id, "Question", pal.accent.gamma_multiply(0.08), pal.accent, text, |ui| {
                    crate::chat::markdown(ui, id, text, pal);
                    for o in options {
                        ui.label(egui::RichText::new(format!("- {o}")).color(pal.text_weak));
                    }
                });
            }
            Entry::Plan(t) => {
                Self::card(ui, id, "Plan", pal.accent.gamma_multiply(0.08), pal.accent, t, |ui| {
                    crate::chat::markdown(ui, id, t, pal);
                });
            }
            Entry::Step(text) => {
                Self::card(ui, id, t("Tutor step"), pal.ok.gamma_multiply(0.10), pal.ok, text, |ui| {
                    crate::chat::markdown(ui, id, text, pal);
                });
            }
            Entry::Notice(t) => {
                ui.add(egui::Label::new(egui::RichText::new(t).italics().color(pal.warning)).wrap());
            }
            Entry::Error(t) => {
                Self::card(ui, id, "Error", pal.danger.gamma_multiply(0.12), pal.danger, t, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(t).color(pal.danger)).wrap());
                });
            }
        }
    }

    /// The message box and the controls above it. Returns what the user sends.
    fn composer(&mut self, ui: &mut egui::Ui, busy: bool, waiting: Option<WaitKind>) -> Option<Reply> {
        let mut send = None;
        match waiting.filter(|_| !busy) {
            Some(WaitKind::Answer) => {
                let options = match self.log.iter().rev().find(|e| matches!(e, Entry::Question { .. })) {
                    Some(Entry::Question { options, .. }) => options.clone(),
                    _ => vec![],
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(t("Answer:")).weak());
                    for o in options {
                        if ui.button(&o).clicked() {
                            send = Some(Reply::Text(o));
                        }
                    }
                });
            }
            Some(WaitKind::Approval) => {
                ui.horizontal(|ui| {
                    if ui.button(t("Approve plan")).clicked() {
                        send = Some(Reply::Approve);
                    }
                    ui.label(egui::RichText::new(t("or type changes below")).weak());
                });
            }
            None => {}
        }
        ui.horizontal(|ui| {
            let l = ui.label(match waiting {
                Some(WaitKind::Answer) => "Answer",
                Some(WaitKind::Approval) => "Revise",
                None => "Ask",
            });
            let w = (ui.available_width() - 64.0).max(80.0);
            let shift_enter = egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter);
            let r = egui::ScrollArea::vertical()
                .id_salt("copilot_input_scroll")
                .max_height(160.0)
                .max_width(w)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.input)
                            .id_salt("copilot_input")
                            .desired_rows(2)
                            .desired_width(w - 14.0)
                            .return_key(shift_enter)
                            .hint_text(t("Enter to send, Shift+Enter for a new line")),
                    )
                })
                .inner;
            let r = r.labelled_by(l.id);
            let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if (ui.add_enabled(!busy, egui::Button::new(t("Send"))).clicked() || enter) && !busy {
                let t = std::mem::take(&mut self.input).trim().to_string();
                if !t.is_empty() {
                    send = Some(Reply::Text(t));
                }
                r.request_focus();
            }
        });
        send
    }

    /// The tutor's current step, with the buttons that answer it.
    fn step_card(&mut self, ui: &mut egui::Ui, pal: &crate::theme::Palette) {
        let Some(g) = self.guide.as_mut() else { return };
        egui::Frame::new()
            .fill(pal.ok.gamma_multiply(0.10))
            .stroke(egui::Stroke::new(1.0, pal.ok.gamma_multiply(0.6)))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::spot::mark(ui.ctx(), "tutor.step", ui.max_rect());
                ui.label(egui::RichText::new(t("Your step")).small().strong().color(pal.ok));
                ui.add(egui::Label::new(egui::RichText::new(&g.instruction).color(pal.text_strong)).wrap());
                if !g.why.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(&g.why).small().color(pal.text_weak)).wrap());
                }
                if !g.status.is_empty() {
                    let col = if g.passed { pal.ok } else { pal.text_weak };
                    ui.label(egui::RichText::new(&g.status).small().color(col));
                }
                ui.horizontal_wrapped(|ui| {
                    if let Some(key) = &g.target
                        && ui.small_button(t("Show me where")).clicked()
                    {
                        crate::spot::point(ui.ctx(), key, &g.instruction, None);
                    }
                    if ui.button(t("Done")).on_hover_text(t("Tell the tutor you did this step")).clicked() {
                        g.answer = Some(GuideAnswer::Done);
                    }
                    if ui.button(t("I'm stuck")).on_hover_text(t("Ask for a hint")).clicked() {
                        g.answer = Some(GuideAnswer::Stuck);
                    }
                });
            });
        ui.add_space(4.0);
    }

    /// The chat window. Returns what the user sends.
    pub fn window(&mut self, ctx: &egui::Context, llm: &LlmSettings) -> Option<Reply> {
        let mut open = self.open;
        let mut send = None;
        crate::float::tool_window(ctx, "copilot", "Copilot", [480.0, 640.0], &mut open, |ui| {
            let busy = self.busy();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(llm.describe()).weak());
                if ui.small_button(t("AI Models…")).on_hover_text(t("Choose the provider, model and API key")).clicked() {
                    self.open_settings = true;
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let (agent, plan, tutor) = (!self.plan_mode && !self.tutor, self.plan_mode && !self.tutor, self.tutor);
                    if ui
                        .selectable_label(agent, t("Do it"))
                        .on_hover_text(t("The copilot does the task on the project, and you watch every change"))
                        .clicked()
                    {
                        (self.plan_mode, self.tutor) = (false, false);
                    }
                    if ui
                        .selectable_label(plan, t("Plan mode"))
                        .on_hover_text(t("Investigate with read-only tools and present a plan for approval before editing anything"))
                        .clicked()
                    {
                        (self.plan_mode, self.tutor) = (true, false);
                    }
                    if ui
                        .selectable_label(tutor, t("Tutor"))
                        .on_hover_text(t("Learn by doing: the copilot gives one step at a time, points at where to click and checks your work; it edits nothing itself"))
                        .clicked()
                    {
                        (self.plan_mode, self.tutor) = (false, true);
                    }
                });
                ui.separator();
                if ui.add_enabled(!busy, egui::Button::new(t("Clear"))).clicked() {
                    self.clear();
                }
                if !self.log.is_empty() {
                    crate::chat::copy_button_with(ui, egui::Id::unique("copilot_copy_all"), "Copy all", || self.transcript());
                }
                if busy && ui.button(t("Stop")).clicked() {
                    self.stop();
                }
            });
            ui.separator();
            let pal = crate::theme::palette(ui.ctx());
            if !self.todos.is_empty() {
                self.todo_panel(ui, &pal);
                ui.separator();
            }
            let waiting = self.waiting();
            egui::Panel::bottom(egui::Id::unique("copilot_composer"))
                .resizable(false)
                .frame(egui::Frame::new().inner_margin(egui::Margin { top: 6, ..Default::default() }))
                .show(ui, |ui| {
                    self.step_card(ui, &pal);
                    let guiding = self.guide.is_some();
                    let said = self.composer(ui, busy && !guiding, waiting);
                    match (said, self.guide.as_mut()) {
                        (Some(Reply::Text(text)), Some(g)) => {
                            self.log.push(Entry::User(text.clone()));
                            g.answer = Some(GuideAnswer::Said(text));
                        }
                        (said, _) => send = said,
                    }
                });
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                if let Some(t) = self.log_view(ui, busy, &pal) {
                    send = Some(Reply::Text(t));
                }
            });
        });
        self.open = open;
        send
    }

    /// The conversation; returns a starter request the user clicked.
    fn log_view(&self, ui: &mut egui::Ui, busy: bool, pal: &crate::theme::Palette) -> Option<String> {
        let mut picked = None;
        egui::ScrollArea::vertical().id_salt("copilot_log").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            // Drags on the gaps between messages select nothing, but must not move the window.
            let _ = ui.interact(ui.clip_rect(), ui.scope_id().with("log_bg"), egui::Sense::drag());
            ui.spacing_mut().item_spacing.y = 6.0;
            if self.log.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(crate::i18n::t(
                            "Ask for a design task, e.g. \"place and route nand2, then fix DRC and run LVS\" or \
                             \"size the OTA for 60 dB gain and check the phase margin\", or ask how to use IC Studio, \
                             e.g. \"how do I import a GDS file?\": answers come from the user manual. Turn on Plan mode \
                             to review a plan before anything is edited.",
                        ))
                        .weak(),
                    )
                    .wrap(),
                );
                ui.add_space(6.0);
                ui.label(egui::RichText::new(crate::i18n::t("Try:")).weak());
                let starters = if self.tutor {
                    tutor_suggestions(self.active_cell.as_deref())
                } else {
                    suggestions(self.active_cell.as_deref())
                };
                for (label, request) in starters {
                    if ui.add_enabled(!busy, egui::Button::new(label)).on_hover_text(&request).clicked() {
                        picked = Some(request);
                    }
                }
            }
            for (k, e) in self.log.iter().enumerate() {
                self.entry(ui, k, e, pal);
            }
            if !self.streaming.trim().is_empty() {
                let id = egui::Id::unique("copilot_streaming");
                Self::card(ui, id, "Copilot", egui::Color32::TRANSPARENT, pal.text_weak, &self.streaming, |ui| {
                    crate::chat::markdown(ui, id, &self.streaming, pal);
                });
            }
            if busy && self.guide.is_some() {
                ui.label(egui::RichText::new(t("Waiting for you to do the step below…")).weak());
            } else if busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    let now = self.todos.iter().find(|t| t.status == TodoStatus::InProgress);
                    ui.label(egui::RichText::new(now.map_or("Working…".to_string(), |t| format!("{}…", t.content))).weak());
                });
            }
            crate::chat::drag_autoscroll(ui);
        });
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_are_gemini_functions() {
        let d = declarations();
        assert!(d.iter().any(|t| t["name"] == "run_drc"));
        assert!(d.iter().any(|t| t["name"] == "update_todos"));
        assert!(d.iter().all(|t| t["name"] != "open_project"));
        assert!(d.iter().all(|t| t["parametersJsonSchema"]["type"] == "object"));
    }

    #[test]
    fn read_only_tools_exist() {
        let names: Vec<String> = declarations().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        for t in READ_ONLY {
            assert!(names.iter().any(|n| n == t), "{t}");
        }
        assert!(read_only("optimize", &Map::new()));
        let apply: Map<String, Value> = json!({"apply": true}).as_object().cloned().unwrap();
        assert!(!read_only("optimize", &apply));
        assert!(!read_only("route", &Map::new()));
    }

    #[test]
    fn parses_todos() {
        let a = json!({"todos": [{"content": "Place", "status": "completed"}, {"content": "Route", "status": "in_progress"}]});
        let t = parse_todos(a.as_object().unwrap()).unwrap();
        assert_eq!(t[1], Todo { content: "Route".into(), status: TodoStatus::InProgress });
        assert!(parse_todos(json!({"todos": [{"content": "x", "status": "done"}]}).as_object().unwrap()).is_err());
    }

    #[test]
    fn user_turns_merge() {
        let mut h = vec![json!({"role": "user", "parts": [{"text": "a"}]})];
        push_user(&mut h, vec![json!({"text": "b"})]);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0]["parts"].as_array().unwrap().len(), 2);
    }
}
