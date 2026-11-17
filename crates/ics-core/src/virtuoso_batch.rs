//! Open Virtuoso libraries in one step, without linking OpenAccess: find the library through
//! `cds.lib`, run Virtuoso in batch mode (`virtuoso -nograph -replay`) with the dump script
//! [`crate::virtuoso_dump::DUMP_SKILL`], and return the dump for
//! [`crate::virtuoso_dump::import_dump`].
//!
//! Virtuoso is looked up as `$ICS_VIRTUOSO`, else `virtuoso` on the `PATH`. Without it, a dump
//! written in Virtuoso (`icsDumpLib`) is imported as before.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::virtuoso_dump::DUMP_SKILL;

/// Libraries defined by a `cds.lib` file: `DEFINE name path` (relative to the file),
/// `UNDEFINE name`, `INCLUDE file` and `SOFTINCLUDE file` (missing files skipped), `#` comments
/// and `$VAR` / `${VAR}` environment variables. Later definitions win.
pub fn parse_cds_lib(path: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut libs = Vec::new();
    read_cds_lib(path, &mut libs, 0, false)?;
    Ok(libs)
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = it.peek() == Some(&'{');
        if braced {
            it.next();
        }
        let mut name = String::new();
        while let Some(&n) = it.peek() {
            if n.is_ascii_alphanumeric() || n == '_' {
                name.push(n);
                it.next();
            } else {
                break;
            }
        }
        if braced && it.peek() == Some(&'}') {
            it.next();
        }
        out += &std::env::var(&name).unwrap_or_default();
    }
    out
}

fn read_cds_lib(path: &Path, libs: &mut Vec<(String, PathBuf)>, depth: usize, soft: bool) -> Result<(), String> {
    if depth > 16 {
        return Err(format!("{}: INCLUDE nested too deeply", path.display()));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) if soft => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let base = path.parent().unwrap_or(Path::new("."));
    let resolve = |p: &str| {
        let p = PathBuf::from(expand_env(p));
        if p.is_relative() { base.join(p) } else { p }
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [kw, name, dir, ..] if kw.eq_ignore_ascii_case("DEFINE") => {
                libs.retain(|(n, _)| n != name);
                libs.push((name.to_string(), resolve(dir)));
            }
            [kw, name, ..] if kw.eq_ignore_ascii_case("UNDEFINE") => libs.retain(|(n, _)| n != name),
            [kw, file, ..] if kw.eq_ignore_ascii_case("INCLUDE") => read_cds_lib(&resolve(file), libs, depth + 1, false)?,
            [kw, file, ..] if kw.eq_ignore_ascii_case("SOFTINCLUDE") => read_cds_lib(&resolve(file), libs, depth + 1, true)?,
            _ => {}
        }
    }
    Ok(())
}

/// Where to read a Virtuoso library from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A dump written by `icsDumpLib`.
    Dump(PathBuf),
    /// Library `lib` as `cds_lib` defines it (`None`: a library directory with no `cds.lib`
    /// defining it; a temporary one is written).
    Library { cds_lib: Option<PathBuf>, lib: String, dir: PathBuf },
}

fn is_library_dir(dir: &Path) -> bool {
    ["cdsinfo.tag", ".oalib", "data.dm"].iter().any(|f| dir.join(f).exists())
}

/// What `path` is: a dump file, a `cds.lib` (with `lib` naming the library), a directory
/// holding a `cds.lib`, or a library directory (found in the nearest `cds.lib` above it).
pub fn resolve_source(path: &Path, lib: Option<&str>) -> Result<Source, String> {
    let pick = |cds: &Path| -> Result<Source, String> {
        let libs = parse_cds_lib(cds)?;
        let names: Vec<&str> = libs.iter().map(|(n, _)| n.as_str()).collect();
        let Some(want) = lib else {
            return Err(format!("{} defines {} librar(ies); name one: {}", cds.display(), libs.len(), names.join(", ")));
        };
        match libs.iter().find(|(n, _)| n == want) {
            Some((n, d)) => Ok(Source::Library { cds_lib: Some(cds.to_path_buf()), lib: n.clone(), dir: d.clone() }),
            None => Err(format!("{} does not define library {want} (it has {})", cds.display(), names.join(", "))),
        }
    };
    if path.is_file() {
        let is_cds = path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("cds.lib"));
        return if is_cds { pick(path) } else { Ok(Source::Dump(path.to_path_buf())) };
    }
    if !path.is_dir() {
        return Err(format!("{}: no such file or directory", path.display()));
    }
    if path.join("cds.lib").is_file() && !is_library_dir(path) {
        return pick(&path.join("cds.lib"));
    }
    if !is_library_dir(path) {
        return Err(format!("{} is neither a Virtuoso library nor a directory with a cds.lib", path.display()));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).ok_or("library directory without a name")?;
    let name = lib.map(str::to_string).unwrap_or(name);
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let mut up = path.parent();
    while let Some(d) = up {
        let cds = d.join("cds.lib");
        if cds.is_file()
            && let Ok(libs) = parse_cds_lib(&cds)
            && let Some((n, _)) = libs.iter().find(|(_, dir)| canon(dir) == canon(path))
        {
            return Ok(Source::Library { cds_lib: Some(cds), lib: n.clone(), dir: path.to_path_buf() });
        }
        up = d.parent();
    }
    Ok(Source::Library { cds_lib: None, lib: name, dir: path.to_path_buf() })
}

