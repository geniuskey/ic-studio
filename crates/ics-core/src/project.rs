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
//!
//! A library with [`LibStorage::Gds`] is stored instead as
//! ```text
//! <root>/<lib>/<lib>.gds           every layout of the library (see `gds::write_gds`)
//! <root>/<lib>/library.json        per-cell data GDS cannot hold: description, PCell
//!                                  parameters, schematic and simulation setup
//! ```
//! A cell listed in `library.json` without a structure in the GDS file has no layout.
//! References to cells of other libraries are written by plain cell name (`lib__cell` if the
//! file has a structure of that name); on open they resolve to the first library in manifest
//! order holding such a cell.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gds::{GdsImport, ReadOptions, read_gds_with, sanitize_name, unique_name, write_gds};
use crate::layout::{Geometry, Layout};
use crate::pcell::{PCELL_LIB, PCell};
use crate::schematic::Schematic;
use crate::sim_setup::SimSetup;
use crate::tech::{LayerDef, Tech};

pub const MANIFEST: &str = "ic-studio.json";
pub const FORMAT_VERSION: u32 = 1;
const LIBRARY_FILE: &str = "library.json";

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
    write_atomic(path, serde_json::to_string_pretty(v).expect("serializable").as_bytes())
}

/// Write via a temporary file so a crash never leaves a truncated file behind.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data).map_err(io_err(&tmp))?;
    fs::rename(&tmp, path).map_err(io_err(path))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_err(path)(e)),
        _ => Ok(()),
    }
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
    libraries: Vec<LibEntry>,
    #[serde(default)]
    settings: ProjectSettings,
}

/// A manifest library entry: a plain name for JSON storage, an object otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum LibEntry {
    Name(String),
    Spec {
        name: String,
        #[serde(default)]
        storage: LibStorage,
    },
}

impl LibEntry {
    fn of(l: &Library) -> Self {
        match l.storage {
            LibStorage::Json => LibEntry::Name(l.name.clone()),
            storage => LibEntry::Spec { name: l.name.clone(), storage },
        }
    }

