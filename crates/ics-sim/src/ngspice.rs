//! ngspice backend: runs the external simulator in batch mode and parses ASCII raw files.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use ics_core::Analysis;

use crate::SimError;
use crate::results::{Dataset, VecKind, Vector};

/// Returns the version banner if ngspice can be executed.
pub fn detect(exe: &str) -> Option<String> {
    let out = command(exe).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    text.lines().find(|l| l.to_lowercase().contains("ngspice")).map(|l| {
        let l = l.trim().trim_matches('*').trim();
        l.split(':').next().unwrap_or(l).trim().to_string()
    })
}

fn work_dir() -> std::io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!("ic-studio-ngspice-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Strip dot-analyses / .end / existing control blocks from a deck so we can drive it from
/// .control. The `pre_*` commands of those blocks (`pre_osdi` loading compiled Verilog-A
/// models) run before the deck is parsed, so they are returned to go into the new block.
fn strip_analyses(deck: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut pre = vec![];
    let mut in_control = false;
    for l in deck.lines() {
        let t = l.trim_start().to_ascii_lowercase();
        if t.starts_with(".control") {
            in_control = true;
            continue;
        }
        if t.starts_with(".endc") {
            in_control = false;
            continue;
        }
        if in_control {
            if t.starts_with("pre_") {
                pre.push(l.trim().to_string());
            }
            continue;
        }
        if [".op", ".dc", ".ac", ".tran", ".end"].iter().any(|k| t == *k || t.starts_with(&format!("{k} "))) {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    (out, pre)
}

pub fn run(exe: &str, deck: &str, analyses: &[Analysis]) -> Result<(Vec<Dataset>, String), SimError> {
    let dir = work_dir().map_err(|e| SimError::Backend(format!("cannot create temp dir: {e}")))?;
    let (mut text, pre) = strip_analyses(deck);
    text += ".control\n";
    for l in pre {
        text += &l;
        text.push('\n');
    }
    text += "set filetype=ascii\nset noaskquit\n";
    let mut files = vec![];
    for (i, a) in analyses.iter().enumerate() {
        // Relative to the working directory: Windows paths would need escaping in .control.
        let name = format!("a{i}_{}.raw", a.kind_name());
        text += &format!("{}\nwrite {name} all\n", a.spice_command());
        files.push((a, a.kind_name(), dir.join(name)));
    }
    text += "quit\n.endc\n.end\n";
    let cir = dir.join("deck.cir");
    std::fs::write(&cir, &text).map_err(|e| SimError::Backend(e.to_string()))?;
    let out =
        command(exe).arg("-b").arg("deck.cir").current_dir(&dir).output().map_err(|e| {
            SimError::Backend(format!("failed to launch '{exe}': {e}. Install ngspice or set its path in Settings."))
        })?;
    let mut log = String::from_utf8_lossy(&out.stdout).to_string();
    log += &String::from_utf8_lossy(&out.stderr);
    let mut datasets = vec![];
    for (a, kind, f) in &files {
        match std::fs::read_to_string(f) {
            Ok(raw) => {
                for mut ds in parse_raw(&raw)? {
                    ds.analysis = kind.to_string();
                    if let (Analysis::Dc { source, .. }, Some(sc)) = (a, ds.vectors.first_mut()) {
                        sc.name = source.to_ascii_lowercase();
                        sc.kind = VecKind::Sweep;
                    }
                    datasets.push(ds);
                }
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(SimError::Backend(format!("ngspice produced no output for '{kind}'.\n{log}")));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok((datasets, log))
}

fn normalize_name(name: &str, kind: VecKind) -> String {
    let n = name.to_ascii_lowercase();
    if let Some(b) = n.strip_suffix("#branch") {
        return format!("i({b})");
    }
    if kind == VecKind::Voltage && !n.starts_with("v(") {
        return format!("v({n})");
    }
    n
}

/// Parse an ASCII SPICE raw file (possibly containing several plots).
pub fn parse_raw(raw: &str) -> Result<Vec<Dataset>, SimError> {
    let bad = |m: &str| SimError::Backend(format!("raw file: {m}"));
    let mut out = vec![];
    let mut lines = raw.lines().peekable();
    while lines.peek().is_some() {
        let mut plotname = String::new();
        let mut complex = false;
        let mut nvars = 0usize;
        let mut npts = 0usize;
        let mut vars: Vec<(String, VecKind)> = vec![];
        // header
        loop {
            let Some(l) = lines.next() else { return Ok(out) };
            let (k, v) = l.split_once(':').unwrap_or((l, ""));
            match k.trim().to_ascii_lowercase().as_str() {
                "plotname" => plotname = v.trim().to_string(),
                "flags" => complex = v.to_ascii_lowercase().contains("complex"),
                "no. variables" => nvars = v.trim().parse().map_err(|_| bad("variables"))?,
                "no. points" => npts = v.trim().parse().map_err(|_| bad("points"))?,
                "variables" => {
                    for _ in 0..nvars {
                        let l = lines.next().ok_or_else(|| bad("truncated variables"))?;
                        let f: Vec<&str> = l.split_whitespace().collect();
                        let name = f.get(1).copied().unwrap_or("?");
                        let ty = f.get(2).copied().unwrap_or("");
                        let kind = match ty {
                            "time" => VecKind::Time,
                            "frequency" => VecKind::Frequency,
                            "voltage" => VecKind::Voltage,
                            "current" => VecKind::Current,
                            _ => VecKind::Other,
                        };
                        vars.push((normalize_name(name, kind), kind));
                    }
                }
                "values" => break,
                _ => {}
            }
        }
        let mut vectors: Vec<Vector> = vars
            .iter()
            .map(|(n, k)| Vector { name: n.clone(), kind: *k, re: Vec::with_capacity(npts), im: complex.then(Vec::new) })
            .collect();
        let mut tokens: Vec<String> = vec![];
        let needed = npts * (nvars + 1);
        while tokens.len() < needed {
            let Some(l) = lines.next() else { break };
            tokens.extend(l.split_whitespace().map(str::to_string));
        }
        for p in 0..npts {
            for v in 0..nvars {
                let tok = tokens.get(p * (nvars + 1) + 1 + v).ok_or_else(|| bad("truncated values"))?;
                if complex {
                    let (re, im) = tok.split_once(',').unwrap_or((tok, "0"));
                    vectors[v].re.push(re.parse().map_err(|_| bad("number"))?);
                    vectors[v].im.as_mut().unwrap().push(im.parse().map_err(|_| bad("number"))?);
                } else {
                    vectors[v].re.push(tok.parse().map_err(|_| bad("number"))?);
                }
            }
        }
        let op = plotname.to_ascii_lowercase().contains("operating point");
        if complex {
            // The frequency scale is real.
            if let Some(f) = vectors.first_mut() {
                f.im = None;
            }
        }
        if !op
            && let Some(f) = vectors.first_mut()
            && (f.kind == VecKind::Other || f.kind == VecKind::Voltage)
        {
            f.kind = VecKind::Sweep;
        }
        out.push(Dataset { analysis: String::new(), title: plotname, scale: (!op).then_some(0), vectors, devices: vec![] });
        // skip blank lines between plots
        while lines.peek().is_some_and(|l| l.trim().is_empty()) {
            lines.next();
        }
    }
    Ok(out)
}

/// On Windows `ngspice.exe` is the GUI build, which opens a window and does not write to
/// stdout; batch mode needs the console build `ngspice_con.exe` next to it.
fn console_exe(exe: &str) -> PathBuf {
    let p = PathBuf::from(exe);
    if cfg!(windows)
        && p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("ngspice"))
        && let Some(con) = p.parent().map(|d| d.join("ngspice_con.exe"))
        && con.exists()
    {
        return con;
    }
    p
}

fn command(exe: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(console_exe(exe));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

pub fn default_exe() -> String {
    // Common install locations, falling back to PATH lookup.
    let candidates = [
        "/opt/homebrew/bin/ngspice",
        "/usr/local/bin/ngspice",
        "/usr/bin/ngspice",
        "C:\\Spice64\\bin\\ngspice_con.exe",
        "C:\\Spice64\\bin\\ngspice.exe",
    ];
    for c in candidates {
        if Path::new(c).exists() {
            return c.to_string();
        }
    }
    "ngspice".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_pre_commands_of_control_blocks() {
        let deck = "* t\n.control\npre_osdi /pdk/bsimcmg.osdi\nrun\n.endc\nR1 a 0 1k\n.op\n.end\n";
        let (text, pre) = strip_analyses(deck);
        assert_eq!(pre, vec!["pre_osdi /pdk/bsimcmg.osdi".to_string()]);
        assert_eq!(text, "* t\nR1 a 0 1k\n");
    }

    #[test]
    fn parse_ascii_raw() {
        let raw = "Title: t\nDate: x\nPlotname: Transient Analysis\nFlags: real\nNo. Variables: 2\nNo. Points: 2\nVariables:\n\t0\ttime\ttime\n\t1\tout\tvoltage\nValues:\n 0\t0.0\n\t1.0\n\n 1\t1e-9\n\t2.0\n";
        let ds = parse_raw(raw).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].vectors[1].name, "v(out)");
        assert_eq!(ds[0].vectors[1].re, vec![1.0, 2.0]);
    }

    #[test]
    fn parse_complex_raw() {
        let raw = "Title: t\nPlotname: AC Analysis\nFlags: complex\nNo. Variables: 2\nNo. Points: 1\nVariables:\n\t0\tfrequency\tfrequency\n\t1\tv1#branch\tcurrent\nValues:\n 0\t1.0,0.0\n\t0.5,-0.5\n";
        let ds = parse_raw(raw).unwrap();
        assert_eq!(ds[0].vectors[1].name, "i(v1)");
        assert_eq!(ds[0].vectors[1].im.as_ref().unwrap()[0], -0.5);
        assert!(ds[0].vectors[0].im.is_none());
    }
}
