//! Sign-off DRC / LVS with external tools: KLayout (`.drc` / `.lvs` decks), Siemens Calibre
//! (SVRF rule files) and netgen (netlist comparison).
//!
//! A [`Runset`] of the technology names the tool, the check and the deck. [`run`] writes the
//! cell and everything it places to `<cell>.gds` in a fresh work directory, plus for LVS the
//! schematic as a `.subckt` (`<cell>_sch.cir`), runs the tool there and reads its results:
//!
//! - KLayout: `klayout -b -r DECK -rd input=GDS -rd topcell=CELL -rd report=FILE
//!   [-rd schematic=CIR] [-rd K=V ...]` (variable names configurable). DRC results come from the
//!   report database (`.lyrdb`); LVS from the cross-reference of the `.lvsdb`.
//! - Calibre: a control file sets `LAYOUT PATH / PRIMARY / SYSTEM`, the results database
//!   (ASCII) or `SOURCE` and `LVS REPORT`, then `INCLUDE`s the deck, so the deck must not set
//!   those itself. `calibre -drc -hier CTRL` / `calibre -lvs -hier -spice CELL.sp CTRL`.
//! - netgen: compares IC Studio's own extraction (`<cell>_lay.cir`) with the schematic:
//!   `netgen -batch lvs "LAY CELL" "SCH CELL" SETUP REPORT`, the deck being the setup file.
//!
//! Marker coordinates are mapped to top-cell dbu; results reported inside sub-cells keep the
//! sub-cell coordinates and name the cell in their description.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::drc::{DrcReport, Violation};
use crate::extract::{Extraction, extract};
use crate::geom::IRect;
use crate::netlist::netlist_for;
use crate::project::{CellRef, Project};
use crate::schematic::Schematic;
use crate::tech::{LengthUnit, Tech};
use crate::tech_import::{Node, parse_xml};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Klayout,
    Calibre,
    Netgen,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Klayout => "KLayout",
            Tool::Calibre => "Calibre",
            Tool::Netgen => "netgen",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Drc,
    Lvs,
}

impl Check {
    pub fn label(self) -> &'static str {
        match self {
            Check::Drc => "DRC",
            Check::Lvs => "LVS",
        }
    }
}

/// An external sign-off check of the technology.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runset {
    pub name: String,
    pub tool: Tool,
    pub check: Check,
    /// Rule deck (KLayout script, SVRF rule file) or netgen setup file.
    pub deck: String,
    /// Extra KLayout `-rd` variables, or Calibre `#DEFINE`s (value ignored).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    /// KLayout variable names for the inputs and the report.
    #[serde(default, skip_serializing_if = "KlayoutVars::is_default")]
    pub klayout: KlayoutVars,
    /// Schematic element letters left out of the LVS netlist.
    #[serde(default = "default_skip")]
    pub skip: Vec<String>,
    /// Net name the schematic ground ("0") gets in the LVS netlist.
    #[serde(default = "default_ground")]
    pub ground: String,
}

fn default_skip() -> Vec<String> {
    vec!["V".into(), "I".into()]
}

fn default_ground() -> String {
    "gnd".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KlayoutVars {
    pub input: String,
    pub topcell: String,
    pub report: String,
    pub schematic: String,
}

impl Default for KlayoutVars {
    fn default() -> Self {
        KlayoutVars { input: "input".into(), topcell: "topcell".into(), report: "report".into(), schematic: "schematic".into() }
    }
}

impl KlayoutVars {
    fn is_default(&self) -> bool {
        *self == KlayoutVars::default()
    }
}

/// Executables of the external tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tools {
    pub klayout: String,
    pub calibre: String,
    pub netgen: String,
}

impl Default for Tools {
    fn default() -> Self {
        Tools { klayout: default_exe(Tool::Klayout), calibre: default_exe(Tool::Calibre), netgen: default_exe(Tool::Netgen) }
    }
}

impl Tools {
    pub fn exe(&self, t: Tool) -> &str {
        match t {
            Tool::Klayout => &self.klayout,
            Tool::Calibre => &self.calibre,
            Tool::Netgen => &self.netgen,
        }
    }
}

