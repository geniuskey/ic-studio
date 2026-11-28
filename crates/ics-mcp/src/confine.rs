//! Folder confinement of the MCP server: file paths in tool arguments must lie inside the
//! folders it may use, the server's working directory unless `ICS_MCP_ROOTS` lists others (in
//! the platform's PATH syntax). Relative paths are made absolute against the server's working
//! directory, so a call forwarded to the desktop app means the same file.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

/// Tool arguments that name a file or directory.
pub const PATH_ARGS: [&str; 8] = ["path", "file", "flow_path", "svg_path", "spec_path", "lib_path", "lef_path", "layermap"];

/// Arguments holding SPICE text, whose `.include` / `.lib` files must lie inside the roots too
/// (relative ones are taken from the project, as the simulation tools do).
pub const DECK_ARGS: [&str; 2] = ["deck", "extra"];

/// Arguments resolved against the open project's directory when relative.
const PROJECT_RELATIVE: [&str; 1] = ["data"];

#[derive(Debug, Clone)]
pub struct Roots {
    base: PathBuf,
    roots: Vec<PathBuf>,
}

impl Roots {
    /// `ICS_MCP_ROOTS` when set, else the working directory.
    pub fn from_env() -> Self {
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let listed: Vec<PathBuf> = std::env::var_os("ICS_MCP_ROOTS")
            .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        let roots = if listed.is_empty() { vec![base.clone()] } else { listed };
        Self::new(base, roots)
    }

    /// For an agent inside the desktop app: `ICS_MCP_ROOTS` when set, else the open project's
    /// directory, against which relative paths resolve.
    pub fn for_project(dir: Option<&Path>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let base = dir.map_or(cwd, Path::to_path_buf);
        let listed: Vec<PathBuf> = std::env::var_os("ICS_MCP_ROOTS")
            .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        let roots = if listed.is_empty() { dir.map(Path::to_path_buf).into_iter().collect() } else { listed };
        Self::new(base, roots)
    }

    pub fn new(base: PathBuf, roots: Vec<PathBuf>) -> Self {
        let roots = roots.iter().map(|r| canonical(&absolute(&base, r))).collect();
        Self { base, roots }
    }

    /// Whether `p` (absolute) is inside one of the roots.
    fn allows(&self, p: &Path) -> bool {
        self.roots.iter().any(|r| p.starts_with(r))
    }

