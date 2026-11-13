//! Self-update from GitHub Releases: check the latest release, download this platform's
//! binary next to the running one, swap it in and restart. The `ics` command line tool
//! (CLI and MCP server) is updated too when it sits next to the app, as the installers put it.

use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const REPO: &str = "geniuskey/ic-studio";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Command-line binaries released and updated alongside the app.
const TOOLS: [&str; 1] = ["ics"];

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x86_64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("macos", "aarch64") => Some("macos-aarch64"),
        _ => None,
    }
}

/// Name of the raw binary `bin` attached to each release for this platform.
fn asset_name(bin: &str) -> Option<String> {
    platform().map(|p| format!("{bin}-{p}{}", std::env::consts::EXE_SUFFIX))
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
    /// `(binary, url, size)` of the command-line tools.
    pub tools: Vec<(String, String, u64)>,
}

/// One file to download and swap in.
struct Job {
    url: String,
    size: u64,
    target: PathBuf,
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
    let find = |bin: &str| asset_name(bin).and_then(|n| r.assets.iter().find(|a| a.name == n));
    let asset = find("ic-studio");
    let tools = TOOLS.iter().filter_map(|t| find(t).map(|a| (t.to_string(), a.browser_download_url.clone(), a.size))).collect();
    Ok(Release {
        version: r.tag_name.trim_start_matches('v').to_string(),
        page: r.html_url,
        notes: r.body.unwrap_or_default(),
        url: asset.map(|a| a.browser_download_url.clone()),
        size: asset.map_or(0, |a| a.size),
        tools,
    })
}

fn current_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the executable: {e}"))?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

/// The app binary, then every tool that is installed next to it.
fn jobs(r: &Release) -> Result<Vec<Job>, String> {
    let exe = current_exe()?;
    let url = r.url.clone().ok_or("no download for this platform")?;
    let mut v = vec![Job { url, size: r.size, target: exe.clone() }];
    for (bin, url, size) in &r.tools {
        let target = exe.with_file_name(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
        if target.exists() {
            v.push(Job { url: url.clone(), size: *size, target });
        }
    }
    Ok(v)
}

/// Downloads every job first, then swaps them all in, so a failed download changes nothing.
fn download_all(jobs: &[Job], done: &AtomicU64) -> Result<PathBuf, String> {
    let mut base = 0;
    for j in jobs {
        if let Err(e) = download(&j.url, j.size, &with_suffix(&j.target, "new"), base, done) {
            for j in jobs {
                let _ = std::fs::remove_file(with_suffix(&j.target, "new"));
            }
            return Err(e);
        }
        base += j.size;
    }
    for j in jobs {
        install(&j.target, &with_suffix(&j.target, "new"))?;
    }
    current_exe()
}

/// Downloads `url` to `new`, adding the bytes read to `base` in `done`. [`install`] then
/// swaps it in: on Windows a running file cannot be overwritten but can be renamed, so it
/// is moved aside to `<exe>.old` (removed on the next start by [`cleanup`]).
fn download(url: &str, size: u64, new: &Path, base: u64, done: &AtomicU64) -> Result<(), String> {
    let res = ureq::get(url)
        .header("User-Agent", concat!("ic-studio/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut body = res.into_body();
    let mut reader = body.with_config().limit(1 << 31).reader();
    let mut file = std::fs::File::create(new).map_err(|e| format!("cannot write {}: {e}", new.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("write failed: {e}"))?;
        total += n as u64;
        done.store(base + total, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| format!("write failed: {e}"))?;
    drop(file);
    if size > 0 && total != size {
        return Err(format!("incomplete download ({total} of {size} bytes)"));
    }
    Ok(())
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
    if let Ok(exe) = current_exe() {
        let tools = TOOLS.iter().map(|t| exe.with_file_name(format!("{t}{}", std::env::consts::EXE_SUFFIX)));
        for f in std::iter::once(exe.clone()).chain(tools) {
            let _ = std::fs::remove_file(with_suffix(&f, "old"));
            let _ = std::fs::remove_file(with_suffix(&f, "new"));
        }
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

/// The `ics` command line tool next to the app as a plain path (no `\\?\` prefix), which
/// runs the MCP server as `ics mcp`.
fn ics_path() -> Result<String, String> {
    let ics = current_exe()?.with_file_name(format!("ics{}", std::env::consts::EXE_SUFFIX));
    if !ics.is_file() {
        return Err(format!("{} not found next to IC Studio", ics.display()));
    }
    let s = ics.display().to_string();
    Ok(s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s))
}

/// MCP client configuration that starts `ics mcp`.
pub fn mcp_config() -> Result<String, String> {
    let cfg = serde_json::json!({"mcpServers": {"ic-studio": {"command": ics_path()?, "args": ["mcp"]}}});
    Ok(serde_json::to_string_pretty(&cfg).unwrap_or_default())
}

fn claude(args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = std::process::Command::new("claude");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        // npm installs `claude.cmd`, which only a shell finds.
        if which("claude.exe").is_none() {
            cmd = std::process::Command::new("cmd");
            cmd.creation_flags(CREATE_NO_WINDOW).arg("/C").arg("claude");
        }
    }
    cmd.args(args).stdin(std::process::Stdio::null()).output()
}

#[cfg(windows)]
fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Register `ics mcp` as the `ic-studio` MCP server of Claude Code (user scope), replacing
/// an earlier registration.
pub fn add_to_claude_code() -> Result<String, String> {
    let ics = ics_path()?;
    let _ = claude(&["mcp", "remove", "--scope", "user", "ic-studio"]);
    let out = claude(&["mcp", "add", "--scope", "user", "ic-studio", "--", &ics, "mcp"])
        .map_err(|e| format!("Claude Code (claude) not found: {e}"))?;
    if out.status.success() {
        Ok(format!("Added the ic-studio MCP server to Claude Code ({ics} mcp). Restart Claude Code sessions to use it."))
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if msg.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { msg };
        Err(if msg.is_empty() {
            "Claude Code (claude) not found on the PATH".into()
        } else {
            format!("claude mcp add failed: {msg}")
        })
    }
}

pub enum State {
    Idle,
    Checking(Receiver<Result<Release, String>>),
    UpToDate,
    Available(Release),
    Downloading { release: Release, total: u64, done: Arc<AtomicU64>, rx: Receiver<Result<PathBuf, String>> },
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
        let release = release.clone();
        let jobs = match jobs(&release) {
            Ok(j) => j,
            Err(e) => {
                self.state = State::Failed(e);
                return;
            }
        };
        let total = if jobs.iter().all(|j| j.size > 0) { jobs.iter().map(|j| j.size).sum() } else { 0 };
        let done = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let (d, ctx) = (done.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(download_all(&jobs, &d));
            ctx.request_repaint();
        });
        self.state = State::Downloading { release, total, done, rx };
    }

    /// Download progress in [0, 1], when known.
    pub fn progress(&self) -> Option<f32> {
        match &self.state {
            State::Downloading { total, done, .. } if *total > 0 => Some(done.load(Ordering::Relaxed) as f32 / *total as f32),
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
