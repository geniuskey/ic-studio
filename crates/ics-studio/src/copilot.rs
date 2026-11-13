//! Design copilot: a chat with a Gemini model that works on the open project through the same tools
//! as the MCP server. Its tool calls go through the app's live-mode queue, so every edit, DRC
//! marker and waveform appears in the window as it happens, and edits stay undoable per cell.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;
use ics_mcp::ToolResult;
use ics_mcp::bridge::Pending;
use serde_json::{Map, Value, json};

/// Model turns per question before the copilot gives up.
const MAX_TURNS: usize = 16;
const TOOL_TIMEOUT: Duration = Duration::from_secs(600);
pub const DEFAULT_MODEL: &str = "gemini-flash-latest";
/// Tools the copilot does not get: it works on the project already open in the window.
const HIDDEN: [&str; 3] = ["new_project", "open_project", "gui_status"];

/// One `generateContent` round trip: Gemini request JSON in, response JSON out.
pub trait Llm: Send {
    fn generate(&mut self, request: &Value) -> Result<Value, String>;
}

pub struct Gemini {
    pub key: String,
    pub model: String,
}

impl Llm for Gemini {
    fn generate(&mut self, request: &Value) -> Result<Value, String> {
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model);
        let mut res = ureq::post(&url)
            .header("x-goog-api-key", &self.key)
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(request)
            .map_err(|e| format!("request failed: {e}"))?;
        let status = res.status();
        let body: Value = res.body_mut().read_json().map_err(|e| format!("bad response: {e}"))?;
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("Gemini {}: {msg}", status.as_u16()));
        }
        Ok(body)
    }
}

/// Makes the model client for a question (a scripted one in tests).
pub type LlmFactory = Box<dyn Fn(&str, &str) -> Box<dyn Llm>>;

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User(String),
    Assistant(String),
    Tool { name: String, args: String, result: String, error: bool },
    Error(String),
}

enum Event {
    Entry(Entry),
    Done(Vec<Value>),
}

pub struct Copilot {
    pub open: bool,
    pub log: Vec<Entry>,
    pub input: String,
    pub model: String,
    /// API key typed into the window (not stored); else `GEMINI_API_KEY` / `GOOGLE_API_KEY`.
    pub key: String,
    /// Gemini `contents` of the conversation so far.
    history: Vec<Value>,
    events: Option<Receiver<Event>>,
    /// Tool calls from the worker, for the app to run.
    pub calls: Option<Receiver<Pending>>,
    factory: LlmFactory,
}

impl Default for Copilot {
    fn default() -> Self {
        Self {
            open: false,
            log: vec![],
            input: String::new(),
            model: DEFAULT_MODEL.into(),
            key: String::new(),
            history: vec![],
            events: None,
            calls: None,
            factory: Box::new(|key, model| Box::new(Gemini { key: key.into(), model: model.into() })),
        }
    }
}

const KEY_VARS: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_API_KEY"];

/// `KEY=value` lines of a `.env` file (quotes and `export ` stripped).
fn dotenv(path: &std::path::Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=').filter(|_| !l.starts_with('#'))?;
            Some((k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()))
        })
        .collect()
}

/// The API key from a `.env` file in one of `dirs` (e.g. the project folder) or the working
/// directory, or their parents; else from the environment. A `.env` next to the work wins
/// over a key set globally.
fn env_key_in(dirs: &[std::path::PathBuf]) -> Option<String> {
    let cwd = std::env::current_dir().ok();
    let from_file = dirs.iter().chain(&cwd).flat_map(|d| d.ancestors().take(4)).find_map(|d| {
        let vars = dotenv(&d.join(".env"));
        KEY_VARS.iter().find_map(|k| vars.iter().find(|(n, v)| n == k && !v.is_empty()).map(|(_, v)| v.clone()))
    });
    from_file.or_else(|| {
        KEY_VARS.iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty())).map(|k| k.trim().to_string())
    })
}

fn env_key() -> Option<String> {
    env_key_in(&[])
}

/// The MCP tools as Gemini function declarations.
pub fn declarations() -> Vec<Value> {
    ics_mcp::tools::definitions()
        .into_iter()
        .filter(|t| !HIDDEN.contains(&t["name"].as_str().unwrap_or_default()))
        .map(|t| json!({"name": t["name"], "description": t["description"], "parametersJsonSchema": t["inputSchema"]}))
        .collect()
}