    fn parts(&self) -> (&str, LibStorage) {
        match self {
            LibEntry::Name(n) => (n, LibStorage::Json),
            LibEntry::Spec { name, storage } => (name, *storage),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CellMeta {
    #[serde(default)]
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pcell: Option<PCell>,
}

/// `library.json` of a GDS-stored library.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GdsLibraryFile {
    #[serde(default)]
    cells: BTreeMap<String, GdsCellMeta>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GdsCellMeta {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pcell: Option<PCell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schematic: Option<Schematic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sim: Option<SimSetup>,
}

/// How a library is stored on disk (see the module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibStorage {
    /// A directory of JSON files per cell.
    #[default]
    Json,
    /// One GDSII file with every layout, plus `library.json`.
    Gds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub name: String,
    pub description: String,
    pub schematic: Option<Schematic>,
    pub layout: Option<Layout>,
    pub sim: SimSetup,
    /// Set for generated PCell variants.
    pub pcell: Option<PCell>,
}

impl Cell {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            schematic: None,
            layout: None,
            sim: SimSetup::default(),
            pcell: None,
        }
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
    pub storage: LibStorage,
}

impl Library {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), cells: vec![], storage: LibStorage::Json }
    }
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
            libraries: vec![Library::new("work")],
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
        self.libraries.push(Library::new(name));
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

    /// The cell holding PCell variant `p` in [`PCELL_LIB`], generating it on first use.
    pub fn ensure_pcell(&mut self, p: &PCell) -> std::result::Result<CellRef, String> {
        let r = CellRef::new(PCELL_LIB, p.variant_name());
        if self.cell(&r).is_some() {
            return Ok(r);
        }
        let layout = p.generate(&self.tech)?;
        if self.library_mut(PCELL_LIB).is_none() {
            self.add_library(PCELL_LIB)?;
        }
        let cell = Cell {
            description: p.summary(self.tech.dbu_per_um),
            layout: Some(layout),
            pcell: Some(p.clone()),
            ..Cell::new(&r.cell)
        };
        self.add_cell(PCELL_LIB, cell)
    }

    /// Add the cells of a GDS import as a new library named after `name_hint` (made valid and
    /// unique). The import's `new_layers` are appended to the technology unless a layer with
    /// the same GDS numbers exists. The library is stored as GDS ([`LibStorage::Gds`]).
    /// Returns the library name and the top cells of the import.
    pub fn add_gds_library(
        &mut self,
        name_hint: &str,
        mut imp: GdsImport,
    ) -> std::result::Result<(String, Vec<CellRef>), String> {
        if imp.cells.is_empty() {
            return Err("the GDS file contains no cells".into());
        }
        merge_new_layers(&mut self.tech, &mut imp);
        let mut used: HashSet<String> = self.libraries.iter().map(|l| l.name.to_ascii_lowercase()).collect();
        let lib = unique_name(&sanitize_name(name_hint), &mut used);
        imp.sanitize_names(&lib);
        let tops = imp.top_cells().into_iter().map(|c| CellRef::new(&lib, c)).collect();
        let mut cells: Vec<Cell> = imp
            .cells
            .into_iter()
            .map(|(name, layout)| Cell { description: "Imported from GDS".into(), layout: Some(layout), ..Cell::new(name) })
            .collect();
        cells.sort_by(|a, b| a.name.cmp(&b.name));
        self.libraries.push(Library { name: lib.clone(), cells, storage: LibStorage::Gds });
        Ok((lib, tops))
    }

    /// Change how library `lib` is stored; the old files are removed by the next [`Project::save`].
    pub fn set_library_storage(&mut self, lib: &str, storage: LibStorage) -> std::result::Result<(), String> {
        let l = self.library_mut(lib).ok_or_else(|| format!("no library '{lib}'"))?;
        if storage == LibStorage::Json {
            let mut seen = HashSet::new();
            if let Some(c) = l.cells.iter().find(|c| !seen.insert(c.name.to_ascii_lowercase())) {
                return Err(format!("cell names differing only in case (e.g. '{}') cannot be stored as directories", c.name));
            }
        }
        l.storage = storage;
        Ok(())
    }

    /// Replace the technology, moving every layout shape to the new layer with the same
    /// (GDS layer, datatype), else the same name. Layers in use without a match are appended
    /// to the new technology so no shape is lost. When the database unit changes, all layout
    /// coordinates are rescaled to keep their physical size; otherwise PCell variants are
    /// regenerated with the new technology. Returns warnings.
    pub fn set_tech(&mut self, mut tech: Tech) -> std::result::Result<Vec<String>, String> {
        if tech.dbu_per_um <= 0 {
            return Err(format!("invalid database unit: {} dbu per µm", tech.dbu_per_um));
        }
        let mut used: Vec<bool> = vec![false; self.tech.layers.len()];
        let mut undefined = 0usize;
        for l in self.libraries.iter().flat_map(|l| &l.cells).filter_map(|c| c.layout.as_ref()) {
            for s in &l.shapes {
                match used.get_mut(s.layer) {
                    Some(u) => *u = true,
                    None => undefined += 1,
                }
            }
        }
        let mut warnings = vec![];
        let mut map: Vec<Option<usize>> = vec![None; used.len()];
        for (i, old) in self.tech.layers.iter().enumerate().filter(|&(i, _)| used[i]) {
            let to = match tech.layer_by_gds(old.gds_layer, old.gds_datatype) {
                Some(to) => to,
                None => match tech.layer_index(&old.name) {
                    Some(to) => {
                        let n = &tech.layers[to];
                        warnings.push(format!(
                            "layer {} matched by name: GDS {}/{} becomes {}/{}",
                            old.name, old.gds_layer, old.gds_datatype, n.gds_layer, n.gds_datatype
                        ));
                        to
                    }
                    None => {
                        warnings.push(format!(
                            "layer {} ({}/{}) is not in the new technology and was added to it",
                            old.name, old.gds_layer, old.gds_datatype
                        ));
                        push_layer(&mut tech, old.clone())
                    }
                },
            };
            map[i] = Some(to);
        }
        if tech.layers.is_empty() {
            return Err("the technology has no layers".into());
        }
        if undefined > 0 {
            warnings.push(format!("{undefined} shape(s) on undefined layers were removed"));
        }
        let scale = (tech.dbu_per_um != self.tech.dbu_per_um).then(|| tech.dbu_per_um as f64 / self.tech.dbu_per_um as f64);
        if let Some(f) = scale {
            warnings.push(format!(
                "database unit changed from {} to {} per µm: layout coordinates were scaled by {f}",
                self.tech.dbu_per_um, tech.dbu_per_um
            ));
        }
        for l in self.libraries.iter_mut().flat_map(|l| &mut l.cells).filter_map(|c| c.layout.as_mut()) {
            l.shapes.retain(|s| s.layer < map.len());
            for s in &mut l.shapes {
                s.layer = map[s.layer].expect("used layers are mapped");
            }
            if let Some(f) = scale {
                scale_layout(l, f);
            }
        }
        self.tech = tech;
        // PCell parameters are in database units, so variants are only regenerated for the new
        // technology when the unit is kept.
        if let Some(lib) = self.libraries.iter_mut().find(|l| l.name == PCELL_LIB).filter(|_| scale.is_none()) {
            for c in &mut lib.cells {
                let Some(pc) = &c.pcell else { continue };
                match pc.generate(&self.tech) {
                    Ok(lay) => {
                        c.layout = Some(lay);
                        c.description = pc.summary(self.tech.dbu_per_um);
                    }
                    Err(e) => warnings.push(format!("PCell {} kept its old layout: {e}", c.name)),
                }
            }
        }
        Ok(warnings)
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
        let to = CellRef::new(&r.lib, new_name);
        for lib in &mut self.libraries {
            for c in &mut lib.cells {
                if let Some(l) = c.layout.as_mut() {
                    l.rename_refs(r, &to);
                }
            }
        }
        Ok(to)
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
            libraries: self.libraries.iter().map(LibEntry::of).collect(),
            settings: self.settings.clone(),
        };
        write_json(&root.join(MANIFEST), &manifest)?;
        write_json(&root.join("tech.json"), &self.tech)?;
        for lib in &self.libraries {
            let lib_dir = root.join(&lib.name);
            match lib.storage {
                LibStorage::Json => {
                    for cell in &lib.cells {
                        let dir = lib_dir.join(&cell.name);
                        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
                        write_json(
                            &dir.join("cell.json"),
                            &CellMeta { description: cell.description.clone(), pcell: cell.pcell.clone() },
                        )?;
                        write_json(&dir.join("sim.json"), &cell.sim)?;
                        sync_view(&dir.join("schematic.json"), cell.schematic.as_ref())?;
                        sync_view(&dir.join("layout.json"), cell.layout.as_ref())?;
                    }
                    remove_file_if_exists(&gds_file(&lib_dir, &lib.name))?;
                    remove_file_if_exists(&lib_dir.join(LIBRARY_FILE))?;
                }
                LibStorage::Gds => save_gds_library(&lib_dir, lib, &self.tech)?,
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
        let mut tech = if tech_path.exists() { read_json(&tech_path)? } else { Tech::generic() };
        let mut libraries = vec![];
        for entry in &manifest.libraries {
            let (lib_name, storage) = entry.parts();
            let lib_dir = root.join(lib_name);
            if storage == LibStorage::Gds {
                let cells = open_gds_library(&lib_dir, lib_name, &mut tech)?;
                libraries.push(Library { name: lib_name.into(), cells, storage });
                continue;
            }
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
                    cells.push(Cell { name, description: meta.description, schematic, layout, sim, pcell: meta.pcell });
                }
            }
            cells.sort_by(|a, b| a.name.cmp(&b.name));
            libraries.push(Library { name: lib_name.into(), cells, storage });
        }
        resolve_external_refs(&mut libraries);
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

