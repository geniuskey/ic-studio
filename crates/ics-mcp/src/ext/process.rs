//! An extension's program: a child process speaking newline-delimited JSON-RPC 2.0 on stdin /
//! stdout (stderr is its log).
//!
//! Host to extension: `initialize` (once), `tools/call` `{name, arguments}` answered like MCP
//! (`{content: [{type: "text", text}], isError}`; any other result is taken as the JSON value
//! of a successful call), and the `shutdown` notification before stdin closes.
//!
//! Extension to host, while a call is in progress: the request `ics/call` `{name, arguments}`
//! runs any IC Studio tool on the open project and is answered like `tools/call`; the
//! notification `ics/log` `{level, message}` writes to the console. A plain MCP server (tools
//! only) is therefore a valid extension too.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::Host;
use crate::ToolResult;

/// Protocol version sent in `initialize`.
pub const PROTOCOL: u32 = 1;
const MCP_VERSION: &str = "2025-06-18";
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Simulations and DRC on large layouts called back from an extension can take a while.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(1800);
const STDERR_LINES: usize = 40;

pub struct Process {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    next_id: u64,
}

/// What an extension's program is started with.
pub struct Launch<'a> {
    pub id: &'a str,
    pub dir: &'a Path,
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    /// Folder holding the bundled `ics_extension.py`, put on `PYTHONPATH`.
    pub sdk: Option<PathBuf>,
}

/// `program` as given when it is a bare name looked up on the PATH, else relative to `dir`.
fn resolve(dir: &Path, program: &str) -> PathBuf {
    let p = Path::new(program);
    if p.is_absolute() || p.components().count() == 1 && !program.starts_with('.') { p.to_path_buf() } else { dir.join(p) }
}

impl Process {
    pub fn start(l: &Launch, host: &mut dyn Host) -> Result<Process, String> {
        let program = resolve(l.dir, &l.argv[0]);
        let mut cmd = Command::new(&program);
        let subst = |s: &str| s.replace("${extension}", &l.dir.to_string_lossy());
        cmd.args(l.argv[1..].iter().map(|a| subst(a)))
            .current_dir(l.dir)
            .env("ICS_EXTENSION_ID", l.id)
            .env("ICS_EXTENSION_DIR", l.dir)
            .env("ICS_VERSION", env!("CARGO_PKG_VERSION"))
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(sdk) = &l.sdk {
            let mut paths = vec![sdk.clone()];
            paths.extend(
                std::env::var_os("PYTHONPATH").map(|v| std::env::split_paths(&v).collect::<Vec<_>>()).unwrap_or_default(),
            );
            if let Ok(v) = std::env::join_paths(paths) {
                cmd.env("PYTHONPATH", v);
            }
        }
        for (k, v) in l.env {
            cmd.env(k, subst(v));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", program.display()))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let (tx, lines) = mpsc::channel();
        let id = l.id.to_string();
        std::thread::Builder::new()
            .name(format!("ext-{id}-out"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if !line.trim().is_empty() && tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr.clone();
        std::thread::Builder::new()
            .name(format!("ext-{id}-err"))
            .spawn(move || {
                for line in BufReader::new(stderr_pipe).lines() {
                    let Ok(line) = line else { break };
                    eprintln!("[{id}] {line}");
                    let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                    if t.len() == STDERR_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            })
            .map_err(|e| e.to_string())?;
        let mut p = Process { child, stdin, lines, stderr, next_id: 1 };
        // MCP's initialize, so a plain MCP server works as an extension too.
        let info = json!({"name": "ic-studio", "version": env!("CARGO_PKG_VERSION")});
        let params = json!({
            "protocolVersion": MCP_VERSION,
            "icsProtocol": PROTOCOL,
            "clientInfo": info,
            "hostInfo": info,
            "extension": {"id": l.id, "dir": l.dir},
            "capabilities": {"ics": {"call": true, "log": true}},
        });
        p.request("initialize", params, START_TIMEOUT, host).map_err(|e| format!("{id} did not start: {e}", id = l.id))?;
        p.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(p)
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{msg}").and_then(|_| self.stdin.flush()).map_err(|e| format!("cannot write to the extension: {e}"))
    }

    /// The last lines the program wrote to stderr, for error messages.
    fn stderr_tail(&self) -> String {
        let t = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        if t.is_empty() { String::new() } else { format!("\n{}", t.iter().cloned().collect::<Vec<_>>().join("\n")) }
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Send a request and serve the extension's calls to the host until its answer arrives.
    pub fn request(&mut self, method: &str, params: Value, timeout: Duration, host: &mut dyn Host) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let line = match self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    return Err(format!("no answer to {method} within {} s; the extension was stopped", timeout.as_secs()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(50));
                    let status =
                        self.child.try_wait().ok().flatten().map_or("closed its output".into(), |s| format!("exited ({s})"));
                    return Err(format!("the extension {status}{}", self.stderr_tail()));
                }
            };
            let msg: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => {
                    // Stray prints on stdout are not protocol messages; show them as a log line.
                    host.log("info", &line);
                    continue;
                }
            };
            if let Some(m) = msg.get("method").and_then(Value::as_str) {
                self.serve(m, &msg, host)?;
                continue;
            }
            if msg.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(e) = msg.get("error") {
                return Err(e.get("message").and_then(Value::as_str).map_or_else(|| e.to_string(), str::to_string));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// A request or notification from the extension.
    fn serve(&mut self, method: &str, msg: &Value, host: &mut dyn Host) -> Result<(), String> {
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let reply = match method {
            "ics/log" => {
                let level = params.get("level").and_then(Value::as_str).unwrap_or("info");
                host.log(level, params.get("message").and_then(Value::as_str).unwrap_or_default());
                None
            }
            "ics/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                let empty = Map::new();
                let args = params.get("arguments").and_then(Value::as_object).unwrap_or(&empty);
                let r = if name.is_empty() { ToolResult::error("ics/call: missing tool name") } else { host.call(name, args) };
                Some(Ok(json!({"content": [{"type": "text", "text": r.text}], "isError": r.is_error})))
            }
            "ping" => Some(Ok(json!({}))),
            _ => Some(Err(json!({"code": -32601, "message": format!("method not found: {method}")}))),
        };
        match (msg.get("id"), reply) {
            (Some(id), Some(Ok(result))) => self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result})),
            (Some(id), Some(Err(error))) => self.send(&json!({"jsonrpc": "2.0", "id": id, "error": error})),
            _ => Ok(()),
        }
    }

    /// `tools/call`, with the answer as a tool result.
    pub fn call_tool(&mut self, tool: &str, args: &Map<String, Value>, host: &mut dyn Host) -> Result<ToolResult, String> {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}), CALL_TIMEOUT, host)?;
        Ok(tool_result(&r))
    }
}

/// An MCP `tools/call` result (or a bare value) as a tool result.
pub fn tool_result(r: &Value) -> ToolResult {
    match r.get("content").and_then(Value::as_array) {
        Some(parts) => {
            let text: Vec<&str> = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
            let structured = r.get("structuredContent").filter(|_| text.is_empty()).map(Value::to_string);
            ToolResult {
                text: structured.unwrap_or_else(|| text.join("\n")),
                is_error: r.get("isError").and_then(Value::as_bool).unwrap_or(false),
            }
        }
        None => ToolResult {
            text: if r.is_string() { r.as_str().unwrap_or_default().to_string() } else { r.to_string() },
            is_error: false,
        },
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.send(&json!({"jsonrpc": "2.0", "method": "shutdown"}));
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