fn system_prompt(context: &str) -> String {
    format!(
        "You are the design copilot of IC Studio, a full-custom IC design environment (schematic, SPICE simulation, \
         layout, DRC/LVS). You act on the project open in the user's window through the tools; the user sees every \
         change live. Layout coordinates are in µm with y up. Work in small verified steps: inspect first \
         (project_info, layout_summary, get_netlist), make the change, then check it (run_drc, fix_drc, run_lvs, \
         simulate). Never invent results: report what the tools return. Do not call save_project unless the user asks to save; edits stay in the window, where the user can undo them. Answer briefly, in the user's language, as plain text without Markdown.\n\n{context}"
    )
}

/// The conversation loop on a worker thread: ask the model, run the tools it calls through
/// `tools`, feed the results back, until it answers in text.
fn converse(
    mut llm: Box<dyn Llm>,
    mut history: Vec<Value>,
    system: String,
    tools: Sender<Pending>,
    events: Sender<Event>,
    wake: egui::Context,
) {
    let send = |e: Event| {
        let _ = events.send(e);
        wake.request_repaint();
    };
    let decl = declarations();
    for _ in 0..MAX_TURNS {
        let req = json!({
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": history,
            "tools": [{"functionDeclarations": decl}],
        });
        let res = match llm.generate(&req) {
            Ok(r) => r,
            Err(e) => {
                send(Event::Entry(Entry::Error(e)));
                break;
            }
        };
        let content = res["candidates"][0]["content"].clone();
        let parts = content["parts"].as_array().cloned().unwrap_or_default();
        if parts.is_empty() {
            let why = res["candidates"][0]["finishReason"].as_str().unwrap_or("no answer").to_string();
            send(Event::Entry(Entry::Error(format!("The model stopped: {why}"))));
            break;
        }
        history.push(json!({"role": "model", "parts": parts}));
        let mut replies = vec![];
        for p in &parts {
            if let Some(t) = p["text"].as_str().filter(|t| !t.trim().is_empty()) {
                send(Event::Entry(Entry::Assistant(t.trim().to_string())));
            }
            let Some(call) = p.get("functionCall") else { continue };
            let name = call["name"].as_str().unwrap_or_default().to_string();
            let args: Map<String, Value> = call["args"].as_object().cloned().unwrap_or_default();
            let (tx, rx) = mpsc::channel();
            let res = if tools.send(Pending { tool: name.clone(), arguments: args.clone(), reply: tx }).is_err() {
                ToolResult::error("the app is gone")
            } else {
                wake.request_repaint();
                rx.recv_timeout(TOOL_TIMEOUT).unwrap_or_else(|_| ToolResult::error("the tool call timed out"))
            };
            send(Event::Entry(Entry::Tool {
                name: name.clone(),
                args: Value::Object(args).to_string(),
                result: res.text.clone(),
                error: res.is_error,
            }));
            let key = if res.is_error { "error" } else { "result" };
            let body: Value = serde_json::from_str(&res.text).unwrap_or(Value::String(res.text));
            replies.push(json!({"functionResponse": {"name": name, "response": {key: body}}}));
        }
        if replies.is_empty() {
            break;
        }
        history.push(json!({"role": "user", "parts": replies}));
    }
    send(Event::Done(history));
}

impl Copilot {
    /// Replace the model client (tests use a scripted one).
    pub fn set_llm(&mut self, factory: LlmFactory) {
        self.factory = factory;
    }

    pub fn busy(&self) -> bool {
        self.events.is_some()
    }

    /// Ask a question; `context` describes the open project and cell for the model.
    pub fn ask(&mut self, ctx: &egui::Context, question: &str, context: &str) {
        self.ask_in(ctx, question, context, &[]);
    }