/// The tool's executable: `ICS_KLAYOUT` / `ICS_CALIBRE` / `ICS_NETGEN`, else a common install
/// location, else the bare name (looked up on PATH).
pub fn default_exe(t: Tool) -> String {
    let (var, name) = match t {
        Tool::Klayout => ("ICS_KLAYOUT", "klayout"),
        Tool::Calibre => ("ICS_CALIBRE", "calibre"),
        Tool::Netgen => ("ICS_NETGEN", "netgen"),
    };
    if let Ok(v) = std::env::var(var)
        && !v.trim().is_empty()
    {
        return v;
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    match t {
        Tool::Klayout => {
            if let Ok(a) = std::env::var("APPDATA") {
                candidates.push(Path::new(&a).join("KLayout").join("klayout_app.exe"));
            }
            candidates.extend(
                [
                    "C:\\Program Files\\KLayout\\klayout_app.exe",
                    "C:\\Program Files (x86)\\KLayout\\klayout_app.exe",
                    "/Applications/klayout.app/Contents/MacOS/klayout",
                    "/opt/homebrew/bin/klayout",
                    "/usr/local/bin/klayout",
                    "/usr/bin/klayout",
                ]
                .map(PathBuf::from),
            );
        }
        Tool::Calibre => {
            if let Ok(h) = std::env::var("MGC_HOME") {
                candidates.push(Path::new(&h).join("bin").join("calibre"));
            }
        }
        Tool::Netgen => candidates.extend(["/usr/local/bin/netgen", "/usr/bin/netgen"].map(PathBuf::from)),
    }
    candidates.into_iter().find(|c| c.exists()).map_or_else(|| name.to_string(), |c| c.to_string_lossy().into_owned())
}

/// Outcome of an external LVS.
#[derive(Debug, Clone, PartialEq)]
pub struct LvsOutcome {
    /// `None` when the result could not be read.
    pub matched: Option<bool>,
    /// Error messages of the comparison (KLayout), or the report's summary lines.
    pub messages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SignoffReport {
    pub runset: String,
    pub tool: Tool,
    pub check: Check,
    pub drc: Option<DrcReport>,
    pub lvs: Option<LvsOutcome>,
    /// The tool's console output.
    pub log: String,
    /// Work directory holding the inputs and reports.
    pub dir: PathBuf,
    pub millis: f64,
}

impl SignoffReport {
    pub fn summary(&self) -> String {
        let what = format!("{} ({})", self.runset, self.tool.label());
        match (&self.drc, &self.lvs) {
            (Some(d), _) => format!("{what}: {} violation(s) in {:.1} s", d.violations.len(), self.millis / 1e3),
            (_, Some(l)) => match l.matched {
                Some(true) => format!("{what}: layout matches schematic"),
                Some(false) => format!("{what}: layout does not match schematic ({} message(s))", l.messages.len()),
                None => format!("{what}: no LVS result found; see {}", self.dir.display()),
            },
            _ => what,
        }
    }
}

fn work_dir(cell: &str) -> Result<PathBuf, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "ic-studio-signoff-{}-{}-{}",
        cell.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>(),
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(d)
}

/// The runset of `tech` named `name` (case-insensitive), or the only one whose name contains it.
pub fn find<'a>(tech: &'a Tech, name: &str) -> Result<&'a Runset, String> {
    let list = || tech.signoff.iter().map(|r| format!("'{}'", r.name)).collect::<Vec<_>>().join(", ");
    if tech.signoff.is_empty() {
        return Err(format!("technology '{}' has no sign-off runsets", tech.name));
    }
    if let Some(r) = tech.signoff.iter().find(|r| r.name.eq_ignore_ascii_case(name)) {
        return Ok(r);
    }
    let lower = name.to_ascii_lowercase();
    let hits: Vec<&Runset> = tech.signoff.iter().filter(|r| r.name.to_ascii_lowercase().contains(&lower)).collect();
    match hits.as_slice() {
        [r] => Ok(r),
        [] => Err(format!("no runset '{name}'; the technology has {}", list())),
        _ => Err(format!(
            "'{name}' matches several runsets: {}",
            hits.iter().map(|r| format!("'{}'", r.name)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Name of `cell` as a GDS structure when written with everything it places.
fn top_name(project: &Project, cell: &CellRef) -> String {
    let clash = crate::hier::descendants(project, cell).iter().any(|r| r != cell && r.cell == cell.cell);
    if clash { format!("{}__{}", cell.lib, cell.cell) } else { cell.cell.clone() }
}

/// Run `runset` on `cell` of `project`.
pub fn run(project: &Project, cell: &CellRef, runset: &Runset, tools: &Tools) -> Result<SignoffReport, String> {
    let start = Instant::now();
    let c = project.cell(cell).ok_or_else(|| format!("no cell '{cell}'"))?;
    if c.layout.is_none() {
        return Err(format!("{cell} has no layout"));
    }
    let deck = PathBuf::from(&runset.deck);
    if !deck.exists() {
        return Err(format!("deck {} does not exist", deck.display()));
    }
    let deck = std::path::absolute(&deck).unwrap_or(deck);
    let tech = &project.tech;
    let dir = work_dir(&cell.cell)?;
    let top = top_name(project, cell);
    let gds = dir.join(format!("{}.gds", cell.cell));
    let bytes = crate::gds::write_gds_hier(&cell.lib, project, std::slice::from_ref(cell), tech);
    std::fs::write(&gds, bytes).map_err(|e| format!("{}: {e}", gds.display()))?;
    let schematic = match runset.check {
        Check::Lvs => {
            let sch = c.schematic.as_ref().ok_or_else(|| format!("LVS needs a schematic view of {cell}"))?;
            let path = dir.join(format!("{}_sch.cir", cell.cell));
            std::fs::write(&path, lvs_netlist(sch, tech, &top, runset)).map_err(|e| format!("{}: {e}", path.display()))?;
            Some(path)
        }
        Check::Drc => None,
    };
    let exe = tools.exe(runset.tool);
    let mut cmd = Command::new(exe);
    cmd.current_dir(&dir);
    let report = match (runset.tool, runset.check) {
        (Tool::Klayout, check) => {
            let v = &runset.klayout;
            let report = dir.join(if check == Check::Drc { "report.lyrdb" } else { "report.lvsdb" });
            cmd.arg("-b").arg("-r").arg(&deck);
            let mut rd = |k: &str, val: &str| {
                cmd.arg("-rd").arg(format!("{k}={val}"));
            };
            rd(&v.input, &gds.to_string_lossy());
            rd(&v.topcell, &top);
            rd(&v.report, &report.to_string_lossy());
            if let Some(s) = &schematic {
                rd(&v.schematic, &s.to_string_lossy());
            }
            for (k, val) in &runset.vars {
                rd(k, val);
            }
            report
        }
        (Tool::Calibre, check) => {
            let q = |p: &Path| format!("\"{}\"", p.to_string_lossy().replace('\\', "/"));
            let mut ctrl = format!("LAYOUT PATH {}\nLAYOUT PRIMARY \"{top}\"\nLAYOUT SYSTEM GDSII\n", q(&gds));
            for k in runset.vars.keys() {
                ctrl += &format!("#DEFINE {k}\n");
            }
            let report = match check {
                Check::Drc => {
                    let r = dir.join("drc.results");
                    ctrl +=
                        &format!("DRC RESULTS DATABASE {} ASCII\nDRC SUMMARY REPORT {}\n", q(&r), q(&dir.join("drc.summary")));
                    r
                }
                Check::Lvs => {
                    let r = dir.join("lvs.report");
                    let s = schematic.as_deref().expect("LVS schematic");
                    ctrl +=
                        &format!("SOURCE PATH {}\nSOURCE PRIMARY \"{top}\"\nSOURCE SYSTEM SPICE\nLVS REPORT {}\n", q(s), q(&r));
                    r
                }
            };
            ctrl += &format!("INCLUDE {}\n", q(&deck));
            let ctrl_path = dir.join("runset.svrf");
            std::fs::write(&ctrl_path, ctrl).map_err(|e| format!("{}: {e}", ctrl_path.display()))?;
            match check {
                Check::Drc => cmd.args(["-drc", "-hier"]),
                Check::Lvs => cmd.args(["-lvs", "-hier", "-spice"]).arg(dir.join(format!("{}.sp", cell.cell))),
            };
            cmd.arg(&ctrl_path);
            report
        }
        (Tool::Netgen, Check::Drc) => return Err("netgen only compares netlists (LVS)".into()),
        (Tool::Netgen, Check::Lvs) => {
            let lay = c.layout.as_ref().expect("checked");
            let x = extract(lay, project, tech);
            let lay_path = dir.join(format!("{}_lay.cir", cell.cell));
            std::fs::write(&lay_path, layout_netlist(&x, tech, &top)).map_err(|e| format!("{}: {e}", lay_path.display()))?;
            let report = dir.join("netgen.report");
            let s = schematic.as_deref().expect("LVS schematic");
            cmd.arg("-batch").arg("lvs");
            cmd.arg(format!("{} {top}", lay_path.to_string_lossy())).arg(format!("{} {top}", s.to_string_lossy()));
            cmd.arg(&deck).arg(&report);
            report
        }
    };
    let out = cmd.output().map_err(|e| format!("cannot run {} ({exe}): {e}", runset.tool.label()))?;
    let log = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let read = || std::fs::read_to_string(&report).ok();
    let fail = |log: &str| {
        let tail: Vec<&str> = log.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{} produced no report ({}):\n{}", runset.tool.label(), out.status, tail.join("\n"))
    };
    let mut rep = SignoffReport {
        runset: runset.name.clone(),
        tool: runset.tool,
        check: runset.check,
        drc: None,
        lvs: None,
        log,
        dir,
        millis: 0.0,
    };
    match runset.check {
        Check::Drc => {
            let text = read().ok_or_else(|| fail(&rep.log))?;
            let mut d = match runset.tool {
                Tool::Klayout => parse_lyrdb(&text, tech.dbu_per_um, &top)?,
                _ => parse_calibre_rdb(&text, tech.dbu_per_um)?,
            };
            d.millis = start.elapsed().as_secs_f64() * 1e3;
            rep.drc = Some(d);
        }
        Check::Lvs => {
            let text = read();
            let outcome = match runset.tool {
                Tool::Klayout => text.as_deref().map(|t| parse_lvsdb(t, &top)),
                Tool::Calibre => text.as_deref().map(parse_calibre_lvs),
                Tool::Netgen => Some(parse_netgen(&(text.unwrap_or_default() + &rep.log))),
            };
            rep.lvs = Some(outcome.ok_or_else(|| fail(&rep.log))?);
        }
    }
    rep.millis = start.elapsed().as_secs_f64() * 1e3;
    Ok(rep)
}

fn spice_net(n: &str, ground: &str) -> String {
    if n == "0" { ground.to_string() } else { n.split_whitespace().collect::<Vec<_>>().join("_") }
}

/// The schematic as `.subckt TOP <named nets>` without sources, models and analyses.
pub fn lvs_netlist(sch: &Schematic, tech: &Tech, top: &str, runset: &Runset) -> String {
    let nl = netlist_for(sch, tech, "");
    let skip = |line: &str| {
        let first = line.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default();
        line.starts_with('.') || line.starts_with('*') || runset.skip.iter().any(|s| s.eq_ignore_ascii_case(&first))
    };
    let auto = |n: &str| n.strip_prefix("net").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()));
    let mut ports: Vec<String> =
        nl.conn.pin_nets.values().filter(|n| !auto(n) && n.as_str() != "?").map(|n| spice_net(n, &runset.ground)).collect();
    ports.sort();
    ports.dedup();
    let mut s = format!("* {top}: schematic for LVS, generated by IC Studio\n.subckt {top} {}\n", ports.join(" "));
    for line in nl.body.lines().filter(|l| !l.trim().is_empty() && !skip(l)) {
        let toks: Vec<String> =
            line.split_whitespace().map(|t| if t == "0" { runset.ground.clone() } else { t.to_string() }).collect();
        s += &toks.join(" ");
        s.push('\n');
    }
    s += ".ends\n";
    s
}

/// IC Studio's extraction as `.subckt TOP <labelled nets>`, with the PDK element and length
/// conventions of the schematic netlist.
pub fn layout_netlist(x: &Extraction, tech: &Tech, top: &str) -> String {
    let net = |i: usize| x.nets.get(i).map_or("?".to_string(), |n| spice_net(&n.name, "gnd"));
    let mut ports: Vec<String> = x.nets.iter().enumerate().filter(|(_, n)| n.labelled).map(|(i, _)| net(i)).collect();
    ports.sort();
    ports.dedup();
    let unit = tech.length_unit();
    let len = |v: i64| {
        let m = v as f64 / x.dbu_per_um.max(1) as f64 * 1e-6;
        match unit {
            LengthUnit::M => format!("{}u", m * 1e6),
            LengthUnit::Um => format!("{}", m * 1e6),
        }
    };
    let mut s = format!("* {top}: extracted by IC Studio\n.subckt {top} {}\n", ports.join(" "));
    for d in &x.devices {
        let subckt = tech.devices.iter().any(|t| t.subckt && t.name == d.model);
        let prefix = if subckt { "X" } else { "" };
        s += &format!(
            "{prefix}{} {} {} {} {} {} w={} l={}\n",
            d.name,
            net(d.d),
            net(d.g),
            net(d.s),
            net(d.b),
            d.model,
            len(d.w),
            len(d.l)
        );
    }
    s += ".ends\n";
    s
}

/// Bounding box of all `x,y` number pairs in `s` (µm), in dbu.
fn bbox_of(s: &str, dbu_per_um: i64) -> Option<IRect> {
    let nums: Vec<f64> = s
        .split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .filter_map(|t| t.parse().ok())
        .collect();
    let f = dbu_per_um as f64;
    let pts: Vec<(i64, i64)> = nums.chunks_exact(2).map(|p| ((p[0] * f).round() as i64, (p[1] * f).round() as i64)).collect();
    let (x0, x1) = (pts.iter().map(|p| p.0).min()?, pts.iter().map(|p| p.0).max()?);
    let (y0, y1) = (pts.iter().map(|p| p.1).min()?, pts.iter().map(|p| p.1).max()?);
    Some(IRect::new(x0, y0, x1, y1))
}

fn unquote(s: &str) -> String {
    s.trim().replace('\'', "")
}

/// Violations of a KLayout report database (`.lyrdb`).
pub fn parse_lyrdb(text: &str, dbu_per_um: i64, top: &str) -> Result<DrcReport, String> {
    let root = parse_xml(text)?;
    let db = if root.name == "report-database" {
        &root
    } else {
        root.child("report-database").ok_or("not a KLayout report database")?
    };
    let mut descriptions = BTreeMap::new();
    fn walk(n: &Node, prefix: &str, out: &mut BTreeMap<String, String>) {
        for c in n.children.iter().filter(|c| c.name == "category") {
            let name = format!("{prefix}{}", unquote(c.child_text("name").unwrap_or_default()));
            out.insert(name.clone(), c.child_text("description").unwrap_or_default().to_string());
            if let Some(sub) = c.child("categories") {
                walk(sub, &format!("{name}."), out);
            }
        }
    }
    if let Some(c) = db.child("categories") {
        walk(c, "", &mut descriptions);
    }
    let mut rep = DrcReport { rules_run: descriptions.len(), ..Default::default() };
    for item in db.child("items").map(|i| i.children.as_slice()).unwrap_or_default().iter().filter(|i| i.name == "item") {
        let rule = unquote(item.child_text("category").unwrap_or_default());
        let cell = unquote(item.child_text("cell").unwrap_or_default());
        let values = item.child("values").map(|v| v.children.as_slice()).unwrap_or_default();
        let marker = values
            .iter()
            .filter(|v| !v.text.trim_start().starts_with("text") && !v.text.trim_start().starts_with("float"))
            .filter_map(|v| v.text.split_once(':').and_then(|(_, g)| bbox_of(g, dbu_per_um)))
            .reduce(|a, b| a.union(&b));
        let Some(marker) = marker else {
            rep.skipped += 1;
            continue;
        };
        let mut description = descriptions.get(&rule).cloned().filter(|d| !d.is_empty()).unwrap_or_else(|| rule.clone());
        if !cell.is_empty() && cell != top {
            description += &format!(" (in cell {cell})");
        }
        rep.violations.push(Violation { rule, description, layer: None, marker });
    }
    Ok(rep)
}

/// Violations of a Calibre ASCII results database.
pub fn parse_calibre_rdb(text: &str, dbu_per_um: i64) -> Result<DrcReport, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let header = lines.next().ok_or("empty results database")?;
    let precision: f64 = header.split_whitespace().nth(1).and_then(|p| p.parse().ok()).ok_or("bad results database header")?;
    let scale = dbu_per_um as f64 / precision.max(1.0);
    let mut rep = DrcReport::default();
    let num = |t: &str| t.parse::<f64>().map_err(|_| format!("bad number '{t}' in results database"));
    while let Some(rule) = lines.next() {
        let counts = lines.next().ok_or("truncated results database")?;
        let c: Vec<&str> = counts.split_whitespace().collect();
        let (n, comments) = match (c.first().map(|v| v.parse::<usize>()), c.get(2).map(|v| v.parse::<usize>())) {
            (Some(Ok(n)), Some(Ok(k))) => (n, k),
            _ => return Err(format!("bad counts line '{counts}' for rule {rule}")),
        };
        let mut description = Vec::new();
        for _ in 0..comments {
            description.push(lines.next().ok_or("truncated results database")?.to_string());
        }
        let description = description.join(" ");
        rep.rules_run += 1;
        let mut done = 0;
        let mut cell: Option<String> = None;
        while done < n {
            let head = lines.next().ok_or("truncated results database")?;
            let h: Vec<&str> = head.split_whitespace().collect();
            match h.first().copied() {
                Some("CN") => {
                    cell = h.get(1).map(|s| s.to_string());
                    continue;
                }
                Some("p") | Some("e") => {}
                _ => return Err(format!("unexpected line '{head}' in rule {rule}")),
            }
            let k: usize = h.get(2).and_then(|v| v.parse().ok()).ok_or_else(|| format!("bad result line '{head}'"))?;
            let mut pts = Vec::new();
            for _ in 0..k {
                let l = lines.next().ok_or("truncated results database")?;
                for pair in l.split_whitespace().collect::<Vec<_>>().chunks_exact(2) {
                    pts.push(((num(pair[0])? * scale).round() as i64, (num(pair[1])? * scale).round() as i64));
                }
            }
            done += 1;
            let (Some(x0), Some(x1)) = (pts.iter().map(|p| p.0).min(), pts.iter().map(|p| p.0).max()) else { continue };
            let (y0, y1) = (pts.iter().map(|p| p.1).min().unwrap_or(0), pts.iter().map(|p| p.1).max().unwrap_or(0));
            let mut d = if description.is_empty() { rule.to_string() } else { description.clone() };
            if let Some(c) = &cell {
                d += &format!(" (in cell {c})");
            }
            rep.violations.push(Violation {
                rule: rule.to_string(),
                description: d,
                layer: None,
                marker: IRect::new(x0, y0, x1, y1),
            });
        }
    }
    Ok(rep)
}

/// Top-cell result of a KLayout LVS database (`.lvsdb`): the status of the `top` circuit pair
/// in the cross-reference (`1` match, `W` match with warnings, `X` mismatch; `S`, not compared,
/// gives no result) and its messages.
pub fn parse_lvsdb(text: &str, top: &str) -> LvsOutcome {
    let mut matched = None;
    let mut messages = Vec::new();
    let mut in_top = false;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("X(").or_else(|| line.strip_prefix("circuit(")) {
            let t: Vec<&str> = rest.split_whitespace().collect();
            in_top = t.len() >= 3 && (t[0].eq_ignore_ascii_case(top) || t[1].eq_ignore_ascii_case(top));
            if in_top {
                matched = match t[2].trim_end_matches(')') {
                    "1" | "W" | "match" | "warning" => Some(true),
                    "X" | "mismatch" => Some(false),
                    _ => matched,
                };
                if let Some(m) = line.find("B('") {
                    let body = &line[m + 3..];
                    messages.push(body.rfind("')").map_or(body, |e| &body[..e]).replace("\\n", " "));
                }
            }
        } else if in_top && let Some(m) = line.find("B('") {
            let body = &line[m + 3..];
            if let Some(end) = body.rfind("')") {
                let kind = if line.contains("M(E") {
                    "error"
                } else if line.contains("M(W") {
                    "warning"
                } else {
                    "info"
                };
                messages.push(format!("{kind}: {}", &body[..end]));
            }
        }
    }
    LvsOutcome { matched, messages }
}

