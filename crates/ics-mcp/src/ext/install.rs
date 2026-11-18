//! Installing, removing, enabling and creating extensions, and the public extension index.
//!
//! An extension is installed into `<extensions folder>/<id>` from a local folder or a git
//! repository (optionally a sub-folder and a tag / branch); where it came from is kept in
//! `.source.json` there so `update` can fetch it again. Disabled ids are listed in
//! `state.json` of the extensions folder.
//!
//! The index (`extensions/index.json` in the IC Studio repository, or `ICS_EXTENSION_INDEX`) is
//! the community list of published extensions:
//! `{"extensions": [{"id", "name", "description", "repository", "path"?, "rev"?, ...}]}`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::Manifest;

pub const MANIFEST: &str = "extension.toml";
const STATE: &str = "state.json";
const SOURCE: &str = ".source.json";
/// The community index of published extensions.
pub const DEFAULT_INDEX: &str = "https://raw.githubusercontent.com/geniuskey/ic-studio/main/extensions/index.json";

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let f = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
    Manifest::parse(&text)
}

/// Where an extension comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Folder(PathBuf),
    Git { url: String, rev: Option<String>, path: Option<String> },
}

impl Source {
    /// A folder that exists, else a git URL. `rev` and `path` (a sub-folder) only apply to git.
    pub fn parse(s: &str, rev: Option<String>, path: Option<String>) -> Result<Source, String> {
        let p = Path::new(s);
        if p.is_dir() {
            if rev.is_some() {
                return Err("a revision only applies to a git repository".into());
            }
            return Ok(Source::Folder(path.map_or_else(|| p.to_path_buf(), |sub| p.join(sub))));
        }
        let git = s.starts_with("https://")
            || s.starts_with("http://")
            || s.starts_with("git@")
            || s.starts_with("ssh://")
            || s.starts_with("file://")
            || s.ends_with(".git");
        if !git {
            return Err(format!("'{s}' is neither a folder nor a git URL"));
        }
        Ok(Source::Git { url: s.to_string(), rev, path })
    }

    fn to_json(&self) -> Value {
        match self {
            Source::Folder(p) => json!({"folder": p}),
            Source::Git { url, rev, path } => json!({"git": url, "rev": rev, "path": path}),
        }
    }

    fn from_json(v: &Value) -> Option<Source> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match (s("folder"), s("git")) {
            (Some(f), _) => Some(Source::Folder(f.into())),
            (None, Some(url)) => Some(Source::Git { url, rev: s("rev"), path: s("path") }),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Folder(p) => p.display().to_string(),
            Source::Git { url, rev, path } => {
                let mut s = url.clone();
                if let Some(p) = path {
                    s += &format!(" ({p})");
                }
                if let Some(r) = rev {
                    s += &format!(" @ {r}");
                }
                s
            }
        }
    }
}

pub fn recorded_source(dir: &Path) -> Option<Source> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(SOURCE)).ok()?).ok()?;
    Source::from_json(&v)
}

