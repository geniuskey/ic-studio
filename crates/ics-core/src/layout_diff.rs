//! Geometric layout comparison: the flattened geometry of two versions of a cell XOR-ed per layer
//! (what was added, what was removed), plus the placements that appeared, disappeared or moved.
//! [`open_revision`] reads a project as it was at a git revision, so a cell can be compared with
//! any commit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::project::Project;
use crate::region::{BoolOp, Region};

#[derive(Debug, Clone, Default)]
pub struct LayerDiff {
    pub layer: usize,
    /// Geometry only in the new version.
    pub added: Region,
    /// Geometry only in the old version.
    pub removed: Region,
}

#[derive(Debug, Clone, Default)]
pub struct LayoutDiff {
    /// Layers that differ, in layer order.
    pub layers: Vec<LayerDiff>,
    /// Placements (by name, else `cell@x,y`) only in the new / old version, and those moved.
    pub placed: Vec<String>,
    pub unplaced: Vec<String>,
    pub moved: Vec<String>,
    /// Shapes skipped because they are not Manhattan.
    pub skipped: usize,
}

impl LayoutDiff {
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.placed.is_empty() && self.unplaced.is_empty() && self.moved.is_empty()
    }

    /// Every connected piece of difference: (layer, added, box), largest first.
    pub fn pieces(&self) -> Vec<(usize, bool, IRect)> {
        let mut out = vec![];
        for d in &self.layers {
            for (added, r) in [(true, &d.added), (false, &d.removed)] {
                for p in r.parts() {
                    if let Some(b) = p.bbox() {
                        out.push((d.layer, added, b, p.area()));
                    }
                }
            }
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.3));
        out.into_iter().map(|(l, a, b, _)| (l, a, b)).collect()
    }

    /// One line per differing layer, e.g. "metal1: +2 pieces 0.46 µm², -1 piece 0.2 µm²".
    pub fn summary(&self, names: &dyn Fn(usize) -> String, dbu_per_um: i64) -> Vec<String> {
        let um2 = |a: i128| a as f64 / (dbu_per_um.max(1) as f64).powi(2);
        let mut out = vec![];
        for d in &self.layers {
            let (na, nr) = (d.added.parts().len(), d.removed.parts().len());
            out.push(format!(
                "{}: +{na} piece(s) {:.4} µm², -{nr} piece(s) {:.4} µm²",
                names(d.layer),
                um2(d.added.area()),
                um2(d.removed.area())
            ));
        }
        for (sign, list) in [("+", &self.placed), ("-", &self.unplaced), ("~", &self.moved)] {
            for p in list {
                out.push(format!("{sign} {p}"));
            }
        }
        out
    }
}

fn layer_regions(top: &Layout, src: &dyn LayoutSource, skipped: &mut usize) -> BTreeMap<usize, Vec<IRect>> {
    let mut out: BTreeMap<usize, Vec<IRect>> = BTreeMap::new();
    for f in flatten(top, src, 64) {
        match &f.geom {
            Geometry::Label { .. } => {}
            Geometry::Rect(r) => out.entry(f.layer).or_default().push(*r),
            g => match Region::from_geometry(g) {
                Ok(r) => out.entry(f.layer).or_default().extend_from_slice(r.rects()),
                Err(_) => *skipped += 1,
            },
        }
    }
    out
}

fn inst_key(i: &crate::layout::CellInst) -> String {
    i.name.clone().unwrap_or_else(|| format!("{}@{},{}", i.cell, i.trans.x, i.trans.y))
}

/// Compare two versions of a cell (each with the source its instances resolve through).
pub fn diff(old: &Layout, old_src: &dyn LayoutSource, new: &Layout, new_src: &dyn LayoutSource) -> LayoutDiff {
    let mut out = LayoutDiff::default();
    let a = layer_regions(old, old_src, &mut out.skipped);
    let b = layer_regions(new, new_src, &mut out.skipped);
    let layers: std::collections::BTreeSet<usize> = a.keys().chain(b.keys()).copied().collect();
    for l in layers {
        let ra = Region::from_rects(a.get(&l).into_iter().flatten().copied());
        let rb = Region::from_rects(b.get(&l).into_iter().flatten().copied());
        let added = rb.boolean(&ra, BoolOp::Not);
        let removed = ra.boolean(&rb, BoolOp::Not);
        if !added.is_empty() || !removed.is_empty() {
            out.layers.push(LayerDiff { layer: l, added, removed });
        }
    }
    let named = |l: &Layout| -> BTreeMap<String, (crate::project::CellRef, crate::layout::Trans)> {
        l.instances.iter().map(|i| (inst_key(i), (i.cell.clone(), i.trans))).collect()
    };
    let (na, nb) = (named(old), named(new));
    for (k, v) in &nb {
        match na.get(k) {
            None => out.placed.push(k.clone()),
            Some(o) if o != v => out.moved.push(k.clone()),
            _ => {}
        }
    }
    out.unplaced = na.keys().filter(|k| !nb.contains_key(*k)).cloned().collect();
    out
}