fn gds_file(lib_dir: &Path, lib: &str) -> PathBuf {
    lib_dir.join(format!("{lib}.gds"))
}

fn save_gds_library(dir: &Path, lib: &Library, tech: &Tech) -> Result<()> {
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    let refs: Vec<CellRef> = lib.cells.iter().filter(|c| c.layout.is_some()).map(|c| CellRef::new(&lib.name, &c.name)).collect();
    let cells: Vec<(&CellRef, &Layout)> = refs.iter().zip(lib.cells.iter().filter_map(|c| c.layout.as_ref())).collect();
    write_atomic(&gds_file(dir, &lib.name), &write_gds(&lib.name, &cells, tech))?;
    let default_sim = SimSetup::default();
    let file = GdsLibraryFile {
        cells: lib
            .cells
            .iter()
            .map(|c| {
                let meta = GdsCellMeta {
                    description: c.description.clone(),
                    pcell: c.pcell.clone(),
                    schematic: c.schematic.clone(),
                    sim: (c.sim != default_sim).then(|| c.sim.clone()),
                };
                (c, meta)
            })
            .filter(|(c, m)| {
                c.layout.is_none() || !m.description.is_empty() || m.pcell.is_some() || m.schematic.is_some() || m.sim.is_some()
            })
            .map(|(c, m)| (c.name.clone(), m))
            .collect(),
    };
    write_json(&dir.join(LIBRARY_FILE), &file)?;
    for e in fs::read_dir(dir).map_err(io_err(dir))?.flatten() {
        let p = e.path();
        if p.join("cell.json").is_file() {
            fs::remove_dir_all(&p).map_err(io_err(&p))?;
        }
    }
    Ok(())
}

