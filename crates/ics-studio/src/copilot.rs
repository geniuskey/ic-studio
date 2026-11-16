//! Design copilot: an agent on the model chosen in AI Models settings that works on the open project through the same tools
//! as the MCP server. Its tool calls go through the app's live-mode queue, so every edit, DRC
//! marker and waveform appears in the window as it happens, and edits stay undoable per cell.
//!
//! Besides the design tools the agent has its own: `update_todos` keeps a task list the window
//! shows, `ask_user` pauses the run until the user answers, and `exit_plan_mode` presents a plan
//! for approval. In plan mode only read-only tools run, so the agent investigates and plans first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use ics_mcp::ToolResult;
use ics_mcp::bridge::Pending;
use serde_json::{Map, Value, json};

/// Model turns per request before the copilot pauses.
const MAX_TURNS: usize = 40;
const TOOL_TIMEOUT: Duration = Duration::from_secs(600);
/// Tools the copilot does not get: it works on the project already open in the window.
const HIDDEN: [&str; 3] = ["new_project", "open_project", "gui_status"];
/// Design tools that only read the project: the ones plan mode runs.
const READ_ONLY: [&str; 13] = [
    "project_info",
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
            history: vec![],
            wait: None,
            events: None,
            cancel: Arc::default(),
            calls: None,
            factory: Box::new(crate::llm::client),
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
    ics_mcp::tools::definitions()
        .into_iter()
        .filter(|t| !HIDDEN.contains(&t["name"].as_str().unwrap_or_default()))
        .map(|t| json!({"name": t["name"], "description": t["description"], "parametersJsonSchema": t["inputSchema"]}))
        .chain(agent_declarations())
        .collect()
}

/// Whether a design tool call leaves the project unchanged.
fn read_only(name: &str, args: &Map<String, Value>) -> bool {
    READ_ONLY.contains(&name) || (name == "optimize" && args.get("apply") != Some(&Value::Bool(true)))
}

fn system_prompt(context: &str, plan_mode: bool) -> String {
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
    tools: Sender<Pending>,
    events: Sender<Event>,
    wake: egui::Context,
    cancel: Arc<AtomicBool>,
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
            loop {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(r) => break r,
                    Err(RecvTimeoutError::Disconnected) => break ToolResult::error("the app dropped the tool call"),
                    Err(RecvTimeoutError::Timeout) if self.cancelled() => {
                        break ToolResult::error("cancelled by the user; the call may still finish in the app");
                    }
                    Err(RecvTimeoutError::Timeout) if start.elapsed() > TOOL_TIMEOUT => {
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
        let decl = declarations();
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
                        _ if self.plan_mode && !read_only(&name, &args) => json!({"error": format!(
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
            system: system_prompt(context, self.plan_mode),
            plan_mode: self.plan_mode,
            tools: tool_tx,
            events: ev_tx,
            wake: ctx.clone(),
            cancel: self.cancel.clone(),
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
        egui::CollapsingHeader::new(format!("Tasks {done}/{}", self.todos.len()))
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
                    ui.label(egui::RichText::new("Answer:").weak());
                    for o in options {
                        if ui.button(&o).clicked() {
                            send = Some(Reply::Text(o));
                        }
                    }
                });
            }
            Some(WaitKind::Approval) => {
                ui.horizontal(|ui| {
                    if ui.button("Approve plan").clicked() {
                        send = Some(Reply::Approve);
                    }
                    ui.label(egui::RichText::new("or type changes below").weak());
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
                            .hint_text("Enter to send, Shift+Enter for a new line"),
                    )
                })
                .inner;
            let r = r.labelled_by(l.id);
            let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if (ui.add_enabled(!busy, egui::Button::new("Send")).clicked() || enter) && !busy {
                let t = std::mem::take(&mut self.input).trim().to_string();
                if !t.is_empty() {
                    send = Some(Reply::Text(t));
                }
                r.request_focus();
            }
        });
        send
    }

    /// The chat window. Returns what the user sends.
    pub fn window(&mut self, ctx: &egui::Context, llm: &LlmSettings) -> Option<Reply> {
        let mut open = self.open;
        let mut send = None;
        crate::float::tool_window(ctx, "copilot", "Copilot", [480.0, 640.0], &mut open, |ui| {
            let busy = self.busy();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(llm.describe()).weak());
                if ui.small_button("AI Models…").on_hover_text("Choose the provider, model and API key").clicked() {
                    self.open_settings = true;
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled(!busy, egui::Checkbox::new(&mut self.plan_mode, "Plan mode"))
                    .on_hover_text("Investigate with read-only tools and present a plan for approval before editing anything");
                if ui.add_enabled(!busy, egui::Button::new("Clear")).clicked() {
                    self.clear();
                }
                if !self.log.is_empty() {
                    crate::chat::copy_button_with(ui, egui::Id::unique("copilot_copy_all"), "Copy all", || self.transcript());
                }
                if busy && ui.button("Stop").clicked() {
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
                .show(ui, |ui| send = self.composer(ui, busy, waiting));
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| self.log_view(ui, busy, &pal));
        });
        self.open = open;
        send
    }

    fn log_view(&self, ui: &mut egui::Ui, busy: bool, pal: &crate::theme::Palette) {
        egui::ScrollArea::vertical().id_salt("copilot_log").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            // Drags on the gaps between messages select nothing, but must not move the window.
            let _ = ui.interact(ui.clip_rect(), ui.scope_id().with("log_bg"), egui::Sense::drag());
            ui.spacing_mut().item_spacing.y = 6.0;
            if self.log.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Ask for a design task, e.g. \"place and route nand2, then fix DRC and run LVS\" or \
                             \"size the OTA for 60 dB gain and check the phase margin\". Turn on Plan mode to review \
                             a plan before anything is edited.",
                        )
                        .weak(),
                    )
                    .wrap(),
                );
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
            if busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    let now = self.todos.iter().find(|t| t.status == TodoStatus::InProgress);
                    ui.label(egui::RichText::new(now.map_or("Working…".to_string(), |t| format!("{}…", t.content))).weak());
                });
            }
            crate::chat::drag_autoscroll(ui);
        });
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