/// Ids switched off in `root`.
pub fn disabled(root: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(root.join(STATE))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["disabled"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

pub fn set_enabled(root: &Path, id: &str, enabled: bool) -> Result<(), String> {
    if !root.join(id).join(MANIFEST).is_file() {
        return Err(format!("no installed extension '{id}'"));
    }
    let mut off = disabled(root);
    if enabled {
        off.remove(id);
    } else {
        off.insert(id.to_string());
    }
    let v = json!({"disabled": off});
    std::fs::write(root.join(STATE), serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())
}

fn git(args: &[&str], dir: Option<&Path>) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("cannot run git ({e}); install git to add extensions from a repository"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("git {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let n = name.to_string_lossy();
        if matches!(n.as_ref(), ".git" | "__pycache__" | "target" | "node_modules" | ".venv" | SOURCE) {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_dir() {
            copy_dir(&src, &dst)?;
        } else if ty.is_file() {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// Install `src` into `root`, replacing an installed copy of the same id when `replace`.
/// Returns its manifest and folder.
pub fn install(root: &Path, src: &Source, replace: bool) -> Result<(Manifest, PathBuf), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let tmp = root.join(format!(".tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let res = (|| {
        let folder = match src {
            Source::Folder(p) => p.clone(),
            Source::Git { url, rev, path } => {
                let t = tmp.to_string_lossy().to_string();
                let mut args = vec!["clone", "--depth", "1", "--quiet"];
                if path.is_some() {
                    args.extend(["--filter=blob:none", "--sparse"]);
                }
                if let Some(r) = rev {
                    args.extend(["--branch", r]);
                }
                args.extend([url.as_str(), t.as_str()]);
                git(&args, None)?;
                match path {
                    Some(p) => {
                        git(&["sparse-checkout", "set", p], Some(&tmp))?;
                        tmp.join(p)
                    }
                    None => tmp.clone(),
                }
            }
        };
        let m = read_manifest(&folder)?;
        let dest = root.join(m.id());
        if dest.exists() {
            if !replace {
                return Err(format!("extension '{}' is already installed (update or remove it first)", m.id()));
            }
            std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        copy_dir(&folder, &dest)?;
        let source = match src {
            Source::Folder(p) => Source::Folder(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())),
            s => s.clone(),
        };
        let _ = std::fs::write(dest.join(SOURCE), serde_json::to_string_pretty(&source.to_json()).unwrap_or_default());
        Ok((m, dest))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

/// Install the recorded source of an installed extension again.
pub fn update(root: &Path, id: &str) -> Result<(Manifest, PathBuf), String> {
    let dir = root.join(id);
    let src = recorded_source(&dir).ok_or_else(|| format!("'{id}' has no recorded source; install it again"))?;
    install(root, &src, true)
}

pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    let dir = root.join(id);
    if !dir.join(MANIFEST).is_file() {
        return Err(format!("no installed extension '{id}'"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = set_enabled_quiet(root, id);
    Ok(())
}

fn set_enabled_quiet(root: &Path, id: &str) -> Result<(), String> {
    let mut off = disabled(root);
    if off.remove(id) {
        std::fs::write(root.join(STATE), json!({"disabled": off}).to_string()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Kinds of extension `new` can start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Template {
    /// A Python program with one tool and a menu command.
    Python,
    /// No program: a SKILL script as a menu command.
    Skill,
}

/// Write a new extension `id` into `dir` (which must not exist yet).
pub fn scaffold(dir: &Path, id: &str, template: Template) -> Result<(), String> {
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let name = id.split('-').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<Vec<_>>().join(" ");
    let head = format!(
        "[extension]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\ndescription = \"\"\nauthors = []\n\
         license = \"MIT\"\n# repository = \"https://github.com/you/{id}\"\nics = \">={}\"\n",
        env!("CARGO_PKG_VERSION")
    );
    let mut files: Vec<(&str, String)> = vec![];
    match template {
        Template::Python => {
            files.push((
                MANIFEST,
                format!(
                    "{head}\n[run]\ncommand = [\"python3\", \"main.py\"]\nwindows = [\"python\", \"main.py\"]\n\n\
                     [[tools]]\nname = \"count_devices\"\ndescription = \"Count the devices of a cell's schematic by kind.\"\n\
                     read_only = true\ninput_schema = {{ type = \"object\", properties = {{ cell = {{ type = \"string\", \
                     description = \"Cell (cell or lib/cell)\" }} }}, required = [\"cell\"] }}\n\n\
                     [[commands]]\nid = \"count\"\ntitle = \"Count devices of the active cell\"\ntool = \"count_devices\"\n\
                     arguments = {{ cell = \"${{cell}}\" }}\n"
                ),
            ));
            files.push(("main.py", SCAFFOLD_PY.to_string()));
        }
        Template::Skill => {
            files.push((
                MANIFEST,
                format!("{head}\n[[commands]]\nid = \"hello\"\ntitle = \"Say hello from SKILL\"\nskill = \"hello.il\"\n"),
            ));
            files
                .push(("hello.il", "; Runs on the active cell (geGetEditCellView()).\nprintf(\"hello from SKILL\\n\")\n".into()));
        }
    }
    files.push(("README.md", format!("# {name}\n\nAn IC Studio extension.\n\n```sh\nics ext install .\n```\n")));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (f, text) in files {
        std::fs::write(dir.join(f), text).map_err(|e| format!("{f}: {e}"))?;
    }
    Ok(())
}

const SCAFFOLD_PY: &str = r#""""An IC Studio extension. Run `ics ext install .` in this folder, then call
`<id>__count_devices` from the Extensions menu, the copilot or `ics ext call`."""

from collections import Counter

from ics_extension import Extension  # provided by IC Studio on PYTHONPATH

ext = Extension()


@ext.tool("count_devices")
def count_devices(cell: str):
    # Any built-in IC Studio tool can be called on the open project.
    netlist = ext.call("get_netlist", cell=cell)["deck"]
    kinds = Counter(
        line.split()[0][0].upper()
        for line in netlist.splitlines()
        if line.strip() and not line.startswith(("*", "."))
    )
    ext.log(f"{cell}: {sum(kinds.values())} devices")
    return {"cell": cell, "devices": dict(kinds)}


if __name__ == "__main__":
    ext.run()
"#;

/// One entry of the extension index.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository: String,
    pub path: Option<String>,
    pub rev: Option<String>,
    pub license: Option<String>,
    pub authors: Vec<String>,
    pub tags: Vec<String>,
}

impl IndexEntry {
    pub fn source(&self) -> Source {
        Source::Git { url: self.repository.clone(), rev: self.rev.clone(), path: self.path.clone() }
    }

    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        q.is_empty()
            || [&self.id, &self.name, &self.description].iter().any(|s| s.to_lowercase().contains(&q))
            || self.tags.iter().any(|t| t.to_lowercase() == q)
    }
}

pub fn parse_index(text: &str) -> Result<Vec<IndexEntry>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("extension index: {e}"))?;
    let list = v["extensions"].as_array().ok_or("extension index: no 'extensions' list")?;
    let s = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
    let strings = |e: &Value, k: &str| -> Vec<String> {
        e.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    list.iter()
        .map(|e| {
            let id = s(e, "id").ok_or("extension index: an entry without id")?;
            Ok(IndexEntry {
                name: s(e, "name").unwrap_or_else(|| id.clone()),
                description: s(e, "description").unwrap_or_default(),
                repository: s(e, "repository").ok_or_else(|| format!("extension index: {id} has no repository"))?,
                path: s(e, "path"),
                rev: s(e, "rev"),
                license: s(e, "license"),
                authors: strings(e, "authors"),
                tags: strings(e, "tags"),
                id,
            })
        })
        .collect()
}

/// The index location: `ICS_EXTENSION_INDEX` (URL or file), else [`DEFAULT_INDEX`].
pub fn index_location() -> String {
    std::env::var("ICS_EXTENSION_INDEX").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_INDEX.to_string())
}

/// Read the index from a file or download it (with curl, which Windows 10+, macOS and Linux
/// have).
pub fn fetch_index(location: &str) -> Result<Vec<IndexEntry>, String> {
    let text = if location.starts_with("http://") || location.starts_with("https://") {
        let mut cmd = Command::new("curl");
        cmd.args(["-fsSL", "--max-time", "30", location]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().map_err(|e| format!("cannot run curl to download the extension index: {e}"))?;
        if !out.status.success() {
            return Err(format!("cannot download {location}: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        std::fs::read_to_string(location).map_err(|e| format!("{location}: {e}"))?
    };
    parse_index(&text)
}