/// Cells of a GDS-stored library. Layers the technology lacks are appended to `tech`;
/// references to cells missing from the file are left for [`resolve_external_refs`].
fn open_gds_library(dir: &Path, lib: &str, tech: &mut Tech) -> Result<Vec<Cell>> {
    let path = gds_file(dir, lib);
    let data = fs::read(&path).map_err(io_err(&path))?;
    let mut imp = read_gds_with(&data, tech, lib, ReadOptions { add_unknown_layers: true, ..Default::default() })
        .map_err(|e| ProjectError::Other(format!("{}: {e}", path.display())))?;
    drop(data);
    merge_new_layers(tech, &mut imp);
    let meta_path = dir.join(LIBRARY_FILE);
    let mut meta: GdsLibraryFile = if meta_path.exists() { read_json(&meta_path)? } else { GdsLibraryFile::default() };
    let mut used: HashSet<String> = imp.cells.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
    let renamed: HashMap<String, String> = imp
        .cells
        .iter()
        .filter(|(n, _)| !valid_name(n))
        .map(|(n, _)| (n.clone(), unique_name(&sanitize_name(n), &mut used)))
        .collect();
    let mut cells = Vec::with_capacity(imp.cells.len() + meta.cells.len());
    for (name, mut layout) in imp.cells {
        if !renamed.is_empty() {
            for i in &mut layout.instances {
                if let Some(n) = renamed.get(&i.cell.cell) {
                    i.cell.cell = n.clone();
                }
            }
        }
        let name = renamed.get(&name).cloned().unwrap_or(name);
        let m = meta.cells.remove(&name).unwrap_or_default();
        cells.push(Cell {
            name,
            description: m.description,
            schematic: m.schematic,
            layout: Some(layout),
            sim: m.sim.unwrap_or_default(),
            pcell: m.pcell,
        });
    }
    for (name, m) in meta.cells {
        cells.push(Cell {
            description: m.description,
            schematic: m.schematic,
            sim: m.sim.unwrap_or_default(),
            pcell: m.pcell,
            ..Cell::new(name)
        });
    }
    cells.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(cells)
}

/// Point references a GDS-stored library makes to cells it does not hold at the first library
/// with a cell of that name, or at `lib/cell` for a `lib__cell` reference.
fn resolve_external_refs(libraries: &mut [Library]) {
    let mut owner: HashMap<String, String> = HashMap::new();
    let mut all: HashSet<(String, String)> = HashSet::new();
    for l in libraries.iter() {
        for c in &l.cells {
            owner.entry(c.name.clone()).or_insert_with(|| l.name.clone());
            all.insert((l.name.clone(), c.name.clone()));
        }
    }
    for l in libraries.iter_mut().filter(|l| l.storage == LibStorage::Gds) {
        let own: HashSet<String> = l.cells.iter().map(|c| c.name.clone()).collect();
        for i in l.cells.iter_mut().filter_map(|c| c.layout.as_mut()).flat_map(|lay| &mut lay.instances) {
            if i.cell.lib != l.name || own.contains(&i.cell.cell) {
                continue;
            }
            if let Some(lib) = owner.get(&i.cell.cell) {
                i.cell.lib = lib.clone();
            } else if let Some((a, b)) = i.cell.cell.split_once("__")
                && all.contains(&(a.to_string(), b.to_string()))
            {
                i.cell = CellRef::new(a, b);
            }
        }
    }
}