/// Overall result of a Calibre LVS report.
pub fn parse_calibre_lvs(text: &str) -> LvsOutcome {
    let overall = text.find("OVERALL COMPARISON RESULTS").map_or(text, |i| &text[i..]);
    let head: String = overall.lines().take(30).collect::<Vec<_>>().join("\n");
    let matched = if head.contains("INCORRECT") || head.contains("NOT COMPARED") {
        Some(false)
    } else if head.contains("CORRECT") {
        Some(true)
    } else {
        None
    };
    let messages = overall
        .lines()
        .filter(|l| l.contains("Error:") || l.contains("Warning:"))
        .map(|l| l.trim().to_string())
        .take(50)
        .collect();
    LvsOutcome { matched, messages }
}

/// Result of a netgen comparison from its report and console output.
pub fn parse_netgen(text: &str) -> LvsOutcome {
    let lower = text.to_ascii_lowercase();
    let matched = if lower.contains("netlists do not match") || lower.contains("circuits do not match") {
        Some(false)
    } else if lower.contains("netlists match") || lower.contains("circuits match") {
        Some(true)
    } else {
        None
    };
    let messages = text
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("mismatch") || l.contains("do not match") || l.contains("match uniquely") || l.contains("property errors")
        })
        .map(|l| l.trim().to_string())
        .take(50)
        .collect();
    LvsOutcome { matched, messages }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LYRDB: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<report-database>
 <top-cell>inv</top-cell>
 <categories>
  <category><name>M1.W</name><description>metal1 width &lt; 0.5 um</description><categories></categories></category>
  <category><name>grp</name><description></description><categories>
   <category><name>P.S</name><description>poly space</description><categories></categories></category>
  </categories></category>
 </categories>
 <items>
  <item><category>'M1.W'</category><cell>inv</cell><values>
   <value>edge-pair: (0.24,0.2;0.24,1.07)|(0.58,1.07;0.58,0.2)</value></values></item>
  <item><category>'grp'.'P.S'</category><cell>sub</cell><values>
   <value>polygon: (0,0;0,1;-1.5,1;-1.5,0)</value></values></item>
  <item><category>'M1.W'</category><cell>inv</cell><values><value>text: ('x',r0 1,2)</value></values></item>
 </items>
