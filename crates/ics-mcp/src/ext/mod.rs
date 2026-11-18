//! Third-party extensions: folders with an `extension.toml` (see [`manifest`]) in the user's
//! extensions folder ([`user_dir`]). An extension adds tools (served by its own program over
//! the protocol in [`process`]), menu commands, SKILL scripts and PDK files. Its tools are
//! listed with the built-in ones as `<id>__<tool>`, so the desktop app, the design copilot,
//! `ics mcp` agents, the CLI and the Python API can all call them, and its program can call
//! every built-in tool back on the open project.
//!
//! Extensions run as separate processes with the user's rights: install only ones you trust.

pub mod install;
pub mod manifest;
pub mod process;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

pub use manifest::{CommandDef, Manifest, SEP};

use crate::ToolResult;
use process::{Launch, Process};

/// The Python helper module extensions can `import ics_extension` (put on `PYTHONPATH`).
pub const PYTHON_SDK: &str = include_str!("sdk/ics_extension.py");

/// Where an extension's program reaches IC Studio while one of its tools runs.
pub trait Host {
    /// Run a built-in tool on the open project.
    fn call(&mut self, tool: &str, args: &Map<String, Value>) -> ToolResult;
    /// A log line from the extension.
    fn log(&mut self, level: &str, message: &str) {
        eprintln!("extension {level}: {message}");
    }
}

impl<F: FnMut(&str, &Map<String, Value>) -> ToolResult> Host for F {
    fn call(&mut self, tool: &str, args: &Map<String, Value>) -> ToolResult {
        self(tool, args)
    }
}

/// The user's extensions folder: `ICS_EXTENSIONS_DIR`, else `ic-studio/extensions` in the
/// per-user data folder (`%APPDATA%`, `~/Library/Application Support`, `$XDG_DATA_HOME` or
/// `~/.local/share`).
pub fn user_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = var("ICS_EXTENSIONS_DIR") {
        return Some(d);
    }
    let data = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
    };
    data.map(|d| d.join("ic-studio").join("extensions"))
}

/// One installed extension.
pub struct Extension {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub enabled: bool,
    /// Why it cannot be used (e.g. it needs another IC Studio version).
    pub problem: Option<String>,
    process: Mutex<Option<Process>>,
}

impl Extension {
    pub fn id(&self) -> &str {
        self.manifest.id()
    }

    pub fn usable(&self) -> bool {
        self.enabled && self.problem.is_none()
    }

    /// Whether its program is running.
    pub fn running(&self) -> bool {
        self.process.lock().map(|mut p| p.as_mut().is_some_and(Process::alive)).unwrap_or(false)
    }

    /// Stop its program (it starts again on the next call).
    pub fn stop(&self) {
        if let Ok(mut p) = self.process.lock() {
            p.take();
        }
    }

    /// Install source recorded by [`install::install`], if any.
    pub fn source(&self) -> Option<install::Source> {
        install::recorded_source(&self.dir)
    }

    fn call(&self, tool: &str, args: &Map<String, Value>, host: &mut dyn Host) -> ToolResult {
        let Some(run) = &self.manifest.run else {
            return ToolResult::error(format!("extension {} has no program", self.id()));
        };
        let mut slot = match self.process.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if slot.as_mut().is_some_and(|p| !p.alive()) {
            *slot = None;
        }
        if slot.is_none() {
            let env: Vec<(String, String)> = run.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let launch =
                Launch { id: self.id(), dir: &self.dir, argv: run.argv(), env: &env, sdk: self.dir.parent().and_then(sdk_dir) };
            match Process::start(&launch, host) {
                Ok(p) => *slot = Some(p),
                Err(e) => return ToolResult::error(e),
            }
        }
        let p = slot.as_mut().expect("started above");
        match p.call_tool(tool, args, host) {
            Ok(r) => r,
            Err(e) => {
                *slot = None;
                ToolResult::error(format!("extension {}: {e}", self.id()))
            }
        }
    }
}

