//! Self-update from GitHub Releases: check the latest release, download this platform's
//! binary next to the running one, swap it in and restart.

use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const REPO: &str = "geniuskey/ic-studio";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Name of the raw `ic-studio` binary attached to each release for this platform.
pub fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("ic-studio-windows-x86_64.exe"),
        ("linux", "x86_64") => Some("ic-studio-linux-x86_64"),
        ("macos", "aarch64") => Some("ic-studio-macos-aarch64"),
        _ => None,
    }
}

#[derive(Deserialize, Clone, Debug)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Deserialize, Clone, Debug)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub notes: String,
    pub url: Option<String>,
    pub size: u64,
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|x| x.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut res = ureq::get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("ic-studio/", env!("CARGO_PKG_VERSION")))
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("request failed: {e}"))?;
    match res.status().as_u16() {
        200 => {}
        404 => return Err("no release published yet".into()),
        s => return Err(format!("GitHub returned HTTP {s}")),
    }
    let r: GhRelease = res.body_mut().read_json().map_err(|e| format!("bad response: {e}"))?;
    let asset = asset_name().and_then(|n| r.assets.iter().find(|a| a.name == n));
    Ok(Release {
        version: r.tag_name.trim_start_matches('v').to_string(),
        page: r.html_url,
        notes: r.body.unwrap_or_default(),
        url: asset.map(|a| a.browser_download_url.clone()),
        size: asset.map_or(0, |a| a.size),
    })
}

/// Downloads to `<exe>.new`, then replaces the running executable. On Windows the running
/// file cannot be overwritten but can be renamed, so it is moved aside to `<exe>.old`
/// (removed on the next start by [`cleanup`]).
fn download_and_install(url: &str, size: u64, done: &AtomicU64) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the executable: {e}"))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let new = with_suffix(&exe, "new");
    let res = ureq::get(url)
        .header("User-Agent", concat!("ic-studio/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut body = res.into_body();
    let mut reader = body.with_config().limit(1 << 31).reader();
    let mut file = std::fs::File::create(&new).map_err(|e| format!("cannot write {}: {e}", new.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("write failed: {e}"))?;
        total += n as u64;
        done.store(total, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| format!("write failed: {e}"))?;
    drop(file);
    if size > 0 && total != size {
        let _ = std::fs::remove_file(&new);
        return Err(format!("incomplete download ({total} of {size} bytes)"));
    }
    install(&exe, &new)?;
    Ok(exe)
}

fn with_suffix(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(format!(".{suffix}"));
    PathBuf::from(s)
}

#[cfg(windows)]
fn install(exe: &Path, new: &Path) -> Result<(), String> {
    let old = with_suffix(exe, "old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| format!("cannot move the running executable aside: {e}"))?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("cannot install the update: {e}"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn install(exe: &Path, new: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("chmod failed: {e}"))?;
    std::fs::rename(new, exe).map_err(|e| format!("cannot install the update: {e}"))
}

/// Removes leftovers of a previous update.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(with_suffix(&exe, "old"));
        let _ = std::fs::remove_file(with_suffix(&exe, "new"));
    }
}

/// Starts the installed executable with the same arguments.
pub fn relaunch(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot restart: {e}"))
}

pub enum State {
    Idle,
    Checking(Receiver<Result<Release, String>>),
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: Arc<AtomicU64>, rx: Receiver<Result<PathBuf, String>> },
    Installed { release: Release, exe: PathBuf },
    Failed(String),
}

pub struct Updater {
    pub state: State,
    /// Whether the result of the running check should be reported (a manual check).
    pub verbose: bool,
}

pub enum Event {
    UpToDate,
    Available(String),
    Installed(String),
    Failed(String),
}

impl Default for Updater {
    fn default() -> Self {
        Self { state: State::Idle, verbose: false }
    }
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Checking(_) | State::Downloading { .. })
    }

    pub fn check(&mut self, ctx: &egui::Context, verbose: bool) {
        if self.busy() {
            return;
        }
        self.verbose = verbose;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_latest());
            ctx.request_repaint();
        });
        self.state = State::Checking(rx);
    }

    pub fn download(&mut self, ctx: &egui::Context) {
        let State::Available(release) = &self.state else { return };
        let Some(url) = release.url.clone() else { return };
        let release = release.clone();
        let size = release.size;
        let done = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let (d, ctx) = (done.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(download_and_install(&url, size, &d));
            ctx.request_repaint();
        });
        self.state = State::Downloading { release, done, rx };
    }

    /// Download progress in [0, 1], when known.
    pub fn progress(&self) -> Option<f32> {
        match &self.state {
            State::Downloading { release, done, .. } if release.size > 0 => {
                Some(done.load(Ordering::Relaxed) as f32 / release.size as f32)
            }
            _ => None,
        }
    }

    pub fn available(&self) -> Option<&Release> {
        match &self.state {
            State::Available(r) | State::Downloading { release: r, .. } | State::Installed { release: r, .. } => Some(r),
            _ => None,
        }
    }

    pub fn poll(&mut self) -> Option<Event> {
        let next = match &self.state {
            State::Checking(rx) => match rx.try_recv() {
                Ok(Ok(r)) if is_newer(&r.version, CURRENT) => {
                    let v = r.version.clone();
                    self.state = State::Available(r);
                    return Some(Event::Available(v));
                }
                Ok(Ok(_)) => (State::UpToDate, Event::UpToDate),
                Ok(Err(e)) => (State::Failed(e.clone()), Event::Failed(e)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => (State::Idle, Event::Failed("update check stopped".into())),
            },
            State::Downloading { release, rx, .. } => match rx.try_recv() {
                Ok(Ok(exe)) => {
                    let v = release.version.clone();
                    (State::Installed { release: release.clone(), exe }, Event::Installed(v))
                }
                Ok(Err(e)) => (State::Failed(e.clone()), Event::Failed(e)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => (State::Idle, Event::Failed("download stopped".into())),
            },
            _ => return None,
        };
        self.state = next.0;
        Some(next.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(is_newer("0.2.0-rc1", "0.1.0"));
    }

    #[test]
    fn swaps_binary() {
        let dir = std::env::temp_dir().join(format!("ics-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ic-studio.exe");
        let new = with_suffix(&exe, "new");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        install(&exe, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert!(!new.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
