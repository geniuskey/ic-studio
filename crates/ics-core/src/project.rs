//! Project / library / cell / view management and on-disk storage.
//!
//! On-disk layout (human readable JSON, friendly to version control):
//! ```text
//! <root>/ic-studio.json            project manifest
//! <root>/tech.json                 technology (layers, rules)
//! <root>/<lib>/<cell>/cell.json    cell metadata
//! <root>/<lib>/<cell>/schematic.json
//! <root>/<lib>/<cell>/layout.json
//! <root>/<lib>/<cell>/sim.json
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::Layout;
use crate::schematic::Schematic;
use crate::sim_setup::SimSetup;
use crate::tech::Tech;

pub const MANIFEST: &str = "ic-studio.json";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("I/O error on {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid file {path}: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ProjectError>;

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io { path: path.to_path_buf(), source }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(io_err(path))?;
    serde_json::from_str(&text).map_err(|source| ProjectError::Parse { path: path.to_path_buf(), source })
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(v).expect("serializable");
    // Write atomically so a crash never leaves a truncated file behind.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(io_err(&tmp))?;
    fs::rename(&tmp, path).map_err(io_err(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewKind {
    Schematic,
    Layout,
    Simulation,
}

impl ViewKind {
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Schematic => "schematic",
            ViewKind::Layout => "layout",
            ViewKind::Simulation => "simulation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CellRef {
    pub lib: String,
    pub cell: String,
}

impl CellRef {
    pub fn new(lib: impl Into<String>, cell: impl Into<String>) -> Self {
        Self { lib: lib.into(), cell: cell.into() }
    }
}

impl std::fmt::Display for CellRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.lib, self.cell)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    /// Path to the ngspice executable (defaults to `ngspice` on PATH).
    #[serde(default)]
    pub ngspice_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    name: String,
    format_version: u32,
    #[serde(default)]
    description: String,
    libraries: Vec<String>,
    #[serde(default)]
    settings: ProjectSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CellMeta {
    #[serde(default)]
    description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub name: String,
    pub description: String,
    pub schematic: Option<Schematic>,
    pub layout: Option<Layout>,
    pub sim: SimSetup,
}

impl Cell {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), description: String::new(), schematic: None, layout: None, sim: SimSetup::default() }
    }

    pub fn has_view(&self, v: ViewKind) -> bool {
        match v {
            ViewKind::Schematic => self.schematic.is_some(),
            ViewKind::Layout => self.layout.is_some(),
            ViewKind::Simulation => self.schematic.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    pub name: String,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone)]
