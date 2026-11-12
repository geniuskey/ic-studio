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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gds::{GdsImport, sanitize_name, unique_name};
use crate::layout::{Geometry, Layout};
use crate::pcell::{PCELL_LIB, PCell};
use crate::schematic::Schematic;
use crate::sim_setup::SimSetup;
use crate::tech::{LayerDef, Tech};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pcell: Option<PCell>,
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
    /// the same GDS numbers exists. Returns the library name and the top cells of the import.
    pub fn add_gds_library(
        &mut self,
        name_hint: &str,
        mut imp: GdsImport,
    ) -> std::result::Result<(String, Vec<CellRef>), String> {
        if imp.cells.is_empty() {
            return Err("the GDS file contains no cells".into());
        }
        let base = imp.new_layer_base;
        let new_layers: Vec<usize> = std::mem::take(&mut imp.new_layers)
            .into_iter()
            .map(|d| self.tech.layer_by_gds(d.gds_layer, d.gds_datatype).unwrap_or_else(|| push_layer(&mut self.tech, d)))
            .collect();
        for (_, l) in &mut imp.cells {
            for s in &mut l.shapes {
                if let Some(&to) = s.layer.checked_sub(base).and_then(|k| new_layers.get(k)) {
                    s.layer = to;
                }
            }
        }
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
        self.libraries.push(Library { name: lib.clone(), cells });
        Ok((lib, tops))
    }

    /// Replace the technology, moving every layout shape to the new layer with the same
    /// (GDS layer, datatype), else the same name. Layers in use without a match are appended
    /// to the new technology so no shape is lost. When the database unit changes, all layout
    /// coordinates are rescaled to keep their physical size. Returns warnings.
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
            libraries: self.libraries.iter().map(|l| l.name.clone()).collect(),
            settings: self.settings.clone(),
        };
        write_json(&root.join(MANIFEST), &manifest)?;
        write_json(&root.join("tech.json"), &self.tech)?;
        for lib in &self.libraries {
            for cell in &lib.cells {
                let dir = root.join(&lib.name).join(&cell.name);
                fs::create_dir_all(&dir).map_err(io_err(&dir))?;
                write_json(
                    &dir.join("cell.json"),
                    &CellMeta { description: cell.description.clone(), pcell: cell.pcell.clone() },
                )?;
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
                    cells.push(Cell { name, description: meta.description, schematic, layout, sim, pcell: meta.pcell });
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
        let nmos = PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 2 };
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
        assert!(p.ensure_pcell(&PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 0 }).is_err());
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
        let opts = ReadOptions { add_unknown_layers: true };

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

    #[test]
    fn old_tech_without_vias_loads() {
        let mut v = serde_json::to_value(Tech::generic()).unwrap();
        v.as_object_mut().unwrap().remove("vias");
        let t: Tech = serde_json::from_value(v).unwrap();
        assert!(t.vias.is_empty());
    }
}