/// Append the import's new layers to `tech` (reusing layers with the same GDS numbers) and
/// move the shapes on them to their final indices.
fn merge_new_layers(tech: &mut Tech, imp: &mut GdsImport) {
    if imp.new_layers.is_empty() {
        return;
    }
    let base = imp.new_layer_base;
    let new_layers: Vec<usize> = std::mem::take(&mut imp.new_layers)
        .into_iter()
        .map(|d| tech.layer_by_gds(d.gds_layer, d.gds_datatype).unwrap_or_else(|| push_layer(tech, d)))
        .collect();
    for (_, l) in &mut imp.cells {
        for s in &mut l.shapes {
            if let Some(&to) = s.layer.checked_sub(base).and_then(|k| new_layers.get(k)) {
                s.layer = to;
            }
        }
    }
}

/// Append `layer` to `tech` (renamed `name_2`, `name_3`, ... if the name is taken); its index.
fn push_layer(tech: &mut Tech, mut layer: LayerDef) -> usize {
    let base = layer.name.clone();
    let mut k = 1;
    while tech.layer_index(&layer.name).is_some() {
        k += 1;
        layer.name = format!("{base}_{k}");
    }
    tech.layers.push(layer);
    tech.layers.len() - 1
}

fn scale_layout(l: &mut Layout, f: f64) {
    let s = |v: &mut i64| *v = (*v as f64 * f).round() as i64;
    let p = |q: &mut (i64, i64)| {
        s(&mut q.0);
        s(&mut q.1);
    };
    for sh in &mut l.shapes {
        match &mut sh.geom {
            Geometry::Rect(r) => {
                for v in [&mut r.x0, &mut r.y0, &mut r.x1, &mut r.y1] {
                    s(v);
                }
            }
            Geometry::Polygon { points } => points.iter_mut().for_each(p),
            Geometry::Path { points, width } => {
                points.iter_mut().for_each(p);
                s(width);
            }
            Geometry::Label { at, size, .. } => {
                p(at);
                s(size);
            }
        }
    }
    for i in &mut l.instances {
        s(&mut i.trans.x);
        s(&mut i.trans.y);
        if let Some(a) = i.array.as_mut() {
            p(&mut a.col);
            p(&mut a.row);
        }
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
    use crate::geom::IRect;

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

    #[test]
    fn ensure_pcell_is_idempotent_and_persists() {
        let mut p = Project::new("pc");
        let nmos = PCell::mos(false, 1000, 180, 2);
        let a = p.ensure_pcell(&nmos).unwrap();
        assert_eq!(a, CellRef::new(PCELL_LIB, "nmos_w1000_l180_nf2"));
        assert_eq!(p.ensure_pcell(&nmos).unwrap(), a);
        let res = p.ensure_pcell(&PCell::PolyRes { w: 400, l: 5000 }).unwrap();
        assert_ne!(res, a);
        let lib = p.libraries.iter().find(|l| l.name == PCELL_LIB).unwrap();
        assert_eq!(lib.cells.len(), 2);
        let c = p.cell(&a).unwrap();
        assert_eq!(c.pcell.as_ref(), Some(&nmos));
        assert_eq!(c.description, "nmos W=1um L=0.18um nf=2");
        assert!(c.layout.as_ref().is_some_and(|l| !l.is_empty()));
        assert!(p.ensure_pcell(&PCell::mos(false, 1000, 180, 0)).is_err());
        assert_eq!(p.libraries.iter().find(|l| l.name == PCELL_LIB).unwrap().cells.len(), 2);

        let dir = std::env::temp_dir().join(format!("ics-pcell-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        p.save_as(&dir).unwrap();
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.libraries, p.libraries);
        assert_eq!(q.tech, p.tech);
        assert_eq!(q.cell(&a).unwrap().pcell.as_ref(), Some(&nmos));
        fs::remove_dir_all(&dir).unwrap();
    }

    fn layer_gds(p: &Project, r: &CellRef) -> Vec<(u16, u16)> {
        let l = p.cell(r).unwrap().layout.as_ref().unwrap();
        l.shapes.iter().map(|s| &p.tech.layers[s.layer]).map(|d| (d.gds_layer, d.gds_datatype)).collect()
    }

    #[test]
    fn set_tech_remaps_layers_by_gds_numbers() {
        let mut p = crate::examples::demo_project();
        let cells: Vec<CellRef> = p.all_cells().filter(|(_, c)| c.layout.is_some()).map(|(r, _)| r).collect();
        assert!(!cells.is_empty());
        let before: Vec<_> = cells.iter().map(|r| layer_gds(&p, r)).collect();
        let shape_count =
            |p: &Project| cells.iter().map(|r| p.cell(r).unwrap().layout.as_ref().unwrap().shapes.len()).sum::<usize>();
        let n_before = shape_count(&p);

        let mut t = Tech::generic();
        t.name = "reordered".into();
        t.layers.reverse();
        let metal1 = t.layer_index("metal1").unwrap();
        t.layers[metal1].name = "met1".into();
        t.layers.retain(|l| l.name != "poly");
        let contact = t.layer_index("contact").unwrap();
        t.layers[contact].gds_layer = 66;
        let warnings = p.set_tech(t).unwrap();
        assert_eq!(p.tech.name, "reordered");
        assert_eq!(shape_count(&p), n_before);
        let poly = p.tech.layer_index("poly").expect("missing used layer is appended");
        assert_eq!(poly, p.tech.layers.len() - 1);
        assert!(warnings.iter().any(|w| w.starts_with("layer poly (5/0) is not in the new technology")), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("contact matched by name: GDS 6/0 becomes 66/0")), "{warnings:?}");
        for (r, b) in cells.iter().zip(&before) {
            let after: Vec<(u16, u16)> = b.iter().map(|&(l, d)| if l == 6 { (66, d) } else { (l, d) }).collect();
            assert_eq!(layer_gds(&p, r), after, "{r}");
        }

        let mut fine = p.tech.clone();
        fine.dbu_per_um = 2000;
        let r = &cells[0];
        let rect = |p: &Project| {
            let l = p.cell(r).unwrap().layout.as_ref().unwrap();
            l.shapes.iter().find_map(|s| if let Geometry::Rect(r) = s.geom { Some(r) } else { None }).unwrap()
        };
        let b0 = rect(&p);
        let warnings = p.set_tech(fine).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(rect(&p), IRect::new(b0.x0 * 2, b0.y0 * 2, b0.x1 * 2, b0.y1 * 2));
        assert!(p.set_tech(Tech { dbu_per_um: 0, ..Tech::generic() }).is_err());
    }

    #[test]
    fn add_gds_library_appends_new_layers() {
        use crate::gds::{ReadOptions, read_gds_with, write_gds};
        let mut p = Project::new("g");
        let n = p.tech.layers.len();
        let mut src_tech = p.tech.clone();
        src_tech.layers.push(LayerDef::generated(99, 0, 0));
        src_tech.layers.push(LayerDef::generated(98, 3, 1));
        let mut l = Layout::default();
        l.add_rect(n, 0, 0, 10, 10);
        l.add_rect(n + 1, 0, 0, 20, 20);
        l.add_rect(0, 0, 0, 30, 30);
        let top = CellRef::new("x", "top");
        let bytes = write_gds("x", &[(&top, &l)], &src_tech);
        let opts = ReadOptions { add_unknown_layers: true, ..Default::default() };

        p.tech.layers.push(LayerDef { name: "L98D3".into(), ..LayerDef::generated(97, 0, 5) });
        let imp = read_gds_with(&bytes, &Tech::generic(), "gds", opts).unwrap();
        assert_eq!(imp.new_layers.len(), 2);
        let (lib, _) = p.add_gds_library("chip", imp).unwrap();
        let names: Vec<&str> = p.tech.layers[n..].iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["L98D3", "L99D0", "L98D3_2"]);
        let r = CellRef::new(&lib, "top");
        assert_eq!(layer_gds(&p, &r), [(99, 0), (98, 3), (1, 0)]);

        let layers = p.tech.layers.len();
        let imp = read_gds_with(&bytes, &p.tech, "gds", opts).unwrap();
        assert!(imp.new_layers.is_empty());
        let (lib2, _) = p.add_gds_library("chip", imp).unwrap();
        assert_eq!(p.tech.layers.len(), layers, "known layers are not added again");
        assert_eq!(layer_gds(&p, &CellRef::new(&lib2, "top")), [(99, 0), (98, 3), (1, 0)]);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ics-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Libraries with layout ids renumbered the way a GDS read assigns them.
    fn canonical(p: &Project) -> Vec<Library> {
        let mut libs = p.libraries.clone();
        for l in libs.iter_mut().flat_map(|l| &mut l.cells).filter_map(|c| c.layout.as_mut()) {
            let mut id = 0;
            for s in &mut l.shapes {
                id += 1;
                s.id = id;
            }
            for i in &mut l.instances {
                id += 1;
                i.id = id;
            }
            l.next_id = id;
        }
        libs
    }

    fn cell_dirs(dir: &Path) -> usize {
        fs::read_dir(dir).map_or(0, |rd| rd.flatten().filter(|e| e.path().join("cell.json").exists()).count())
    }

    fn chip_library(p: &mut Project) {
        use crate::layout::{ArraySpec, Trans};
        p.add_library("other").unwrap();
        let mut other_leaf = Layout::default();
        other_leaf.add_rect(2, 0, 0, 50, 50);
        p.add_cell("other", Cell { layout: Some(other_leaf), ..Cell::new("leaf") }).unwrap();
        p.add_library("chip").unwrap();
        p.set_library_storage("chip", LibStorage::Gds).unwrap();
        let mut leaf = Layout::default();
        leaf.add_rect(6, 0, 0, 1000, 500);
        leaf.add(4, Geometry::Polygon { points: vec![(0, 0), (500, 0), (500, 500), (250, 800), (0, 500)] });
        leaf.add(7, Geometry::Path { points: vec![(0, 0), (2000, 0), (2000, 3000)], width: 280 });
        leaf.add(7, Geometry::Label { text: "out".into(), at: (100, 100), size: 200 });
        p.add_cell("chip", Cell { description: "leaf cell".into(), layout: Some(leaf), ..Cell::new("leaf") }).unwrap();
        let nmos = PCell::mos(false, 1000, 180, 2);
        let dev = p.ensure_pcell(&nmos).unwrap();
        let mut top = Layout::default();
        top.add_rect(1, -100, -100, 5000, 5000);
        for (k, rot) in (0..4u8).enumerate() {
            let id = top.add_instance(CellRef::new("chip", "leaf"), Trans::with(k as i64 * 3000, 0, rot, k % 2 == 1), None);
            top.instance_mut(id).unwrap().name = (k < 2).then(|| format!("X{k}"));
        }
        let arr = Some(ArraySpec { cols: 3, rows: 2, col: (5000, 0), row: (300, 7000) });
        let id = top.add_instance(CellRef::new("chip", "leaf"), Trans::with(0, 9000, 1, true), arr);
        top.instance_mut(id).unwrap().name = Some("XARR".into());
        let id = top.add_instance(dev, Trans::new(0, -5000), None);
        top.instance_mut(id).unwrap().name = Some("M1".into());
        top.add_instance(CellRef::new("other", "leaf"), Trans::new(7, 7), None);
        let src = p.cell(&CellRef::new("work", "cmos_inverter")).unwrap().clone();
        assert!(src.schematic.is_some());
        let mut sim = src.sim.clone();
        sim.temp = 85.0;
        p.add_cell("chip", Cell { schematic: src.schematic.clone(), sim, layout: Some(top), ..Cell::new("top") }).unwrap();
        p.add_cell("chip", Cell { description: "schematic only".into(), schematic: src.schematic, ..Cell::new("sch") }).unwrap();
        let pc = PCell::PolyRes { w: 400, l: 5000 };
        let res = Cell {
            description: pc.summary(p.tech.dbu_per_um),
            layout: Some(pc.generate(&p.tech).unwrap()),
            pcell: Some(pc),
            ..Cell::new("res")
        };
        p.add_cell("chip", res).unwrap();
        p.add_cell("chip", Cell { layout: Some(Layout::default()), ..Cell::new("empty") }).unwrap();
    }

    #[test]
    fn gds_library_round_trip() {
        let dir = temp_dir("gdslib");
        let mut p = crate::examples::demo_project();
        chip_library(&mut p);
        p.set_library_storage("work", LibStorage::Gds).unwrap();
        p.save_as(&dir).unwrap();
        assert!(dir.join("chip").join("chip.gds").is_file());
        assert!(dir.join("chip").join(LIBRARY_FILE).is_file());
        assert_eq!(cell_dirs(&dir.join("chip")), 0);
        assert!(cell_dirs(&dir.join(PCELL_LIB)) > 0, "JSON libraries keep their cell directories");
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.tech, p.tech);
        assert_eq!(canonical(&q), canonical(&p));
        let top = q.cell(&CellRef::new("chip", "top")).unwrap().layout.as_ref().unwrap();
        let refs: Vec<String> = top.instances.iter().map(|i| i.cell.to_string()).collect();
        assert_eq!(refs[5..], ["pcells/nmos_w1000_l180_nf2", "other/leaf"]);
        assert!(q.cell(&CellRef::new("chip", "empty")).unwrap().layout.is_some(), "an empty layout is kept");
        assert!(q.cell(&CellRef::new("chip", "sch")).unwrap().layout.is_none());

        q.save().unwrap();
        assert_eq!(canonical(&Project::open(&dir).unwrap()), canonical(&p));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn json_manifest_is_unchanged_and_gds_entries_are_objects() {
        let dir = temp_dir("manifest");
        let mut p = crate::examples::demo_project();
        p.save_as(&dir).unwrap();
        let manifest = dir.join(MANIFEST);
        let before = fs::read(&manifest).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert!(v["libraries"].as_array().unwrap().iter().all(|l| l.is_string()));
        Project::open(&dir).unwrap().save().unwrap();
        assert_eq!(fs::read(&manifest).unwrap(), before);

        p.set_library_storage(PCELL_LIB, LibStorage::Gds).unwrap();
        p.save().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(v["libraries"], serde_json::json!(["work", { "name": PCELL_LIB, "storage": "gds" }]));
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.libraries.iter().map(|l| l.storage).collect::<Vec<_>>(), [LibStorage::Json, LibStorage::Gds]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_library_storage_replaces_the_old_files() {
        let dir = temp_dir("storage");
        let mut p = crate::examples::demo_project();
        p.save_as(&dir).unwrap();
        let work = dir.join("work");
        let n = p.libraries[0].cells.len();
        assert_eq!(cell_dirs(&work), n);
        p.set_library_storage("work", LibStorage::Gds).unwrap();
        p.save().unwrap();
        assert_eq!(cell_dirs(&work), 0);
        assert!(work.join("work.gds").is_file() && work.join(LIBRARY_FILE).is_file());
        let mut q = Project::open(&dir).unwrap();
        assert_eq!(canonical(&q), canonical(&p));
        q.set_library_storage("work", LibStorage::Json).unwrap();
        q.save().unwrap();
        assert_eq!(cell_dirs(&work), n);
        assert!(!work.join("work.gds").exists() && !work.join(LIBRARY_FILE).exists());
        assert_eq!(Project::open(&dir).unwrap().libraries, q.libraries);

        let lib = q.library_mut("work").unwrap();
        lib.storage = LibStorage::Gds;
        lib.cells.push(Cell::new("CMOS_INVERTER"));
        assert!(q.set_library_storage("work", LibStorage::Json).is_err(), "case clashes cannot become directories");
        assert!(q.set_library_storage("nope", LibStorage::Gds).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_layers_of_a_gds_library_are_added_on_open() {
        let dir = temp_dir("gdslayers");
        let mut p = Project::new("u");
        p.tech.layers.push(LayerDef::generated(120, 3, 0));
        let extra = p.tech.layers.len() - 1;
        p.set_library_storage("work", LibStorage::Gds).unwrap();
        let mut l = Layout::default();
        l.add_rect(0, 0, 0, 10, 10);
        l.add_rect(extra, 0, 0, 20, 20);
        p.add_cell("work", Cell { layout: Some(l), ..Cell::new("a") }).unwrap();
        p.save_as(&dir).unwrap();
        let mut tech = p.tech.clone();
        tech.layers.pop();
        write_json(&dir.join("tech.json"), &tech).unwrap();
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.tech.layers.len(), p.tech.layers.len());
        assert_eq!(q.tech.layers[extra].name, "L120D3");
        let d0 = &p.tech.layers[0];
        assert_eq!(layer_gds(&q, &CellRef::new("work", "a")), [(d0.gds_layer, d0.gds_datatype), (120, 3)]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_tech_without_vias_loads() {
        let mut v = serde_json::to_value(Tech::generic()).unwrap();
        v.as_object_mut().unwrap().remove("vias");
        let t: Tech = serde_json::from_value(v).unwrap();
        assert!(t.vias.is_empty());
    }
}