    /// [`Self::ask`], also looking for a `.env` with the key in `dirs`.
    pub fn ask_in(&mut self, ctx: &egui::Context, question: &str, context: &str, dirs: &[std::path::PathBuf]) {
        let question = question.trim();
        if question.is_empty() || self.busy() {
            return;
        }
        self.log.push(Entry::User(question.to_string()));
        let key = Some(self.key.trim().to_string()).filter(|k| !k.is_empty()).or_else(|| env_key_in(dirs));
        let key = match key {
            Some(k) => k,
            None => {
                self.log.push(Entry::Error("Set GEMINI_API_KEY (environment or .env) or enter an API key above.".into()));
                return;
            }
        };
        let llm = (self.factory)(&key, &self.model);
        let mut history = std::mem::take(&mut self.history);
        history.push(json!({"role": "user", "parts": [{"text": question}]}));
        let (tool_tx, tool_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let system = system_prompt(context);
        let wake = ctx.clone();
        std::thread::spawn(move || converse(llm, history, system, tool_tx, ev_tx, wake));
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
                Event::Done(h) => done = Some(h),
            }
        }
        if let Some(h) = done {
            self.history = h;
            self.events = None;
            self.calls = None;
        }
    }

    pub fn clear(&mut self) {
        if !self.busy() {
            self.log.clear();
            self.history.clear();
        }
    }

    /// The chat window. Returns a question to ask when the user sends one.
    pub fn window(&mut self, ctx: &egui::Context) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut send = None;
        egui::Window::new("Copilot").open(&mut open).default_size([460.0, 560.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let l = ui.label("Model");
                ui.add(egui::TextEdit::singleline(&mut self.model).desired_width(150.0)).labelled_by(l.id);
                let k = ui.label("API key");
                ui.add(
                    egui::TextEdit::singleline(&mut self.key)
                        .password(true)
                        .desired_width(120.0)
                        .hint_text(if env_key().is_some() { "from environment / .env" } else { "GEMINI_API_KEY" }),
                )
                .labelled_by(k.id);
                if ui.add_enabled(!self.busy(), egui::Button::new("Clear")).clicked() {
                    self.clear();
                }
            });
            ui.separator();
            let pal = crate::theme::palette(ui.ctx());
            egui::ScrollArea::vertical().stick_to_bottom(true).max_height(ui.available_height() - 70.0).show(ui, |ui| {
                if self.log.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Ask for a design task, e.g. \"place and route nand2, then fix DRC and run LVS\" or \
                             \"size the OTA for 60 dB gain and check the phase margin\".",
                        )
                        .weak(),
                    );
                }
                for e in &self.log {
                    match e {
                        Entry::User(t) => {
                            ui.label(egui::RichText::new(format!("You: {t}")).strong());
                        }
                        Entry::Assistant(t) => {
                            ui.label(t.replace("**", "").replace('`', ""));
                        }
                        Entry::Tool { name, args, result, error } => {
                            let color = if *error { pal.danger } else { pal.text_weak };
                            let short: String = result.chars().take(160).collect();
                            ui.label(egui::RichText::new(format!("> {name} {args}")).monospace().small().color(color))
                                .on_hover_text(result);
                            ui.label(egui::RichText::new(short).monospace().small().weak());
                        }
                        Entry::Error(t) => {
                            ui.label(egui::RichText::new(t).color(pal.danger));
                        }
                    }
                    ui.add_space(4.0);
                }
                if self.busy() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Working…");
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let l = ui.label("Ask");
                let r =
                    ui.add(egui::TextEdit::multiline(&mut self.input).desired_rows(2).desired_width(ui.available_width() - 60.0));
                let r = r.labelled_by(l.id);
                let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if (ui.add_enabled(!self.busy(), egui::Button::new("Send")).clicked() || enter) && !self.busy() {
                    send = Some(std::mem::take(&mut self.input).trim().to_string());
                }
            });
        });
        self.open = open;
        send.filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dotenv_lines() {
        let d = std::env::temp_dir().join(format!("ics-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".env"), "# c\r\nexport A = \"x y\"\r\nGEMINI_API_KEY='k1'\r\n").unwrap();
        let v = dotenv(&d.join(".env"));
        assert_eq!(v, [("A".to_string(), "x y".to_string()), ("GEMINI_API_KEY".to_string(), "k1".to_string())]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn declarations_are_gemini_functions() {
        let d = declarations();
        assert!(d.iter().any(|t| t["name"] == "run_drc"));
        assert!(d.iter().all(|t| t["name"] != "open_project"));
        assert!(d.iter().all(|t| t["parametersJsonSchema"]["type"] == "object"));
    }
}