/// Write the bundled Python SDK next to the extensions (once per version) and return its folder.
fn sdk_dir(root: &Path) -> Option<PathBuf> {
    let dir = root.join(".sdk").join(env!("CARGO_PKG_VERSION"));
    let file = dir.join("ics_extension.py");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(PYTHON_SDK) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&file, PYTHON_SDK).ok()?;
    }
    Some(dir)
}

/// A menu command of an extension, ready to run: `tool` with `arguments`.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub tool: String,
    pub arguments: Map<String, Value>,
}

/// What `${...}` in command arguments stands for.
#[derive(Default)]
pub struct Context {
    /// The active cell as `lib/cell`.
    pub cell: Option<String>,
    pub project: Option<PathBuf>,
}

/// The installed extensions. Cheap to clone; clones share the running programs.
#[derive(Clone, Default)]
pub struct Registry {
    pub dir: Option<PathBuf>,
    pub extensions: Vec<Arc<Extension>>,
    /// Folders that could not be loaded, with the reason.
    pub errors: Vec<(PathBuf, String)>,
}

impl Registry {
    /// The user's extensions (none with `ICS_NO_EXTENSIONS=1`).
    pub fn load() -> Self {
        if std::env::var_os("ICS_NO_EXTENSIONS").is_some_and(|v| v == "1") {
            return Self::default();
        }
        user_dir().map(|d| Self::load_from(&d)).unwrap_or_default()
    }

