use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::install::{self, Source, Template};
use super::*;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ics-ext-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A Python interpreter, if the machine has one (the process tests are skipped otherwise).
fn python() -> Option<&'static str> {
    ["python3", "python"]
        .into_iter()
        .find(|p| std::process::Command::new(p).arg("-c").arg("import json").output().is_ok_and(|o| o.status.success()))
}

fn write_ext(root: &Path, id: &str, manifest_extra: &str, main_py: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let py = python().unwrap_or("python3");
    let m = format!(
        "[extension]\nid = \"{id}\"\nname = \"Test {id}\"\nversion = \"0.1.0\"\n\n[run]\ncommand = [\"{py}\", \"main.py\"]\n{manifest_extra}"
    );
    std::fs::write(dir.join("extension.toml"), m).unwrap();
    std::fs::write(dir.join("main.py"), main_py).unwrap();
    dir
}

const ECHO: &str = r#"
from ics_extension import Extension
ext = Extension()

@ext.tool("double")
def double(x):
    print("stray output goes to stderr")
    return {"x2": x * 2, "host": ext.host.get("name")}

@ext.tool("ask")
def ask(cell):
    info = ext.call("project_info")
    ext.log("asked " + cell)
    return {"cell": cell, "libs": info["libs"]}

@ext.tool("nested")
def nested():
    return ext.call("echo__double", x=1)

@ext.tool("boom")
def boom():
    raise ValueError("bad input")

if __name__ == "__main__":
    ext.run()
"#;

const TOOLS: &str = r#"
[[tools]]
name = "double"
description = "Double x."
read_only = true
input_schema = { type = "object", properties = { x = { type = "number" } } }

[[tools]]
name = "ask"
description = "Ask the host."

[[tools]]
name = "nested"
description = "Call another extension tool."

[[tools]]
name = "boom"
description = "Fail."

[[commands]]
id = "ask"
title = "Ask about the active cell"
tool = "ask"
arguments = { cell = "${cell}", fixed = 3 }

[[commands]]
id = "drc"
title = "Built-in DRC"
tool = "run_drc"
arguments = { cell = "${cell}" }

[[commands]]
id = "frame"
title = "Frame"
skill = "frame.il"

[[pdks]]
name = "Tiny"
file = "tiny.toml"
"#;

struct Recorder {
    calls: Vec<String>,
    logs: Vec<String>,
}

