//! Folder confinement of the MCP server: file paths in tool arguments must lie inside the
//! folders it may use, the server's working directory unless `ICS_MCP_ROOTS` lists others (in
//! the platform's PATH syntax). Relative paths are made absolute against the server's working
//! directory, so a call forwarded to the desktop app means the same file.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

/// Tool arguments that name a file or directory.
pub const PATH_ARGS: [&str; 8] = ["path", "file", "flow_path", "svg_path", "spec_path", "lib_path", "lef_path", "layermap"];

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
            let p = Path::new(s.as_str());
            let relative_to_project = PROJECT_RELATIVE.contains(&k.as_str()) && p.is_relative();
            if !PATH_ARGS.contains(&k.as_str()) && !PROJECT_RELATIVE.contains(&k.as_str()) {
                continue;
            }
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
}

fn absolute(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
}

/// `p` with its longest existing ancestor canonicalised (symbolic links resolved) and `..` in
/// the rest folded lexically.
fn canonical(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(n), Some(parent)) => {
                rest.push(n.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
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
        #[cfg(unix)]
        {
            // A link inside a root that points out of it is outside.
            std::os::unix::fs::symlink(&dir, root.join("up")).unwrap();
            assert!(r.check(&args(json!({"path": "up/x.gds"})), None).is_err());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