/// The Virtuoso executable: `$ICS_VIRTUOSO`, else `virtuoso` on the `PATH`.
pub fn find_virtuoso() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ICS_VIRTUOSO").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = if cfg!(windows) { "virtuoso.exe" } else { "virtuoso" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// Library names go into file names and `cds.lib`: letters, digits, `_`, `-` and `.` only.
fn check_lib_name(lib: &str) -> Result<(), String> {
    let ok = !lib.is_empty()
        && !lib.starts_with('.')
        && lib.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok { Ok(()) } else { Err(format!("{lib:?} is not a valid Virtuoso library name")) }
}

/// An absolute path for a `cds.lib` line, which cannot hold whitespace.
fn cds_path(p: &Path) -> Result<String, String> {
    let p = std::path::absolute(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let s = p.display().to_string();
    if s.chars().any(char::is_whitespace) {
        return Err(format!("{s}: cds.lib paths cannot contain spaces; move or link the library to a path without spaces"));
    }
    Ok(s)
}

fn skill_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "/").replace('"', "\\\""))
}

/// Dump library `lib` by running `virtuoso` in batch mode in a temporary directory whose
/// `cds.lib` includes `cds_lib` (or defines the library at `dir`). Returns the dump text.
pub fn dump_library(virtuoso: &Path, cds_lib: Option<&Path>, lib: &str, dir: &Path) -> Result<String, String> {
    check_lib_name(lib)?;
    let cds = match cds_lib {
        Some(c) => format!("INCLUDE {}\n", cds_path(c)?),
        None => format!("DEFINE {lib} {}\n", cds_path(dir)?),
    };
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let work = std::env::temp_dir().join(format!("ics-virtuoso-{}-{run}-{lib}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let script = work.join("icsDump.il");
    let out = work.join(format!("{lib}.icsdump"));
    let replay = work.join("ics_replay.il");
    let log = work.join("CDS.log");
    let _ = std::fs::remove_file(&out);
    let write = |p: &Path, t: &str| std::fs::write(p, t).map_err(|e| format!("{}: {e}", p.display()));
    write(&work.join("cds.lib"), &cds)?;
    write(&script, DUMP_SKILL)?;
    write(
        &replay,
        &format!(
            "load({})\nicsDumpLib({} {})\nexit()\n",
            skill_string(&script.display().to_string()),
            skill_string(lib),
            skill_string(&out.display().to_string())
        ),
    )?;
    let status = Command::new(virtuoso)
        .args(["-nograph", "-replay"])
        .arg(&replay)
        .arg("-log")
        .arg(&log)
        .current_dir(&work)
        .status()
        .map_err(|e| format!("cannot run {}: {e}", virtuoso.display()))?;
    match std::fs::read_to_string(&out) {
        Ok(text) => {
            let _ = std::fs::remove_dir_all(&work);
            Ok(text)
        }
        Err(_) => {
            let tail: Vec<String> = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .rev()
                .take(15)
                .map(str::to_string)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Err(format!("Virtuoso ({status}) wrote no dump for {lib}; log {}:\n{}", log.display(), tail.join("\n")))
        }
    }
}

/// The dump text for `path` (see [`resolve_source`]): read from a dump file, or written by
/// Virtuoso in batch mode for a library.
pub fn read_dump(path: &Path, lib: Option<&str>) -> Result<String, String> {
    match resolve_source(path, lib)? {
        Source::Dump(p) => std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display())),
        Source::Library { cds_lib, lib, dir } => {
            let v = find_virtuoso().ok_or_else(|| {
                format!(
                    "{lib}: Virtuoso was not found (set ICS_VIRTUOSO or put virtuoso on the PATH); \
                     or dump the library in Virtuoso with icsDumpLib and import the dump (ics dump-skill)"
                )
            })?;
            dump_library(&v, cds_lib.as_deref(), &lib, &dir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ics-vb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cds_lib_defines_includes_and_undefines() {
        let d = tmp("cds");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        // SAFETY: tests in this module are the only users of this variable.
        unsafe { std::env::set_var("ICS_TEST_PDK", "/pdk") };
        std::fs::write(d.join("sub/pdk.lib"), "DEFINE analogLib $ICS_TEST_PDK/analogLib\nDEFINE old ./old\n").unwrap();
        std::fs::write(
            d.join("cds.lib"),
            "# libraries\nINCLUDE sub/pdk.lib\nSOFTINCLUDE missing.lib\nDEFINE myLib ./myLib  # mine\nUNDEFINE old\nDEFINE myLib2 ${ICS_TEST_PDK}/x\n",
        )
        .unwrap();
        let libs = parse_cds_lib(&d.join("cds.lib")).unwrap();
        let names: Vec<&str> = libs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["analogLib", "myLib", "myLib2"]);
        // On Windows a rooted path without a drive is on the drive of the cds.lib.
        let drive: PathBuf = d.components().take_while(|c| matches!(c, std::path::Component::Prefix(_))).collect();
        assert_eq!(libs[0].1, drive.join(PathBuf::from("/pdk/analogLib")));
        assert_eq!(libs[1].1, d.join("./myLib"));
        assert!(parse_cds_lib(&d.join("nope.lib")).is_err());
    }

    #[test]
    fn sources_are_resolved() {
        let d = tmp("src");
        std::fs::create_dir_all(d.join("myLib")).unwrap();
        std::fs::write(d.join("myLib/cdsinfo.tag"), "CDSLIBRARY\n").unwrap();
        std::fs::write(d.join("cds.lib"), "DEFINE myLib ./myLib\n").unwrap();
        std::fs::write(d.join("x.icsdump"), "").unwrap();
        assert_eq!(resolve_source(&d.join("x.icsdump"), None).unwrap(), Source::Dump(d.join("x.icsdump")));
        assert!(resolve_source(&d.join("cds.lib"), None).unwrap_err().contains("myLib"), "lists the libraries");
        assert!(matches!(resolve_source(&d, Some("myLib")).unwrap(), Source::Library { ref lib, .. } if lib == "myLib"));
        match resolve_source(&d.join("myLib"), None).unwrap() {
            Source::Library { cds_lib, lib, .. } => {
                assert_eq!(lib, "myLib");
                assert_eq!(cds_lib, Some(d.join("cds.lib")), "found in the cds.lib above it");
            }
            s => panic!("{s:?}"),
        }
    }

    /// The batch run, with a stand-in for Virtuoso that answers the replay file.
    #[cfg(unix)]
    #[test]
    fn batch_dump_runs_the_replay() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("batch");
        let fake = d.join("virtuoso");
        // Reads `-replay FILE`, finds the output path of icsDumpLib, writes a minimal dump there.
        std::fs::write(
            &fake,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -replay ] && R=$2; shift; done\n\
             grep -q '^INCLUDE\\|^DEFINE' cds.lib || exit 3\n\
             OUT=$(sed -n 's/^icsDumpLib(\"[^\"]*\" \"\\(.*\\)\")$/\\1/p' \"$R\")\n\
             printf '(\"icsdump\" 1 \"myLib\")\\n' > \"$OUT\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let lib = d.join("myLib");
        std::fs::create_dir_all(&lib).unwrap();
        let text = dump_library(&fake, None, "myLib", &lib).unwrap();
        assert!(text.starts_with("(\"icsdump\" 1 \"myLib\")"), "{text}");
        let dump = crate::virtuoso_dump::Dump::parse(&text).unwrap();
        assert_eq!(dump.lib, "myLib");
        // A failing Virtuoso reports its log.
        std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(dump_library(&fake, None, "myLib", &lib).unwrap_err().contains("wrote no dump"));
        // Library names never escape the scratch directory.
        assert!(dump_library(&fake, None, "../../x", &lib).unwrap_err().contains("not a valid"));
        assert!(dump_library(&fake, None, "a/b", &lib).unwrap_err().contains("not a valid"));
    }
}