impl Host for Recorder {
    fn call(&mut self, tool: &str, _args: &Map<String, Value>) -> ToolResult {
        self.calls.push(tool.to_string());
        ToolResult { text: json!({"libs": ["work"]}).to_string(), is_error: false }
    }
    fn log(&mut self, _level: &str, message: &str) {
        self.logs.push(message.to_string());
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn registry_lists_tools_commands_and_pdks() {
    let root = temp("registry");
    let dir = write_ext(&root, "echo", TOOLS, ECHO);
    std::fs::write(dir.join("frame.il"), "printf(\"hi\\n\")").unwrap();
    std::fs::create_dir_all(root.join("broken")).unwrap();
    std::fs::write(root.join("broken/extension.toml"), "[extension]\nid = \"Bad\"").unwrap();
    let reg = Registry::load_from(&root);
    assert_eq!(reg.extensions.len(), 1);
    assert_eq!(reg.errors.len(), 1, "{:?}", reg.errors);
    let defs = reg.definitions();
    assert_eq!(defs.len(), 4);
    assert_eq!(defs[0]["name"], "echo__double");
    assert_eq!(defs[0]["inputSchema"]["properties"]["x"]["type"], "number");
    assert_eq!(defs[1]["inputSchema"]["type"], "object");
    assert!(reg.has_tool("echo__ask") && !reg.has_tool("echo__nope") && !reg.has_tool("ask"));
    assert!(reg.read_only("echo__double") && !reg.read_only("echo__ask"));

    let ctx = Context { cell: Some("work/inv".into()), project: None };
    let inv = reg.invocation("echo", "ask", &ctx).unwrap();
    assert_eq!(inv.tool, "echo__ask");
    assert_eq!(inv.arguments, args(json!({"cell": "work/inv", "fixed": 3})));
    assert_eq!(reg.invocation("echo", "drc", &ctx).unwrap().tool, "run_drc");
    let skill = reg.invocation("echo", "frame", &ctx).unwrap();
    assert_eq!((skill.tool.as_str(), skill.arguments["code"].as_str()), ("run_skill", Some("printf(\"hi\\n\")")));
    assert!(reg.invocation("echo", "ask", &Context::default()).unwrap_err().contains("open cell"));
    assert_eq!(reg.pdks()[0].1, dir.join("tiny.toml"));

    install::set_enabled(&root, "echo", false).unwrap();
    let off = Registry::load_from(&root);
    assert!(off.definitions().is_empty() && off.commands().is_empty() && !off.extensions[0].enabled);
    install::set_enabled(&root, "echo", true).unwrap();
    assert_eq!(Registry::load_from(&root).definitions().len(), 4);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn incompatible_version_is_not_offered() {
    let root = temp("version");
    write_ext(&root, "future", "", "");
    let f = root.join("future/extension.toml");
    let m = std::fs::read_to_string(&f).unwrap().replace("version = \"0.1.0\"", "version = \"0.1.0\"\nics = \">=99.0\"");
    std::fs::write(&f, m).unwrap();
    let reg = Registry::load_from(&root);
    assert!(reg.extensions[0].problem.as_deref().unwrap().contains("99.0"));
    assert!(!reg.extensions[0].usable());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn runs_a_python_extension_with_callbacks() {
    if python().is_none() {
        eprintln!("no Python: skipped");
        return;
    }
    let root = temp("run");
    write_ext(&root, "echo", TOOLS, ECHO);
    let reg = Registry::load_from(&root);
    let mut host = Recorder { calls: vec![], logs: vec![] };

    let r = reg.call("echo__double", &args(json!({"x": 21})), &mut host);
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(r.value().unwrap(), json!({"x2": 42, "host": "ic-studio"}));
    assert!(reg.extensions[0].running());

    let r = reg.call("echo__ask", &args(json!({"cell": "inv"})), &mut host);
    assert_eq!(r.value().unwrap(), json!({"cell": "inv", "libs": ["work"]}), "{}", r.text);
    assert_eq!(host.calls, ["project_info"]);
    assert_eq!(host.logs, ["asked inv"]);

    // Extension tools cannot be called back (the extension is busy with this call).
    let r = reg.call("echo__nested", &Map::new(), &mut host);
    assert!(r.is_error && r.text.contains("cannot call extension tools"), "{}", r.text);
    assert_eq!(host.calls.len(), 1);

    let r = reg.call("echo__boom", &Map::new(), &mut host);
    assert!(r.is_error && r.text.contains("bad input"), "{}", r.text);
    // The program keeps serving after a failed tool.
    assert!(!reg.call("echo__double", &args(json!({"x": 1})), &mut host).is_error);

    reg.stop_all();
    assert!(!reg.extensions[0].running());
    // It starts again on the next call.
    assert!(!reg.call("echo__double", &args(json!({"x": 1})), &mut host).is_error);
    reg.stop_all();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_crashing_program_is_reported_and_restarted() {
    if python().is_none() {
        return;
    }
    let root = temp("crash");
    let main = "import sys\nprint('starting up', file=sys.stderr)\nsys.exit(3)\n";
    write_ext(&root, "crash", "[[tools]]\nname = \"t\"\ndescription = \"d\"\n", main);
    let reg = Registry::load_from(&root);
    let mut host = Recorder { calls: vec![], logs: vec![] };
    let r = reg.call("crash__t", &Map::new(), &mut host);
    assert!(r.is_error && r.text.contains("did not start") && r.text.contains("starting up"), "{}", r.text);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn install_update_remove_from_a_folder() {
    let (src_root, root) = (temp("src"), temp("installed"));
    let src = src_root.join("hello");
    install::scaffold(&src, "hello", Template::Skill).unwrap();
    assert!(install::scaffold(&src, "hello", Template::Skill).is_err());
    std::fs::create_dir_all(src.join(".git")).unwrap();
    let source = Source::parse(src.to_str().unwrap(), None, None).unwrap();
    let (m, dir) = install::install(&root, &source, false).unwrap();
    assert_eq!((m.id(), dir.clone()), ("hello", root.join("hello")));
    assert!(dir.join("hello.il").is_file() && !dir.join(".git").exists());
    assert!(install::install(&root, &source, false).unwrap_err().contains("already installed"));

    std::fs::write(src.join("hello.il"), "printf(\"v2\\n\")").unwrap();
    install::update(&root, "hello").unwrap();
    assert!(std::fs::read_to_string(dir.join("hello.il")).unwrap().contains("v2"));
    let reg = Registry::load_from(&root);
    assert!(matches!(reg.get("hello").unwrap().source(), Some(Source::Folder(_))));
    assert_eq!(reg.commands().len(), 1);

    install::remove(&root, "hello").unwrap();
    assert!(Registry::load_from(&root).extensions.is_empty());
    assert!(install::remove(&root, "hello").is_err());
    assert!(Source::parse("not-a-folder-or-url", None, None).is_err());
    assert!(matches!(Source::parse("https://example.com/x.git", None, Some("ext".into())), Ok(Source::Git { .. })));
    // Unauthenticated transports, sub-folders leaving the repository, option-like arguments.
    assert!(Source::parse("http://example.com/x.git", None, None).is_err());
    assert!(Source::parse("git://example.com/x.git", None, None).is_err());
    assert!(Source::parse("-uevil.git", None, None).is_err());
    for p in ["../x", "/abs", "-x", "a/../../b"] {
        assert!(Source::parse("https://example.com/x.git", None, Some(p.into())).is_err(), "{p}");
    }
    assert!(Source::parse("https://example.com/x.git", Some("--upload-pack=x".into()), None).is_err());
    let _ = std::fs::remove_dir_all(&src_root);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn python_template_runs() {
    if python().is_none() {
        return;
    }
    let root = temp("template");
    install::scaffold(&root.join("counter"), "counter", Template::Python).unwrap();
    // The template's python3 / python choice; use the interpreter this machine has.
    let f = root.join("counter/extension.toml");
    let m = std::fs::read_to_string(&f).unwrap().replace("\"python3\"", &format!("\"{}\"", python().unwrap()));
    std::fs::write(&f, m).unwrap();
    let reg = Registry::load_from(&root);
    assert!(reg.errors.is_empty(), "{:?}", reg.errors);
    let deck = "* inv\nM1 out in 0 0 nmos W=1u L=0.18u\nM2 out in vdd vdd pmos\nC1 out 0 1f\n.end\n";
    let mut host = |tool: &str, _: &Map<String, Value>| {
        assert_eq!(tool, "get_netlist");
        ToolResult { text: json!({"deck": deck}).to_string(), is_error: false }
    };
    let r = reg.call("counter__count_devices", &args(json!({"cell": "inv"})), &mut host);
    assert_eq!(r.value().unwrap(), json!({"cell": "inv", "devices": {"M": 2, "C": 1}}), "{}", r.text);
    reg.stop_all();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_repository_index_and_bundled_extensions_are_valid() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions");
    let index = install::fetch_index(repo.join("index.json").to_str().unwrap()).unwrap();
    assert!(!index.is_empty());
    for e in &index {
        assert!(e.repository.starts_with("https://"), "{}", e.id);
        // Entries in other repositories are pinned: the index then fixes what gets installed.
        let own = e.repository.trim_end_matches(".git").ends_with("geniuskey/ic-studio");
        assert!(own || e.commit.as_deref().is_some_and(|c| c.len() >= 40), "{}: pin a full commit hash", e.id);
        if let Some(p) = e.path.as_deref().filter(|_| e.repository.ends_with("geniuskey/ic-studio")) {
            let m = install::read_manifest(&repo.join("..").join(p)).unwrap_or_else(|err| panic!("{}: {err}", e.id));
            assert_eq!(m.id(), e.id);
        }
    }
    assert!(index[0].matches("") && !index[0].matches("zzz-no-such"));
}

#[test]
fn mcp_results_and_bare_values() {
    let r = process::tool_result(&json!({"content": [{"type": "text", "text": "{\"a\":1}"}], "isError": false}));
    assert_eq!(r.value().unwrap(), json!({"a": 1}));
    let r = process::tool_result(&json!({"content": [{"type": "text", "text": "no"}], "isError": true}));
    assert!(r.is_error);
    assert_eq!(process::tool_result(&json!({"b": 2})).value().unwrap(), json!({"b": 2}));
    assert_eq!(process::tool_result(&json!("plain")).text, "plain");
}

/// While a tool runs, `running` and `stop` never wait for it (the UI calls them every frame),
/// and `stop` ends the call.
#[test]
fn stop_and_running_do_not_wait_for_a_call() {
    if python().is_none() {
        return;
    }
    let root = temp("slow");
    let slow = r#"
import time
from ics_extension import Extension
ext = Extension()

@ext.tool("sleep")
def sleep():
    time.sleep(60)
    return "woke"

if __name__ == "__main__":
    ext.run()
"#;
    write_ext(&root, "slow", "[[tools]]\nname = \"sleep\"\ndescription = \"Sleep.\"\n", slow);
    let reg = Registry::load_from(&root);
    let worker = {
        let reg = reg.clone();
        std::thread::spawn(move || {
            let mut host = Recorder { calls: vec![], logs: vec![] };
            reg.call("slow__sleep", &Map::new(), &mut host)
        })
    };
    let e = reg.get("slow").unwrap();
    let t = std::time::Instant::now();
    while !e.running() && t.elapsed().as_secs() < 20 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let t = std::time::Instant::now();
    assert!(e.running());
    e.stop();
    assert!(t.elapsed().as_millis() < 100, "running/stop waited {:?}", t.elapsed());
    let r = worker.join().unwrap();
    assert!(r.is_error && r.text.contains("cancelled"), "{}", r.text);
    assert!(t.elapsed().as_secs() < 5, "the call took {:?} to stop", t.elapsed());
    assert!(!e.running());
}

/// A tool declared read-only (which plan mode trusts) cannot call editing tools back.
#[test]
fn read_only_tools_call_back_read_only_tools_only() {
    let reg = Registry::default();
    let mut rec = Recorder { calls: vec![], logs: vec![] };
    let mut g = Guard { reg: &reg, host: &mut rec, read_only: true };
    assert!(g.call("save_project", &Map::new()).is_error);
    assert!(g.call("delete_items", &Map::new()).is_error);
    assert!(!g.call("project_info", &Map::new()).is_error);
    let mut g = Guard { reg: &reg, host: &mut rec, read_only: false };
    assert!(!g.call("save_project", &Map::new()).is_error);
    assert_eq!(rec.calls, ["project_info", "save_project"]);
}

/// A git source pinned to a commit installs that commit, whatever the branch has since, and
/// an index entry's id must match the manifest's.
#[test]
fn pinned_git_install() {
    let git = |dir: &Path, args: &[&str]| {
        let o = std::process::Command::new("git").args(args).current_dir(dir).output();
        o.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let root = temp("pinned");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    if git(&repo, &["init", "-q"]).is_none() {
        return; // no git
    }
    let manifest = |v: &str| format!("[extension]\nid = \"pin\"\nname = \"Pin\"\nversion = \"{v}\"\n");
    let commit = |v: &str| {
        std::fs::write(repo.join("extension.toml"), manifest(v)).unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", v]).unwrap();
        git(&repo, &["rev-parse", "HEAD"]).unwrap()
    };
    let first = commit("1.0.0");
    commit("2.0.0");
    // A plain local path: git clones it on every platform (a file:// URL of a Windows path is not portable).
    let url = repo.display().to_string();
    let dest = root.join("installed");
    let src = Source::Git { url: url.clone(), rev: None, path: None, commit: Some(first.clone()), id: Some("pin".into()) };
    let (m, _) = install::install(&dest, &src, false).unwrap();
    assert_eq!(m.extension.version, "1.0.0");
    let wrong = Source::Git { url: url.clone(), rev: None, path: None, commit: None, id: Some("other".into()) };
    assert!(install::install(&dest, &wrong, true).unwrap_err().contains("not 'other'"));
    let short = Source::Git { url, rev: None, path: None, commit: Some(first[..8].into()), id: None };
    assert!(install::install(&dest, &short, true).is_err());
    let _ = std::fs::remove_dir_all(&root);
}