/// The git work tree root containing `dir`, and `dir` relative to it.
fn git_root(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel", "--show-prefix"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let top = PathBuf::from(lines.next().unwrap_or_default().trim());
    let prefix = PathBuf::from(lines.next().unwrap_or_default().trim());
    Ok((top, prefix))
}

/// The project in `dir` as committed at git revision `rev` (e.g. `HEAD`, `HEAD~3`, a branch or
/// hash). Its files are copied out of git into a scratch directory, which is removed again.
pub fn open_revision(dir: &Path, rev: &str) -> Result<Project, String> {
    let dir = if dir.is_file() { dir.parent().unwrap_or(dir) } else { dir };
    let (top, prefix) = git_root(dir)?;
    let spec = prefix.to_string_lossy().replace('\\', "/");
    let ls = Command::new("git")
        .arg("-C")
        .arg(&top)
        .args(["ls-tree", "-r", "--name-only", rev, "--", if spec.is_empty() { "." } else { &spec }])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !ls.status.success() {
        return Err(String::from_utf8_lossy(&ls.stderr).trim().to_string());
    }
    let files: Vec<String> = String::from_utf8_lossy(&ls.stdout).lines().map(str::to_string).collect();
    if files.is_empty() {
        return Err(format!("the project is not in revision {rev}"));
    }
    let scratch = std::env::temp_dir().join(format!("ics-rev-{}-{}", std::process::id(), rev.replace(['/', '~', '^', ':'], "_")));
    let _ = std::fs::remove_dir_all(&scratch);
    let res = (|| {
        for f in &files {
            let rel = f.strip_prefix(&spec).unwrap_or(f);
            let show = Command::new("git")
                .arg("-C")
                .arg(&top)
                .arg("show")
                .arg(format!("{rev}:{f}"))
                .output()
                .map_err(|e| e.to_string())?;
            if !show.status.success() {
                return Err(String::from_utf8_lossy(&show.stderr).trim().to_string());
            }
            let to = scratch.join(rel);
            if let Some(p) = to.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            std::fs::write(&to, &show.stdout).map_err(|e| e.to_string())?;
        }
        let mut p = Project::open(&scratch).map_err(|e| e.to_string())?;
        p.root = None;
        Ok(p)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::NoCells;

    #[test]
    fn xor_per_layer_and_placements() {
        let mut a = Layout::default();
        a.add_rect(1, 0, 0, 100, 100);
        a.add_rect(2, 0, 0, 50, 50);
        let mut b = a.clone();
        b.add_rect(1, 100, 0, 150, 100);
        b.shapes.retain(|s| s.layer != 2);
        let d = diff(&a, &NoCells, &b, &NoCells);
        assert_eq!(d.layers.len(), 2);
        assert_eq!(d.layers[0].added.bbox(), Some(IRect::new(100, 0, 150, 100)));
        assert!(d.layers[0].removed.is_empty());
        assert_eq!(d.layers[1].removed.area(), 2500);
        assert_eq!(d.pieces()[0], (1, true, IRect::new(100, 0, 150, 100)));
        assert!(diff(&a, &NoCells, &a, &NoCells).is_empty());
        // The same geometry drawn differently is no difference.
        let mut c = Layout::default();
        c.add_rect(1, 0, 0, 60, 100);
        c.add_rect(1, 60, 0, 100, 100);
        c.add_rect(2, 0, 0, 50, 50);
        assert!(diff(&a, &NoCells, &c, &NoCells).is_empty());
    }

    #[test]
    fn project_at_git_revision() {
        let dir = std::env::temp_dir().join(format!("ics-diff-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proj = dir.join("chip");
        let git = |args: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(&dir).args(args).output().map(|o| o.status.success()).unwrap_or(false);
            assert!(ok, "git {args:?}");
        };
        std::fs::create_dir_all(&dir).unwrap();
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        let mut p = crate::examples::demo_project();
        p.save_as(&proj).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        let r = p.all_cells().find(|(_, c)| c.name == "cmos_inverter").map(|(r, _)| r).unwrap();
        p.cell_mut(&r).unwrap().layout.as_mut().unwrap().add_rect(6, 0, -5000, 1000, -4000);
        p.save().unwrap();
        let old = open_revision(&proj, "HEAD").unwrap();
        let d = diff(old.layout(&r).unwrap(), &old, p.layout(&r).unwrap(), &p);
        assert_eq!(d.layers.len(), 1);
        assert_eq!(d.layers[0].added.bbox(), Some(IRect::new(0, -5000, 1000, -4000)));
        assert!(open_revision(&proj, "no-such-rev").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn placements() {
        let r = crate::project::CellRef::new("l", "c");
        let mut a = Layout::default();
        let i = a.add_instance(r.clone(), crate::layout::Trans::new(0, 0), None);
        a.instance_mut(i).unwrap().name = Some("M1".into());
        let mut b = a.clone();
        b.translate_item(i, 10, 0);
        b.add_instance(r, crate::layout::Trans::new(500, 0), None);
        let d = diff(&a, &NoCells, &b, &NoCells);
        assert_eq!(d.moved, ["M1"]);
        assert_eq!(d.placed, ["l/c@500,0"]);
    }
}
