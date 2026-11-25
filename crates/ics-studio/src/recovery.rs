//! Autosave and crash recovery. While a project has unsaved changes a copy of it is written
//! every [`INTERVAL`] (Settings > Saving changes it) to a recovery folder outside the project (the project's own files are
//! only written by Save). Opening the project again after a crash offers the copy; saving or
//! discarding the changes removes it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ics_core::Project;
use ics_core::project::DiskState;
use serde::{Deserialize, Serialize};

/// Default time between autosaves while there are unsaved changes.
pub const INTERVAL: Duration = Duration::from_secs(120);

const INFO: &str = "recovery.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    /// The project the copy belongs to.
    pub root: PathBuf,
    /// When the copy was written (seconds since the Unix epoch).
    pub saved: u64,
}

/// Per-user folder for recovery copies and crash logs.
pub fn base_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("ICS_RECOVERY_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    dirs::data_local_dir().map(|d| d.join("ic-studio"))
}

/// The recovery folder of the project at `root`.
pub fn dir_for(root: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut h);
    let name = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    base_dir().map(|b| b.join("recovery").join(format!("{name}-{:016x}", h.finish())))
}

fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Write a recovery copy of `p` (which must have a root). Runs on a worker thread.
pub fn write(p: &Project) -> Result<(), String> {
    let root = p.root.clone().ok_or("the project has no folder yet")?;
    let dir = dir_for(&root).ok_or("no per-user data folder")?;
    let tmp = dir.with_extension("writing");
    let _ = std::fs::remove_dir_all(&tmp);
    let mut copy = p.clone();
    copy.root = Some(tmp.clone());
    copy.disk = DiskState::default();
    copy.save().map_err(|e| e.to_string())?;
    let info = Info { root, saved: now() };
    std::fs::write(tmp.join(INFO), serde_json::to_string_pretty(&info).unwrap_or_default()).map_err(|e| e.to_string())?;
    // Replace the previous copy only once the new one is complete.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())
}

/// The recovery copy of the project at `root` when it is newer than the project's own files.
pub fn pending(root: &Path) -> Option<(PathBuf, Info)> {
    let dir = dir_for(root)?;
    let info: Info = serde_json::from_str(&std::fs::read_to_string(dir.join(INFO)).ok()?).ok()?;
    let manifest = root.join(ics_core::project::MANIFEST);
    let saved_at = std::fs::metadata(&manifest)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (info.saved > saved_at).then_some((dir, info))
}

/// Open the copy in `dir` as the project at `root`: its content, with `root` as its folder so
/// that Save writes the project itself.
pub fn restore(dir: &Path, root: &Path) -> Result<Project, String> {
    let mut p = Project::open(dir).map_err(|e| e.to_string())?;
    p.root = Some(root.to_path_buf());
    // The project's own files as they are on disk now, for Save's change check.
    p.disk = Project::open(root).map(|o| o.disk).unwrap_or_default();
    Ok(p)
}

/// Remove the recovery copy of the project at `root`.
pub fn discard(root: &Path) {
    if let Some(dir) = dir_for(root) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Autosave scheduling: one background write at a time.
#[derive(Default)]
pub struct Autosave {
    last: Option<Instant>,
    job: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Autosave {
    /// Start a background copy of `p` when `dirty` has lasted `every` since the last one
    /// (`None`: autosave is off). Returns the error of a finished copy, if any.
    pub fn tick(&mut self, p: Option<&Project>, dirty: bool, every: Option<Duration>) -> Option<String> {
        let mut failed = None;
        if self.job.as_ref().is_some_and(|j| j.is_finished())
            && let Some(Err(e)) = self.job.take().and_then(|j| j.join().ok())
        {
            failed = Some(e);
        }
        if !dirty {
            self.last = None;
            return failed;
        }
        let since = *self.last.get_or_insert_with(Instant::now);
        if self.job.is_none()
            && every.is_some_and(|every| since.elapsed() >= every)
            && let Some(p) = p.filter(|p| p.root.is_some())
        {
            let copy = p.clone();
            self.job = Some(std::thread::spawn(move || write(&copy)));
            self.last = Some(Instant::now());
        }
        failed
    }
}

/// Write a crash log next to the recovery copies when the app panics (release builds on
/// Windows have no console), then run the default hook.
pub fn install_panic_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = base_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let bt = std::backtrace::Backtrace::force_capture();
            let text = format!("IC Studio {} crashed at {}\n{info}\n\n{bt}\n", env!("CARGO_PKG_VERSION"), now());
            let _ = std::fs::write(dir.join("crash.log"), text);
        }
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_restore_and_discard() {
        let base = std::env::temp_dir().join(format!("ics-recovery-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // SAFETY: the only test of this crate touching this variable.
        unsafe { std::env::set_var("ICS_RECOVERY_DIR", base.join("data")) };
        let root = base.join("proj");
        let mut p = ics_core::examples::demo_project();
        p.save_as(&root).unwrap();
        assert!(pending(&root).is_none());
        // An unsaved edit, copied by the autosave, then a "crash".
        let r = ics_core::CellRef::new("work", "cmos_inverter");
        p.cell_mut(&r).unwrap().description = "unsaved edit".into();
        std::thread::sleep(Duration::from_millis(1100));
        write(&p).unwrap();
        let (dir, info) = pending(&root).expect("a newer copy");
        assert_eq!(info.root, root);
        let back = restore(&dir, &root).unwrap();
        assert_eq!(back.root.as_deref(), Some(root.as_path()));
        assert_eq!(back.cell(&r).unwrap().description, "unsaved edit");
        back.save().unwrap();
        assert_eq!(Project::open(&root).unwrap().cell(&r).unwrap().description, "unsaved edit");
        discard(&root);
        assert!(pending(&root).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