    /// The extensions in the sub-folders of `dir`.
    pub fn load_from(dir: &Path) -> Self {
        let disabled = install::disabled(dir);
        let mut reg = Registry { dir: Some(dir.to_path_buf()), ..Self::default() };
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join(install::MANIFEST).is_file()).collect())
            .unwrap_or_default();
        entries.sort();
        for path in entries {
            if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                continue;
            }
            match install::read_manifest(&path) {
                Ok(m) if reg.get(m.id()).is_some() => reg.errors.push((path, format!("extension id '{}' is used twice", m.id()))),
                Ok(m) => {
                    let problem = m
                        .extension
                        .ics
                        .as_deref()
                        .filter(|req| !manifest::satisfies(req, env!("CARGO_PKG_VERSION")))
                        .map(|req| format!("needs IC Studio {req} (this is {})", env!("CARGO_PKG_VERSION")));
                    let enabled = !disabled.contains(m.id());
                    reg.extensions.push(Arc::new(Extension {
                        manifest: m,
                        dir: path,
                        enabled,
                        problem,
                        process: Mutex::new(None),
                    }));
                }
                Err(e) => reg.errors.push((path, e)),
            }
        }
        reg
    }

    pub fn get(&self, id: &str) -> Option<&Arc<Extension>> {
        self.extensions.iter().find(|e| e.id() == id)
    }

    fn usable(&self) -> impl Iterator<Item = &Arc<Extension>> {
        self.extensions.iter().filter(|e| e.usable())
    }

    /// The tools of the enabled extensions as MCP tool definitions.
    pub fn definitions(&self) -> Vec<Value> {
        self.usable()
            .flat_map(|e| {
                e.manifest.tools.iter().map(|t| {
                    json!({
                        "name": e.manifest.tool_name(&t.name),
                        "description": format!("[extension {}] {}", e.manifest.extension.name, t.description),
                        "inputSchema": t.input_schema.clone().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    })
                })
            })
            .collect()
    }

    /// The extension and its own tool name behind an agent-facing tool name.
    fn resolve<'a>(&'a self, name: &'a str) -> Option<(&'a Arc<Extension>, &'a str)> {
        let (id, tool) = name.split_once(SEP)?;
        let e = self.usable().find(|e| e.id() == id)?;
        e.manifest.tools.iter().any(|t| t.name == tool).then_some((e, tool))
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }

    /// Whether an extension tool is declared read-only.
    pub fn read_only(&self, name: &str) -> bool {
        self.resolve(name).is_some_and(|(e, t)| e.manifest.tools.iter().any(|d| d.name == t && d.read_only))
    }

    /// Run an extension tool. Its program's calls back go to `host`; calls of extension tools
    /// from there are refused (an extension waits for its own answer).
    pub fn call(&self, name: &str, args: &Map<String, Value>, host: &mut dyn Host) -> ToolResult {
        let Some((e, tool)) = self.resolve(name) else {
            return ToolResult::error(format!("unknown tool: {name}"));
        };
        let mut guarded = Guard { reg: self, host };
        e.call(tool, args, &mut guarded)
    }

    /// Menu commands of the enabled extensions: (extension, command).
    pub fn commands(&self) -> Vec<(Arc<Extension>, CommandDef)> {
        self.usable().flat_map(|e| e.manifest.commands.iter().map(move |c| (e.clone(), c.clone()))).collect()
    }

    /// What running command `cmd` of extension `id` means in `ctx`.
    pub fn invocation(&self, id: &str, cmd: &str, ctx: &Context) -> Result<Invocation, String> {
        let e = self.usable().find(|e| e.id() == id).ok_or_else(|| format!("no enabled extension '{id}'"))?;
        let c = e.manifest.commands.iter().find(|c| c.id == cmd).ok_or_else(|| format!("{id} has no command '{cmd}'"))?;
        let fill = |s: &str| {
            let mut s = s.replace("${extension}", &e.dir.to_string_lossy());
            if let Some(p) = &ctx.project {
                s = s.replace("${project}", &p.to_string_lossy());
            }
            if let Some(c) = &ctx.cell {
                s = s.replace("${cell}", c);
            }
            s
        };
        if let Some(file) = &c.skill {
            let path = e.dir.join(file);
            let code = std::fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            let mut arguments = Map::new();
            arguments.insert("code".into(), code.into());
            if let Some(cell) = &ctx.cell {
                arguments.insert("cell".into(), cell.clone().into());
            }
            return Ok(Invocation { tool: "run_skill".into(), arguments });
        }
        let tool = c.tool.as_deref().unwrap_or_default();
        let tool = if e.manifest.tools.iter().any(|t| t.name == tool) { e.manifest.tool_name(tool) } else { tool.to_string() };
        let mut arguments = Map::new();
        for (k, v) in c.arguments.iter().flatten() {
            let v = match v {
                Value::String(s) => {
                    let filled = fill(s);
                    if filled.contains("${cell}") || filled.contains("${project}") {
                        return Err(format!(
                            "{}: needs {}",
                            c.title,
                            if filled.contains("${cell}") { "an open cell" } else { "a saved project" }
                        ));
                    }
                    Value::String(filled)
                }
                v => v.clone(),
            };
            arguments.insert(k.clone(), v);
        }
        Ok(Invocation { tool, arguments })
    }

    /// PDK files offered by the enabled extensions: (label, file).
    pub fn pdks(&self) -> Vec<(String, PathBuf)> {
        self.usable()
            .flat_map(|e| e.manifest.pdks.iter().map(|p| (format!("{} ({})", p.name, e.id()), e.dir.join(&p.file))))
            .collect()
    }

    /// Stop every running program.
    pub fn stop_all(&self) {
        for e in &self.extensions {
            e.stop();
        }
    }
}

struct Guard<'a> {
    reg: &'a Registry,
    host: &'a mut dyn Host,
}

impl Host for Guard<'_> {
    fn call(&mut self, tool: &str, args: &Map<String, Value>) -> ToolResult {
        if self.reg.has_tool(tool) || tool.contains(SEP) {
            return ToolResult::error(format!("{tool}: an extension cannot call extension tools"));
        }
        self.host.call(tool, args)
    }

    fn log(&mut self, level: &str, message: &str) {
        self.host.log(level, message);
    }
}

#[cfg(test)]
mod tests;