</report-database>"#;

    #[test]
    fn klayout_report_database() {
        let r = parse_lyrdb(LYRDB, 1000, "inv").unwrap();
        assert_eq!(r.rules_run, 3);
        assert_eq!(r.skipped, 1);
        assert_eq!(r.violations.len(), 2);
        assert_eq!(r.violations[0].rule, "M1.W");
        assert_eq!(r.violations[0].description, "metal1 width < 0.5 um");
        assert_eq!(r.violations[0].marker, IRect::new(240, 200, 580, 1070));
        assert_eq!(r.violations[1].rule, "grp.P.S");
        assert_eq!(r.violations[1].description, "poly space (in cell sub)");
        assert_eq!(r.violations[1].marker, IRect::new(-1500, 0, 0, 1000));
    }

    #[test]
    fn calibre_results_database() {
        let rdb = "inv 1000\nM1.W\n2 2 1 Jan 1 2026\nmetal1 width < 0.23\np 1 4\n0 0\n0 1000\n200 1000\n200 0\nCN sub c 1 0 0 1 0 0\ne 2 1\n-500 0 -500 300\nV1.S\n0 0 0\n";
        let r = parse_calibre_rdb(rdb, 1000).unwrap();
        assert_eq!(r.rules_run, 2);
        assert_eq!(r.violations.len(), 2);
        assert_eq!(r.violations[0].marker, IRect::new(0, 0, 200, 1000));
        assert_eq!(r.violations[0].description, "metal1 width < 0.23");
        assert_eq!(r.violations[1].marker, IRect::new(-500, 0, -500, 300));
        assert!(r.violations[1].description.ends_with("(in cell sub)"));
        assert_eq!(
            parse_calibre_rdb("inv 2000\nR\n1 1 0\np 1 1\n10 20\n", 1000).unwrap().violations[0].marker,
            IRect::new(5, 10, 5, 10)
        );
        assert!(parse_calibre_rdb("inv 1000\nR\n1 1 0\nq 1 1\n", 1000).is_err());
    }

    #[test]
    fn lvs_results() {
        let ok = "Z(\n X(inv INV 1\n  Z(\n   N(1 1 1)\n";
        assert_eq!(parse_lvsdb(ok, "inv").matched, Some(true));
        let bad = "Z(\n X(inv INV X\n  L(\n   M(E B('Net out is not matching any net from reference netlist'))\n";
        let o = parse_lvsdb(bad, "inv");
        assert_eq!(o.matched, Some(false));
        assert_eq!(o.messages, ["error: Net out is not matching any net from reference netlist"]);
        assert_eq!(parse_lvsdb("", "inv").matched, None);
        let skipped = parse_lvsdb(" X(inv INV S B('could not be compared:\\n  A: sub')\n", "inv");
        assert_eq!((skipped.matched, skipped.messages[0].as_str()), (None, "could not be compared:   A: sub"));
        let rep = "   OVERALL COMPARISON RESULTS\n\n   #       ###################      _   _   \n   #       #                 #      *   *   \n   #   #   #     CORRECT     #       |    \n";
        assert_eq!(parse_calibre_lvs(rep).matched, Some(true));
        assert_eq!(parse_calibre_lvs("OVERALL COMPARISON RESULTS\n #   INCORRECT   #\n").matched, Some(false));
        assert_eq!(parse_netgen("Circuit 1 contains 2 devices\nNetlists match uniquely.\n").matched, Some(true));
        assert_eq!(parse_netgen("Netlists do not match.\n").matched, Some(false));
    }

    #[test]
    fn lvs_netlists() {
        let p = crate::examples::demo_project();
        let r = CellRef::new("work", "cmos_inverter");
        let c = p.cell(&r).unwrap();
        let rs = Runset {
            name: "t".into(),
            tool: Tool::Klayout,
            check: Check::Lvs,
            deck: String::new(),
            vars: BTreeMap::new(),
            klayout: KlayoutVars::default(),
            skip: vec!["V".into(), "I".into(), "C".into()],
            ground: "gnd".into(),
        };
        let s = lvs_netlist(c.schematic.as_ref().unwrap(), &p.tech, "cmos_inverter", &rs);
        assert_eq!(
            s.lines().skip(1).collect::<Vec<_>>(),
            [
                ".subckt cmos_inverter gnd in out vdd",
                "M1 out in vdd vdd pmos w=2u l=180n",
                "M2 out in gnd gnd nmos w=1u l=180n",
                ".ends"
            ]
        );
        let x = extract(c.layout.as_ref().unwrap(), &p, &p.tech);
        let l = layout_netlist(&x, &p.tech, "cmos_inverter");
        assert!(l.contains(".subckt cmos_inverter gnd in out vdd\n"), "{l}");
        assert!(l.contains("M1 out in gnd sub nmos w=1u l=0.18u\n"), "{l}");
    }

    /// Runs only where KLayout is installed: a small KLayout DRC and LVS deck for generic180 on
    /// the demo inverter, placed (with well taps) and routed from its schematic.
    #[test]
    fn klayout_signoff_on_demo_inverter() {
        let tools = Tools::default();
        if Command::new(&tools.klayout).arg("-v").output().is_err() {
            eprintln!("KLayout not found; skipped");
            return;
        }
        let mut p = crate::examples::demo_project();
        let r = CellRef::new("work", "cmos_inverter");
        let sch = p.cell(&r).and_then(|c| c.schematic.clone()).unwrap();
        let plan = crate::sdl::plan(&sch, &p.tech);
        let mut lay = crate::Layout::default();
        crate::sdl::apply(&mut p, &mut lay, &plan).unwrap();
        assert!(crate::route::route(&sch, &mut lay, &p, &p.tech).unwrap().failed.is_empty());
        p.cell_mut(&r).unwrap().layout = Some(lay);

        let dir = work_dir("decks").unwrap();
        let drc_deck = dir.join("t.drc");
        std::fs::write(
            &drc_deck,
            "source($input, $topcell)
report(\"t\", $report)
input(7, 0).width(0.5).output(\"M1.W\", \"metal1 width < 0.5 um\")
input(5, 0).width($minpoly.to_f).output(\"P.W\", \"poly width\")
",
        )
        .unwrap();
        let mut rs = Runset {
            name: "klayout drc".into(),
            tool: Tool::Klayout,
            check: Check::Drc,
            deck: drc_deck.to_string_lossy().into_owned(),
            vars: BTreeMap::from([("minpoly".to_string(), "0.1".to_string())]),
            klayout: KlayoutVars::default(),
            skip: default_skip(),
            ground: default_ground(),
        };
        let rep = run(&p, &r, &rs, &tools).unwrap();
        let d = rep.drc.as_ref().unwrap();
        assert!(
            !d.violations.is_empty() && d.violations.iter().all(|v| v.rule == "M1.W"),
            "{:?}
{}",
            d.violations,
            rep.log
        );
        assert_eq!(d.rules_run, 2);

        let lvs_deck = dir.join("t.lvs");
        std::fs::write(&lvs_deck, GENERIC_LVS).unwrap();
        rs.check = Check::Lvs;
        rs.deck = lvs_deck.to_string_lossy().into_owned();
        rs.skip.push("C".into());
        let rep = run(&p, &r, &rs, &tools).unwrap();
        assert_eq!(
            rep.lvs.as_ref().unwrap().matched,
            Some(true),
            "{:?}
{}",
            rep.lvs,
            rep.log
        );
        assert!(rep.summary().contains("matches"), "{}", rep.summary());
        let sch = p.cell_mut(&r).unwrap().schematic.as_mut().unwrap();
        sch.instances.iter_mut().find(|i| i.name == "M1").unwrap().params.insert("w".into(), "3u".into());
        let rep = run(&p, &r, &rs, &tools).unwrap();
        let l = rep.lvs.as_ref().unwrap();
        assert_eq!(l.matched, Some(false), "{:?}", l.messages);
        std::fs::remove_dir_all(dir).ok();
    }

    /// generic180 LVS: PMOS bulk on the nwell (joined by n+ taps), NMOS bulk and p+ taps on the
    /// substrate.
    const GENERIC_LVS: &str = r#"source($input, $topcell)
schematic($schematic)
report_lvs($report)
deep
nwell = input(1, 0)
active = input(2, 0)
nplus = input(3, 0)
pplus = input(4, 0)
poly = input(5, 0)
contact = input(6, 0)
metal1 = input(7, 0)
via1 = input(8, 0)
metal2 = input(9, 0)
via2 = input(10, 0)
metal3 = input(11, 0)
text = labels(63, 0)
bulk = polygon_layer
pactive = active & pplus & nwell
nactive = active & nplus - nwell
pgate = pactive & poly
ngate = nactive & poly
psd = pactive - pgate
nsd = nactive - ngate
ntap = active & nplus & nwell
ptap = active & pplus - nwell
extract_devices(mos4("PMOS"), { "SD" => psd, "G" => pgate, "W" => nwell, "tS" => psd, "tD" => psd, "tG" => poly, "tW" => nwell })
extract_devices(mos4("NMOS"), { "SD" => nsd, "G" => ngate, "W" => bulk, "tS" => nsd, "tD" => nsd, "tG" => poly, "tW" => bulk })
connect(psd, contact)
connect(nsd, contact)
connect(poly, contact)
connect(contact, metal1)
connect(metal1, via1)
connect(via1, metal2)
connect(metal2, via2)
connect(via2, metal3)
connect(metal1, text)
connect(ntap, contact)
connect(ntap, nwell)
connect(ptap, contact)
connect_global(ptap, "SUB")
connect_global(bulk, "SUB")
align
netlist.simplify
schematic.simplify
compare
"#;
}