pub struct Project {
    /// Project directory. `None` for a project that was never saved.
    pub root: Option<PathBuf>,
    pub name: String,
    pub description: String,
    pub tech: Tech,
    pub libraries: Vec<Library>,
    pub settings: ProjectSettings,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !name.starts_with('-')
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            root: None,
            name: name.into(),
            description: String::new(),
            tech: Tech::generic(),
            libraries: vec![Library { name: "work".into(), cells: vec![] }],
            settings: ProjectSettings::default(),
        }
    }

    pub fn cell(&self, r: &CellRef) -> Option<&Cell> {
        self.libraries.iter().find(|l| l.name == r.lib)?.cells.iter().find(|c| c.name == r.cell)
    }

    pub fn cell_mut(&mut self, r: &CellRef) -> Option<&mut Cell> {
        self.libraries.iter_mut().find(|l| l.name == r.lib)?.cells.iter_mut().find(|c| c.name == r.cell)
    }

    pub fn library_mut(&mut self, name: &str) -> Option<&mut Library> {
        self.libraries.iter_mut().find(|l| l.name == name)
    }

    pub fn all_cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.libraries.iter().flat_map(|l| l.cells.iter().map(move |c| (CellRef::new(&l.name, &c.name), c)))
    }

    pub fn add_library(&mut self, name: &str) -> std::result::Result<(), String> {
        if !valid_name(name) {
            return Err(format!("'{name}' is not a valid library name (use letters, digits, _ and -)"));
        }
        if self.libraries.iter().any(|l| l.name == name) {
            return Err(format!("library '{name}' already exists"));
        }
        self.libraries.push(Library { name: name.into(), cells: vec![] });
        Ok(())
    }

    pub fn add_cell(&mut self, lib: &str, cell: Cell) -> std::result::Result<CellRef, String> {
        if !valid_name(&cell.name) {
            return Err(format!("'{}' is not a valid cell name (use letters, digits, _ and -)", cell.name));
        }
        let l = self.library_mut(lib).ok_or_else(|| format!("no library '{lib}'"))?;
        if l.cells.iter().any(|c| c.name == cell.name) {
            return Err(format!("cell '{}' already exists in '{lib}'", cell.name));
        }
        let r = CellRef::new(lib, &cell.name);
        l.cells.push(cell);
        l.cells.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(r)
    }

    /// Delete a cell from memory and (if saved) from disk.
    pub fn delete_cell(&mut self, r: &CellRef) -> Result<()> {
        if let Some(l) = self.library_mut(&r.lib) {
            l.cells.retain(|c| c.name != r.cell);
        }
        if let Some(root) = &self.root {
            let dir = root.join(&r.lib).join(&r.cell);
            if dir.join("cell.json").exists() {
                fs::remove_dir_all(&dir).map_err(io_err(&dir))?;
            }
        }
        Ok(())
    }

    pub fn rename_cell(&mut self, r: &CellRef, new_name: &str) -> std::result::Result<CellRef, String> {
        if !valid_name(new_name) {
            return Err(format!("'{new_name}' is not a valid cell name"));
        }
        if self.cell(&CellRef::new(&r.lib, new_name)).is_some() {
            return Err(format!("cell '{new_name}' already exists"));
        }
        if let Some(root) = &self.root {
            let from = root.join(&r.lib).join(&r.cell);
            if from.exists() {
                fs::rename(&from, root.join(&r.lib).join(new_name)).map_err(|e| e.to_string())?;
            }
        }
        let c = self.cell_mut(r).ok_or("no such cell")?;
        c.name = new_name.to_string();
        Ok(CellRef::new(&r.lib, new_name))
    }

    pub fn save_as(&mut self, root: &Path) -> Result<()> {
        self.root = Some(root.to_path_buf());
        self.save()
    }

    pub fn save(&self) -> Result<()> {
        let root = self.root.as_ref().ok_or_else(|| ProjectError::Other("project has no location".into()))?;
        fs::create_dir_all(root).map_err(io_err(root))?;
        let manifest = Manifest {
            name: self.name.clone(),
            format_version: FORMAT_VERSION,
            description: self.description.clone(),
            libraries: self.libraries.iter().map(|l| l.name.clone()).collect(),
            settings: self.settings.clone(),
        };
        write_json(&root.join(MANIFEST), &manifest)?;
        write_json(&root.join("tech.json"), &self.tech)?;
        for lib in &self.libraries {
            for cell in &lib.cells {
                let dir = root.join(&lib.name).join(&cell.name);
                fs::create_dir_all(&dir).map_err(io_err(&dir))?;
                write_json(&dir.join("cell.json"), &CellMeta { description: cell.description.clone() })?;
                write_json(&dir.join("sim.json"), &cell.sim)?;
                sync_view(&dir.join("schematic.json"), cell.schematic.as_ref())?;
                sync_view(&dir.join("layout.json"), cell.layout.as_ref())?;
            }
        }
        Ok(())
    }

    pub fn open(root: &Path) -> Result<Project> {
        let root = if root.is_file() { root.parent().unwrap_or(root).to_path_buf() } else { root.to_path_buf() };
        let manifest: Manifest = read_json(&root.join(MANIFEST))?;
        if manifest.format_version > FORMAT_VERSION {
            return Err(ProjectError::Other(format!(
                "project format v{} is newer than this IC Studio (v{FORMAT_VERSION})",
                manifest.format_version
            )));
        }
        let tech_path = root.join("tech.json");
        let tech = if tech_path.exists() { read_json(&tech_path)? } else { Tech::generic() };
        let mut libraries = vec![];
        for lib_name in &manifest.libraries {
            let lib_dir = root.join(lib_name);
            let mut cells = vec![];
            if let Ok(rd) = fs::read_dir(&lib_dir) {
                for e in rd.flatten() {
                    let dir = e.path();
                    if !dir.join("cell.json").exists() {
                        continue;
                    }
                    let name = e.file_name().to_string_lossy().to_string();
                    let meta: CellMeta = read_json(&dir.join("cell.json"))?;
                    let opt = |f: &str| -> Result<Option<PathBuf>> {
                        let p = dir.join(f);
                        Ok(p.exists().then_some(p))
                    };
                    let schematic = opt("schematic.json")?.map(|p| read_json(&p)).transpose()?;
                    let layout = opt("layout.json")?.map(|p| read_json(&p)).transpose()?;
                    let sim = opt("sim.json")?.map(|p| read_json(&p)).transpose()?.unwrap_or_default();
                    cells.push(Cell { name, description: meta.description, schematic, layout, sim });
                }
            }
            cells.sort_by(|a, b| a.name.cmp(&b.name));
            libraries.push(Library { name: lib_name.clone(), cells });
        }
        Ok(Project {
            root: Some(root),
            name: manifest.name,
            description: manifest.description,
            tech,
            libraries,
            settings: manifest.settings,
        })
    }
}

fn sync_view<T: Serialize>(path: &Path, view: Option<&T>) -> Result<()> {
    match view {
        Some(v) => write_json(path, v),
        None => {
            if path.exists() {
                fs::remove_file(path).map_err(io_err(path))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_reopen() {
        let dir = std::env::temp_dir().join(format!("ics-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut p = crate::examples::demo_project();
        p.save_as(&dir).unwrap();
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.name, p.name);
        assert_eq!(q.libraries, p.libraries);
        let r = q.all_cells().next().unwrap().0;
        let mut q = q;
        q.delete_cell(&r).unwrap();
        assert!(!dir.join(&r.lib).join(&r.cell).exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