    /// `args` with every path made absolute, or why a path is refused. `project`: the open
    /// project's directory, also allowed.
    pub fn check(&self, args: &Map<String, Value>, project: Option<&Path>) -> Result<Map<String, Value>, String> {
        let mut out = args.clone();
        for (k, v) in out.iter_mut() {
            let Value::String(s) = v else { continue };
            if DECK_ARGS.contains(&k.as_str()) {
                self.check_deck(k, s, project)?;
                continue;
            }
            if !PATH_ARGS.contains(&k.as_str()) && !PROJECT_RELATIVE.contains(&k.as_str()) {
                continue;
            }
            let p = Path::new(s.as_str());
            #[cfg(windows)]
            if !plain_windows_path(p) {
                return Err(format!(
                    "argument '{k}': unsupported Windows path '{s}'; use a local drive or relative path without '..'"
                ));
            }
            // On Windows `\x` and `C:x` are "relative" yet leave the base when joined.
            let relative_to_project = PROJECT_RELATIVE.contains(&k.as_str())
                && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir | Component::ParentDir));
            if relative_to_project {
                if p.components().any(|c| matches!(c, Component::ParentDir)) {
                    return Err(format!("argument '{k}': '{s}' may not leave the project directory"));
                }
                continue;
            }
            let abs = canonical(&absolute(&self.base, p));
            let in_project = project.is_some_and(|d| abs.starts_with(canonical(&absolute(&self.base, d))));
            if !self.allows(&abs) && !in_project {
                let roots: Vec<String> = self.roots.iter().map(|r| r.display().to_string()).collect();
                let roots = if roots.is_empty() { vec!["none: no project is open".to_string()] } else { roots };
                return Err(format!(
                    "argument '{k}': '{s}' is outside the folders this MCP server may use ({}); start it from the \
                     design folder or list more folders in ICS_MCP_ROOTS",
                    roots.join(", ")
                ));
            }
            *v = Value::String(abs.to_string_lossy().into_owned());
        }
        Ok(out)
    }

    /// SPICE text in argument `k`: the files it reads must be allowed, and it may not load
    /// compiled models (native code in ngspice).
    fn check_deck(&self, k: &str, deck: &str, project: Option<&Path>) -> Result<(), String> {
        let dir = project.unwrap_or(&self.base);
        let found = ics_sim::frontend::deck_files(deck, Some(dir));
        if let Some(l) = found.compiled.first() {
            return Err(format!(
                "argument '{k}': '{l}' loads compiled models, which agents may not do; put the model in the \
                 technology's model files instead"
            ));
        }
        let project = project.map(|d| canonical(&absolute(&self.base, d)));
        for f in found.files {
            let abs = canonical(&absolute(dir, &f));
            if !self.allows(&abs) && !project.as_ref().is_some_and(|d| abs.starts_with(d)) {
                return Err(format!(
                    "argument '{k}': the deck reads '{}', outside the folders this MCP server may use; list its \
                     folder in ICS_MCP_ROOTS",
                    f.display()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn plain_windows_path(path: &Path) -> bool {
    use std::path::Prefix;

    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) && path.is_absolute() => {}
        Some(Component::Prefix(_) | Component::RootDir | Component::ParentDir) => return false,
        _ => {}
    }
    !components.any(|component| component == Component::ParentDir)
}

fn absolute(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
}

/// `p` with its longest existing ancestor canonicalised (symbolic links resolved) and `..` in
/// the rest folded lexically.
fn canonical(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    // `file_name()` is None for a trailing `..`, which Unix cannot resolve below a missing
    // folder: keep it in `rest` so it is folded instead of left in a path that only looks inside.
    while !existing.exists() {
        let last = match existing.components().next_back() {
            Some(Component::Normal(n)) => n.to_os_string(),
            Some(Component::ParentDir) => "..".into(),
            Some(Component::CurDir) => ".".into(),
            _ => break,
        };
        let Some(parent) = existing.parent() else { break };
        rest.push(last);
        existing = parent.to_path_buf();
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for n in rest.into_iter().rev() {
        match Path::new(&n).components().next() {
            Some(Component::ParentDir) => {
                out.pop();
            }
            Some(Component::CurDir) => {}
            _ => out.push(n),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn paths_must_stay_inside_the_roots() {
        let dir = ics_core::tmp::private_dir("ics-confine").unwrap();
        let root = dir.join("work");
        std::fs::create_dir_all(&root).unwrap();
        let r = Roots::new(root.clone(), vec![root.clone()]);
        let ok = r.check(&args(json!({"path": "chip/top.gds", "cell": "top"})), None).unwrap();
        assert_eq!(ok["path"], json!(root.canonicalize().unwrap().join("chip/top.gds").to_string_lossy()));
        assert_eq!(ok["cell"], json!("top"), "other arguments are left alone");
        for bad in ["../x.gds", "/etc/passwd", "chip/../../x"] {
            assert!(r.check(&args(json!({"path": bad})), None).is_err(), "{bad}");
        }
        assert!(r.check(&args(json!({"lef_path": dir.join("x.lef")})), None).is_err());
        // The open project's directory is allowed too; `data` is relative to it.
        let proj = dir.join("proj");
        assert!(r.check(&args(json!({"path": proj.join("a.gds")})), Some(&proj)).is_ok());
        assert!(r.check(&args(json!({"data": "meas/iv.csv"})), None).is_ok());
        assert!(r.check(&args(json!({"data": "../iv.csv"})), None).is_err());
        assert!(r.check(&args(json!({"data": "/etc/passwd"})), None).is_err());
        #[cfg(windows)]
        {
            assert!(r.check(&args(json!({"data": "C:x.csv"})), None).is_err());
            let verbatim = root.canonicalize().unwrap();
            let local = verbatim.to_str().unwrap().strip_prefix(r"\\?\").unwrap();
            assert!(r.check(&args(json!({"path": local.to_ascii_lowercase().replace('\\', "/")})), None).is_ok());
            for bad in [verbatim.to_str().unwrap(), r"\\localhost\share\file", r"\rooted", "chip/../chip"] {
                assert!(r.check(&args(json!({"path": bad})), None).is_err(), "{bad}");
            }
        }
        // Decks: included files must be inside too, including through another include.
        std::fs::write(root.join("ok.spi"), "R1 a 0 1k\n").unwrap();
        std::fs::write(root.join("hop.spi"), format!(".include '{}'\n", dir.join("secret.txt").display())).unwrap();
        assert!(r.check(&args(json!({"deck": "* t\n.include ok.spi\n.op\n.end\n"})), None).is_ok());
        for bad in ["* t\n.include /etc/passwd\n.end\n", "* t\n.inc hop.spi\n.end\n", "* t\n.lib ../x.lib tt\n.end\n"] {
            assert!(r.check(&args(json!({"deck": bad})), None).is_err(), "{bad}");
        }
        assert!(r.check(&args(json!({"extra": ".include /etc/passwd"})), None).is_err());
        let osdi = "* t\n.control\npre_osdi ok.osdi\n.endc\n.end\n";
        assert!(r.check(&args(json!({"deck": osdi})), None).unwrap_err().contains("compiled"));
        #[cfg(unix)]
        {
            // A link inside a root that points out of it is outside.
            std::os::unix::fs::symlink(&dir, root.join("up")).unwrap();
            assert!(r.check(&args(json!({"path": "up/x.gds"})), None).is_err());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
