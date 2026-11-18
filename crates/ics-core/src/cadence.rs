//! Cadence Virtuoso design interchange without OpenAccess: the text formats Virtuoso reads
//! and writes itself.
//!
//! - **CDL in** ([`import_cdl`]): a hierarchical CDL netlist as Virtuoso's auCdl netlister
//!   writes it (`.SUBCKT` with `*.PININFO`, `X` instances with `/ cell`, subcircuit
//!   parameters, `.GLOBAL` and `!` global nets, `$` annotations) is flattened from its top
//!   subcircuit into a schematic. Instance paths become element names (`MI0/M0` is `MM0`
//!   inside `XI0`) and local nets get the instance path (`I0/net7`).
//! - **SKILL out** ([`skill_schematic`]): a SKILL script that recreates a schematic in
//!   Virtuoso with analogLib symbols (or PDK symbols from a [`SymbolMap`]). Every terminal
//!   gets a wire stub and a wire label with its net, placed at the terminal's pin as the
//!   symbol defines it (looked up when the script runs), so connectivity does not depend on
//!   the symbol's geometry. Load it in the CIW: `load("inv.il")`.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::connectivity;
use crate::schematic::{DeviceKind, PortDir, Schematic};
use crate::spice_import::{self, Imported};
use crate::tech::Tech;
use crate::units::parse_spice;

/// A subcircuit of a CDL netlist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Subckt {
    pub name: String,
    pub pins: Vec<String>,
    /// `*.PININFO` directions by pin (`I`, `O`, `B`).
    pub dirs: BTreeMap<String, String>,
    /// Parameters with their defaults.
    pub params: Vec<(String, String)>,
    /// Element lines (logical, comments dropped).
    lines: Vec<String>,
}

/// A parsed CDL netlist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cdl {
    pub subckts: Vec<Subckt>,
    /// Element lines outside any subcircuit.
    top: Vec<String>,
    pub globals: Vec<String>,
}

/// Logical lines: `+` continuations joined, `*` comments dropped except `*.PININFO`, inline
/// `$` annotations kept for the caller.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for raw in text.lines() {
        let line = raw.trim();
        let upper = line.to_ascii_uppercase();
        if line.is_empty() || (line.starts_with('*') && !upper.starts_with("*.PININFO")) {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+')
            && let Some(last) = out.last_mut()
        {
            last.push(' ');
            last.push_str(rest.trim());
            continue;
        }
        out.push(line.to_string());
    }
    out
}

/// Split off `$` annotations: (the line before the first `$`, `$[model]` if any).
fn annotations(line: &str) -> (&str, Option<String>) {
    match line.split_once('$') {
        Some((head, rest)) => {
            let model = rest.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(m, _)| m.trim().to_string());
            let model = model.or_else(|| {
                rest.split('$')
                    .find_map(|a| a.trim().strip_prefix('[').and_then(|r| r.split_once(']')).map(|(m, _)| m.trim().to_string()))
            });
            (head.trim(), model)
        }
        None => (line.trim(), None),
    }
}

/// Whitespace tokens with `key = value` joined to `key=value`.
fn tokens(line: &str) -> Vec<String> {
    let raw: Vec<&str> = line.split_whitespace().collect();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < raw.len() {
        let t = raw[i];
        if t == "=" && !out.is_empty() && i + 1 < raw.len() {
            *out.last_mut().expect("checked") += &format!("={}", raw[i + 1]);
            i += 2;
        } else if let Some(k) = t.strip_suffix('=').filter(|_| i + 1 < raw.len()) {
            out.push(format!("{k}={}", raw[i + 1]));
            i += 2;
        } else if t.starts_with('=') && !out.is_empty() {
            *out.last_mut().expect("checked") += t;
            i += 1;
        } else {
            out.push(t.to_string());
            i += 1;
        }
    }
    out
}

impl Cdl {
    pub fn parse(text: &str) -> Result<Cdl, String> {
        let mut cdl = Cdl::default();
        let mut cur: Option<Subckt> = None;
        for line in logical_lines(text) {
            let upper = line.to_ascii_uppercase();
            if let Some(info) = upper.strip_prefix("*.PININFO") {
                let s = cur.as_mut().ok_or("*.PININFO outside a .SUBCKT")?;
                // Keep the case of the names from the original line.
                for item in line[line.len() - info.len()..].split_whitespace() {
                    if let Some((n, d)) = item.rsplit_once(':') {
                        s.dirs.insert(n.to_string(), d.to_ascii_uppercase());
                    }
                }
                continue;
            }
            let t = tokens(annotations(&line).0);
            let Some(head) = t.first() else { continue };
            match head.to_ascii_lowercase().as_str() {
                ".subckt" => {
                    if cur.is_some() {
                        return Err(format!("nested .SUBCKT {}", t.get(1).map_or("", |s| s.as_str())));
                    }
                    let name = t.get(1).ok_or(".SUBCKT without a name")?.clone();
                    let (pins, params): (Vec<&String>, Vec<&String>) = t[2..].iter().partition(|s| !s.contains('='));
                    cur = Some(Subckt {
                        name,
                        pins: pins.into_iter().filter(|p| !p.eq_ignore_ascii_case("params:")).cloned().collect(),
                        params: params
                            .iter()
                            .filter_map(|p| p.split_once('='))
                            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
                            .collect(),
                        ..Default::default()
                    });
                }
                ".ends" => cdl.subckts.push(cur.take().ok_or(".ENDS without .SUBCKT")?),
                ".global" => cdl.globals.extend(t[1..].iter().map(|g| global_name(g))),
                h if h.starts_with('.') => {}
                _ => match &mut cur {
                    Some(s) => s.lines.push(line.clone()),
                    None => cdl.top.push(line.clone()),
                },
            }
        }
        if let Some(s) = cur {
            return Err(format!(".SUBCKT {} has no .ENDS", s.name));
        }
        if cdl.subckts.is_empty() && cdl.top.is_empty() {
            return Err("no subcircuits or elements in the CDL".into());
        }
        Ok(cdl)
    }

    pub fn subckt(&self, name: &str) -> Option<&Subckt> {
        self.subckts.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// The subcircuits no other one instantiates, in file order.
    pub fn tops(&self) -> Vec<String> {
        let used: Vec<String> = self
            .subckts
            .iter()
            .flat_map(|s| s.lines.iter())
            .chain(&self.top)
            .filter_map(|l| x_cell(&tokens(annotations(l).0)).map(|(_, c, _)| c.to_ascii_lowercase()))
            .collect();
        self.subckts.iter().filter(|s| !used.contains(&s.name.to_ascii_lowercase())).map(|s| s.name.clone()).collect()
    }
}

/// `vdd!` is the global `vdd`; `gnd!` is ground.
fn global_name(n: &str) -> String {
    n.trim_end_matches('!').to_string()
}

/// An `X` line: its nodes, cell and parameters.
type XLine = (Vec<String>, String, Vec<(String, String)>);

/// The nodes, cell and parameters of an `X` line.
fn x_cell(t: &[String]) -> Option<XLine> {
    if !t.first()?.to_ascii_uppercase().starts_with('X') {
        return None;
    }
    let rest = &t[1..];
    let params: Vec<(String, String)> =
        rest.iter().filter_map(|s| s.split_once('=')).map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    let pos: Vec<&String> = rest.iter().filter(|s| !s.contains('=')).collect();
    match pos.iter().position(|s| *s == "/") {
        Some(k) => Some((pos[..k].iter().map(|s| s.to_string()).collect(), pos.get(k + 1)?.to_string(), params)),
        None => {
            let (cell, nodes) = pos.split_last()?;
            Some((nodes.iter().map(|s| s.to_string()).collect(), cell.trim_start_matches('/').to_string(), params))
        }
    }
}

/// Evaluate a parameter value: a number, a parameter name or an arithmetic expression of them
/// (`'w*2'`, `{l+10n}`). Values without parameters are returned as written.
fn eval_param(v: &str, env: &HashMap<String, f64>) -> Result<String, String> {
    let s = v.trim().trim_matches('\'').trim_matches('"');
    let s = s.strip_prefix('{').and_then(|x| x.strip_suffix('}')).unwrap_or(s);
    if parse_spice(s).is_some() {
        return Ok(s.to_string());
    }
    let mut p = Expr { s: s.as_bytes(), i: 0, env };
    let x = p.sum()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("cannot evaluate '{v}'"));
    }
    Ok(format!("{x:e}"))
}

struct Expr<'a> {
    s: &'a [u8],
    i: usize,
    env: &'a HashMap<String, f64>,
}

impl Expr<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn sum(&mut self) -> Result<f64, String> {
        let mut x = self.product()?;
        while let Some(c @ (b'+' | b'-')) = self.peek() {
            self.i += 1;
            let y = self.product()?;
            x = if c == b'+' { x + y } else { x - y };
        }
        Ok(x)
    }
    fn product(&mut self) -> Result<f64, String> {
        let mut x = self.unary()?;
        while let Some(c @ (b'*' | b'/')) = self.peek() {
            self.i += 1;
            let y = self.unary()?;
            x = if c == b'*' { x * y } else { x / y };
        }
        Ok(x)
    }
    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            Some(b'(') => {
                self.i += 1;
                let x = self.sum()?;
                if self.peek() != Some(b')') {
                    return Err("missing ')'".into());
                }
                self.i += 1;
                Ok(x)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || b"._".contains(&self.s[self.i])) {
                    self.i += 1;
                }
                let tok = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
                if tok.is_empty() {
                    return Err("expected a value".into());
                }
                parse_spice(tok)
                    .or_else(|| self.env.get(&tok.to_ascii_lowercase()).copied())
                    .ok_or_else(|| format!("unknown parameter '{tok}'"))
            }
        }
    }
}

/// Nodes of a leaf element by its letter.
fn node_count(letter: char) -> Option<usize> {
    match letter {
        'M' => Some(4),
        'R' | 'C' | 'L' | 'D' | 'V' | 'I' => Some(2),
        _ => None,
    }
}

/// A flattened CDL top.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Flat {
    pub top: String,
    /// The flat SPICE netlist.
    pub netlist: String,
    /// Top pins with their `*.PININFO` direction.
    pub pins: Vec<(String, String)>,
    pub devices: usize,
    pub warnings: Vec<String>,
}

impl Cdl {
    /// Flatten from subcircuit `top` (or the elements outside subcircuits when `None` and
    /// there are some).
    pub fn flatten(&self, top: Option<&str>) -> Result<Flat, String> {
        let mut f = Flat::default();
        let (lines, env, pins) = match top {
            Some(t) => {
                let s = self.subckt(t).ok_or_else(|| format!("no .SUBCKT {t}"))?;
                f.top = s.name.clone();
                f.pins = s.pins.iter().map(|p| (p.clone(), s.dirs.get(p).cloned().unwrap_or_default())).collect();
                let mut env = HashMap::new();
                for (k, v) in &s.params {
                    env.insert(k.clone(), parse_spice(&eval_param(v, &env)?).unwrap_or(0.0));
                }
                (s.lines.clone(), env, s.pins.clone())
            }
            None => (self.top.clone(), HashMap::new(), vec![]),
        };
        let nets: HashMap<String, String> = pins.iter().map(|p| (p.to_ascii_lowercase(), p.clone())).collect();
        let mut out = vec![];
        self.expand(&lines, "", &nets, &env, 0, &mut out, &mut f.warnings, None)?;
        f.devices = out.len();
        f.netlist = out.join("\n") + "\n";
        Ok(f)
    }

    #[allow(clippy::too_many_arguments)]
    fn expand(
        &self,
        lines: &[String],
        path: &str,
        ports: &HashMap<String, String>,
        env: &HashMap<String, f64>,
        depth: usize,
        out: &mut Vec<String>,
        warnings: &mut Vec<String>,
        mut keep: Option<&mut Vec<XInst>>,
    ) -> Result<(), String> {
        if depth > 50 {
            return Err("subcircuits nest more than 50 deep (recursive?)".into());
        }
        let net = |n: &str| -> String {
            let low = n.to_ascii_lowercase();
            if let Some(p) = ports.get(&low) {
                return p.clone();
            }
            if n.ends_with('!') || self.globals.iter().any(|g| g.eq_ignore_ascii_case(n)) || n == "0" {
                return global_name(n);
            }
            if path.is_empty() { n.to_string() } else { format!("{path}/{n}") }
        };
        let seg = |name: &str| -> String {
            let rest = &name[1..];
            if path.is_empty() { rest.to_string() } else { format!("{path}/{rest}") }
        };
        for line in lines {
            let (head, model) = annotations(line);
            let t = tokens(head);
            let Some(name) = t.first() else { continue };
            let letter = name.chars().next().unwrap_or(' ').to_ascii_uppercase();
            if letter == 'X' {
                let (nodes, cell, params) = x_cell(&t).ok_or_else(|| format!("{name}: cannot read the instance"))?;
                let Some(sub) = self.subckt(&cell) else {
                    // A device subcircuit of the PDK (no .SUBCKT here): keep it as an X line.
                    let mut l = format!("X{}", seg(name));
                    for n in &nodes {
                        l += &format!(" {}", net(n));
                    }
                    l += &format!(" {cell}");
                    for (k, v) in &params {
                        l += &format!(" {k}={}", eval_param(v, env)?);
                    }
                    out.push(l);
                    continue;
                };
                if nodes.len() != sub.pins.len() {
                    return Err(format!("{name}: {} nodes for the {} pins of {}", nodes.len(), sub.pins.len(), sub.name));
                }
                let mut child_env: HashMap<String, f64> = HashMap::new();
                for (k, v) in &sub.params {
                    let given = params.iter().find(|(p, _)| p == k).map(|(_, v)| v.as_str());
                    let value = match given {
                        Some(g) => eval_param(g, env)?,
                        None => eval_param(v, &child_env)?,
                    };
                    child_env.insert(k.clone(), parse_spice(&value).unwrap_or(0.0));
                }
                for (k, _) in params.iter().filter(|(k, _)| !sub.params.iter().any(|(p, _)| p == k)) {
                    warnings.push(format!("{name}: parameter {k} is not a parameter of {}", sub.name));
                }
                if let Some(keep) = keep.as_deref_mut() {
                    // Hierarchical import: the instance stays an instance of the subcircuit's
                    // cell, a variant cell when it passes other parameter values.
                    keep.push(XInst {
                        name: seg(name),
                        sub: sub.name.clone(),
                        env: child_env,
                        nets: nodes.iter().map(|n| net(n)).collect(),
                    });
                    continue;
                }
                let child_ports: HashMap<String, String> =
                    sub.pins.iter().zip(&nodes).map(|(p, n)| (p.to_ascii_lowercase(), net(n))).collect();
                self.expand(&sub.lines, &seg(name), &child_ports, &child_env, depth + 1, out, warnings, None)?;
                continue;
            }
            let Some(nn) = node_count(letter) else {
                warnings.push(format!("{name}: element type '{letter}' skipped (R, C, L, D, V, I, M and X are imported)"));
                continue;
            };
            let pos: Vec<&String> = t[1..].iter().filter(|s| !s.contains('=')).collect();
            if pos.len() < nn {
                return Err(format!("{name}: expected {nn} nodes"));
            }
            let mut l = format!("{letter}{}", seg(name));
            for n in &pos[..nn] {
                l += &format!(" {}", net(n));
            }
            for v in &pos[nn..] {
                // A value or parameter expression; a model name stays as written.
                l += &format!(" {}", eval_param(v, env).unwrap_or_else(|_| v.to_string()));
            }
            if matches!(letter, 'R' | 'C' | 'L') && pos.len() == nn {
                // CDL may give a model (`$[rppoly]`) and the size instead of a value.
                let (key, value) = match letter {
                    'R' => ("r", "1k"),
                    'C' => ("c", "1p"),
                    _ => ("l", "1n"),
                };
                if !t.iter().any(|s| s.to_ascii_lowercase().starts_with(&format!("{key}="))) {
                    warnings.push(format!(
                        "{}: no value in the CDL{}; set to {value}",
                        l.split_whitespace().next().unwrap_or(""),
                        model.as_ref().map_or(String::new(), |m| format!(" (model {m})"))
                    ));
                    l += &format!(" {value}");
                }
            }
            for kv in t[1..].iter().filter(|s| s.contains('=')) {
                let (k, v) = kv.split_once('=').expect("contains =");
                l += &format!(" {}={}", k.to_ascii_lowercase(), eval_param(v, env)?);
            }
            out.push(l);
        }
        Ok(())
    }
}

/// An instance of a subcircuit kept as a cell instance ([`import_cdl_hier`]).
#[derive(Debug, Clone)]
struct XInst {
    name: String,
    /// The subcircuit and its parameter values for this instance.
    sub: String,
    env: HashMap<String, f64>,
    nets: Vec<String>,
}

/// One cell of a hierarchical CDL import.
#[derive(Debug, Clone)]
pub struct CdlCell {
    /// The subcircuit name (the cell name).
    pub name: String,
    pub imported: Imported,
}

/// The result of [`import_cdl_hier`]: a cell per subcircuit under the top, children first.
#[derive(Debug, Clone, Default)]
pub struct CdlHier {
    pub top: String,
    pub cells: Vec<CdlCell>,
    pub warnings: Vec<String>,
}

/// Import a CDL netlist keeping its hierarchy: every `.SUBCKT` reachable from `top` (chosen as
/// in [`import_cdl`]) becomes a cell of library `lib` whose schematic holds its own devices and
/// an instance of the cell of each subcircuit it places, with the subcircuit's pins as
/// schematic pins. Subcircuits the file does not define (PDK devices) stay devices.
pub fn import_cdl_hier(text: &str, tech: &Tech, top: Option<&str>, lib: &str) -> Result<CdlHier, String> {
    let cdl = Cdl::parse(text)?;
    let top = match top {
        Some(t) => t.to_string(),
        None => cdl.tops().last().cloned().ok_or("the netlist has no .SUBCKT: import it flat")?,
    };
    let mut out =
        CdlHier { top: cdl.subckt(&top).ok_or_else(|| format!("no .SUBCKT {top}"))?.name.clone(), ..Default::default() };
    let ports_of = |s: &Subckt| -> Vec<(String, PortDir)> {
        s.pins.iter().map(|p| (p.clone(), PortDir::parse(s.dirs.get(p).map_or("", String::as_str)))).collect()
    };
    let sub = cdl.subckt(&out.top).expect("found");
    let mut env = HashMap::new();
    for (k, v) in &sub.params {
        env.insert(k.clone(), parse_spice(&eval_param(v, &env)?).unwrap_or(0.0));
    }
    let top_name = out.top.clone();
    let mut done: Vec<String> = vec![];
    build_cell(&cdl, tech, lib, &top_name, &top_name, &env, &ports_of, &mut done, &mut vec![], &mut out)?;
    Ok(out)
}

impl CdlHier {
    /// Store the cells in library `lib` of `p` (created when missing). An existing cell's
    /// schematic is replaced only with `overwrite`. Returns the cells, children first.
    pub fn add_to(&self, p: &mut crate::Project, lib: &str, overwrite: bool) -> Result<Vec<crate::CellRef>, String> {
        if !p.libraries.iter().any(|l| l.name == lib) {
            p.add_library(lib)?;
        }
        for c in &self.cells {
            let r = crate::CellRef::new(lib, &c.name);
            if p.cell(&r).is_some_and(|x| x.schematic.is_some()) && !overwrite {
                return Err(format!("{r} already has a schematic; overwrite to replace it"));
            }
        }
        let mut out = vec![];
        for c in &self.cells {
            let r = crate::CellRef::new(lib, &c.name);
            if p.cell(&r).is_none() {
                p.add_cell(lib, crate::Cell::new(&c.name))?;
            }
            p.cell_mut(&r).expect("added").schematic = Some(c.imported.schematic.clone());
            out.push(r);
        }
        Ok(out)
    }
}

/// Whether the netlist's subcircuits place each other (a hierarchy worth keeping).
pub fn cdl_is_hierarchical(text: &str) -> bool {
    Cdl::parse(text).is_ok_and(|c| {
        c.subckts
            .iter()
            .any(|s| s.lines.iter().any(|l| x_cell(&tokens(annotations(l).0)).is_some_and(|(_, x, _)| c.subckt(&x).is_some())))
    })
}

/// The parameter values of subcircuit `sub` as passed, or `None` when they are its defaults.
fn variant_suffix(sub: &Subckt, env: &HashMap<String, f64>) -> Option<String> {
    let mut defaults = HashMap::new();
    for (k, v) in &sub.params {
        defaults.insert(k.clone(), parse_spice(&eval_param(v, &defaults).ok()?).unwrap_or(0.0));
    }
    let parts: Vec<String> = sub
        .params
        .iter()
        .filter(|(k, _)| env.get(k).is_some_and(|v| (v - defaults[k]).abs() > 1e-12 * v.abs().max(1e-30)))
        // SPICE suffixes (`4u`), so the name stays ASCII.
        .map(|(k, _)| format!("{k}{}", crate::units::format_eng(env[k], "").replace('µ', "u")))
        .collect();
    (!parts.is_empty()).then(|| {
        parts
            .join("_")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c
                } else if c == '.' {
                    'p'
                } else {
                    '_'
                }
            })
            .collect()
    })
}

/// Build the cell `cell` of subcircuit `sub` with parameter values `env` (and first the cells
/// it places), appending them to `out` children first.
#[allow(clippy::too_many_arguments)]
fn build_cell(
    cdl: &Cdl,
    tech: &Tech,
    lib: &str,
    sub_name: &str,
    cell: &str,
    env: &HashMap<String, f64>,
    ports_of: &dyn Fn(&Subckt) -> Vec<(String, PortDir)>,
    done: &mut Vec<String>,
    path: &mut Vec<String>,
    out: &mut CdlHier,
) -> Result<(), String> {
    use crate::geom::{Orient, Pt};
    if path.iter().any(|p| p.eq_ignore_ascii_case(sub_name)) {
        return Err(format!("{sub_name} places itself (recursive subcircuits)"));
    }
    if done.iter().any(|d| d.eq_ignore_ascii_case(cell)) {
        return Ok(());
    }
    if !crate::project::valid_name(cell) {
        return Err(format!("subcircuit {sub_name}: '{cell}' is not a valid cell name (import it flat)"));
    }
    let sub = cdl.subckt(sub_name).ok_or_else(|| format!("no .SUBCKT {sub_name}"))?;
    let nets: HashMap<String, String> = sub.pins.iter().map(|p| (p.to_ascii_lowercase(), p.clone())).collect();
    let (mut lines, mut xs) = (vec![], vec![]);
    cdl.expand(&sub.lines, "", &nets, env, 0, &mut lines, &mut out.warnings, Some(&mut xs))?;
    path.push(sub_name.to_string());
    let mut children = vec![];
    for x in &xs {
        let child = cdl.subckt(&x.sub).expect("kept only local subcircuits");
        let name = match variant_suffix(child, &x.env) {
            Some(v) => format!("{}_{v}", child.name),
            None => child.name.clone(),
        };
        build_cell(cdl, tech, lib, &child.name, &name, &x.env, ports_of, done, path, out)?;
        children.push((name, ports_of(child), child.pins.clone()));
    }
    path.pop();
    let mut imported = if lines.is_empty() {
        Imported::default()
    } else {
        spice_import::import(&(lines.join("\n") + "\n"), tech).map_err(|e| format!("{}: {e}", sub.name))?
    };
    let sch = &mut imported.schematic;
    let base_y = sch.instances.iter().map(|i| i.pos.y).max().map_or(0, |y| y + 14);
    for (k, (x, (name, ports, pins))) in xs.iter().zip(children).enumerate() {
        let pos = Pt::new((k % 6) as i32 * 16, base_y + (k / 6) as i32 * 14);
        let id = sch.add_cell_instance(crate::project::CellRef::new(lib, &name), ports, pos, Orient::default());
        let inst = sch.instance_mut(id).expect("just added");
        inst.name = format!("X{}", x.name);
        let at: HashMap<&str, Pt> = inst.pin_positions().into_iter().collect();
        for (pin, net) in pins.iter().zip(&x.nets) {
            let Some(&p) = at.get(pin.as_str()) else { continue };
            if net == "0" || net.eq_ignore_ascii_case("gnd") {
                sch.add_instance(DeviceKind::Ground, p, Orient::default());
            } else {
                sch.add_label(net.clone(), p);
            }
        }
    }
    // Pins: label the ports no device or instance touches, then mark them all.
    let ports = ports_of(sub);
    let mut spare = Pt::new(-12, 0);
    for (pin, _) in &ports {
        if !sch.labels.iter().any(|l| l.name.eq_ignore_ascii_case(pin)) {
            sch.add_label(pin.clone(), spare);
            spare.y += 4;
        }
    }
    sch.mark_ports(&ports);
    done.push(cell.to_string());
    out.cells.push(CdlCell { name: cell.to_string(), imported });
    Ok(())
}

/// The result of [`import_cdl`].
#[derive(Debug, Clone, Default)]
pub struct CdlImport {
    pub imported: Imported,
    pub flat: Flat,
    /// Every subcircuit of the file.
    pub subckts: Vec<String>,
}

/// Import a CDL netlist as one flat schematic: from `top`, else the only subcircuit no other
/// instantiates (the last of them when there are several), else the elements outside any
/// subcircuit.
pub fn import_cdl(text: &str, tech: &Tech, top: Option<&str>) -> Result<CdlImport, String> {
    let cdl = Cdl::parse(text)?;
    let top = match top {
        Some(t) => Some(t.to_string()),
        None if !cdl.top.is_empty() => None,
        None => cdl.tops().last().cloned(),
    };
    let flat = cdl.flatten(top.as_deref())?;
    if flat.devices == 0 {
        return Err(format!("{} has no devices", flat.top));
    }
    let mut imported = spice_import::import(&flat.netlist, tech)?;
    imported.warnings.splice(0..0, flat.warnings.iter().cloned());
    let pins: Vec<(String, crate::schematic::PortDir)> = flat.pins.iter().map(|(n, d)| (n.clone(), PortDir::parse(d))).collect();
    imported.schematic.mark_ports(&pins);
    Ok(CdlImport { imported, subckts: cdl.subckts.iter().map(|s| s.name.clone()).collect(), flat })
}

/// Virtuoso symbols for the schematic's devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SymbolMap {
    #[serde(rename = "symbol", default)]
    pub symbols: Vec<SymbolDef>,
}

/// The Virtuoso symbol of one device kind (optionally only for one model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolDef {
    /// `nmos`, `pmos`, `resistor`, `capacitor`, `inductor`, `vsource`, `isource`, `diode`.
    pub kind: String,
    /// Only instances with this `model` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub lib: String,
    pub cell: String,
    #[serde(default = "default_view")]
    pub view: String,
    /// IC Studio pin -> Virtuoso terminal.
    pub terms: BTreeMap<String, String>,
    /// IC Studio parameter -> Virtuoso CDF parameter (unlisted parameters are not set).
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

fn default_view() -> String {
    "symbol".into()
}

impl SymbolMap {
    pub fn from_toml(text: &str) -> Result<SymbolMap, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// analogLib symbols.
    pub fn analog_lib() -> SymbolMap {
        let def = |kind: &str, cell: &str, terms: &[(&str, &str)], params: &[(&str, &str)]| SymbolDef {
            kind: kind.into(),
            model: None,
            lib: "analogLib".into(),
            cell: cell.into(),
            view: "symbol".into(),
            terms: terms.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            params: params.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        };
        let mos = [("d", "D"), ("g", "G"), ("s", "S"), ("b", "B")];
        let two = [("p", "PLUS"), ("n", "MINUS")];
        SymbolMap {
            symbols: vec![
                def("nmos", "nmos4", &mos, &[("model", "model"), ("w", "w"), ("l", "l"), ("m", "m")]),
                def("pmos", "pmos4", &mos, &[("model", "model"), ("w", "w"), ("l", "l"), ("m", "m")]),
                def("resistor", "res", &two, &[("r", "r")]),
                def("capacitor", "cap", &two, &[("c", "c")]),
                def("inductor", "ind", &two, &[("l", "l")]),
                def("vsource", "vdc", &two, &[("dc", "vdc"), ("ac", "acm")]),
                def("isource", "idc", &two, &[("dc", "idc"), ("ac", "acm")]),
                def("diode", "diode", &[("a", "PLUS"), ("k", "MINUS")], &[("model", "model")]),
            ],
        }
    }

    /// The symbol of an instance: a model-specific entry first, then the kind's.
    fn find(&self, kind: DeviceKind, model: &str) -> Option<&SymbolDef> {
        let k = kind_name(kind)?;
        let of_kind = || self.symbols.iter().filter(move |s| s.kind.eq_ignore_ascii_case(k));
        of_kind()
            .find(|s| s.model.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(model)))
            .or_else(|| of_kind().find(|s| s.model.is_none()))
    }
}

fn kind_name(k: DeviceKind) -> Option<&'static str> {
    Some(match k {
        // A 3-terminal MOS is written as the 4-terminal symbol with its body on the bulk net.
        DeviceKind::Nmos | DeviceKind::Nmos3 => "nmos",
        DeviceKind::Pmos | DeviceKind::Pmos3 => "pmos",
        DeviceKind::Resistor => "resistor",
        DeviceKind::Capacitor => "capacitor",
        DeviceKind::Inductor => "inductor",
        DeviceKind::VSource => "vsource",
        DeviceKind::ISource => "isource",
        DeviceKind::Diode => "diode",
        DeviceKind::Ground | DeviceKind::Supply | DeviceKind::Cell => return None,
    })
}

fn skill_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Virtuoso orientation of an IC Studio orientation. IC Studio mirrors about the y axis, then
/// rotates clockwise as seen on screen (y down); Virtuoso rotates counter-clockwise (y up). So
/// k clockwise quarter turns are R(360 - 90k), and mirrored ones MY then that rotation
/// (MY R180 = MX, MY R270 = MXR90).
fn orient_code(o: crate::geom::Orient) -> &'static str {
    match (o.mirror, o.rot % 4) {
        (false, 0) => "R0",
        (false, 1) => "R270",
        (false, 2) => "R180",
        (false, _) => "R90",
        (true, 0) => "MY",
        (true, 1) => "MXR90",
        (true, 2) => "MX",
        (true, _) => "MYR90",
    }
}

/// A SKILL script that creates `lib`/`cell` schematic in Virtuoso from `sch`; `scale` is the
/// Virtuoso distance (user units) of one schematic grid point. Returns the script and warnings
/// for devices without a symbol.
pub fn skill_schematic(sch: &Schematic, lib: &str, cell: &str, map: &SymbolMap, scale: f64) -> (String, Vec<String>) {
    let conn = connectivity::extract(sch);
    let mut warnings = vec![];
    let mut s = String::new();
    s += &format!(
        ";; {lib}/{cell} schematic, written by IC Studio. Load it in the CIW: load(\"{cell}.il\")\n\
         ;; Every terminal gets a wire stub and a label with its net at the pin of the symbol, so the\n\
         ;; connectivity does not depend on where the symbols put their pins.\n\n"
    );
    s += r#"procedure(icsConnect(cv inst term net)
  let((t box pt c dx dy end)
    t = car(setof(x inst~>master~>terminals x~>name == term))
    when(t && t~>pins
      box = dbTransformBBox(car(t~>pins)~>fig~>bBox inst~>transform)
      pt = centerBox(box)
      ;; Stub away from the instance centre.
      c = centerBox(inst~>bBox)
      dx = xCoord(pt) - xCoord(c)
      dy = yCoord(pt) - yCoord(c)
      end = if(abs(dx) >= abs(dy)
        then list(xCoord(pt) + if(dx >= 0 0.125 -0.125) yCoord(pt))
        else list(xCoord(pt) yCoord(pt) + if(dy >= 0 0.125 -0.125)))
      schCreateWire(cv "draw" "full" list(pt end) 0.0625 0.0625 0.0)
      schCreateWireLabel(cv nil end net "lowerCenter" "R0" "stick" 0.0625 nil))))

"#;
    s += "let((cv master inst)\n";
    s += &format!("  cv = dbOpenCellViewByType({} {} \"schematic\" \"schematic\" \"w\")\n", skill_str(lib), skill_str(cell));
    s += &format!("  unless(cv error(\"cannot open {lib}/{cell} schematic for writing\"))\n");
    let mut masters: HashMap<String, usize> = HashMap::new();
    for inst in &sch.instances {
        if matches!(inst.kind, DeviceKind::Ground | DeviceKind::Supply) {
            continue;
        }
        if inst.kind == DeviceKind::Cell {
            // A placed cell: the symbol of that cell in the target library, made from its
            // schematic's pins when it has none yet (export the placed cells first).
            let Some(c) = &inst.cell else { continue };
            let key = format!("{lib}/{}/symbol", c.cell);
            if !masters.contains_key(&key) {
                let k = masters.len();
                masters.insert(key.clone(), k);
                let (l, n) = (skill_str(lib), skill_str(&c.cell));
                s += &format!(
                    "  unless(ddGetObj({l} {n} \"symbol\")\n    schPinListToSymbol({l} {n} \"symbol\" schSchemToPinList({l} {n} \"schematic\")))\n\
                     \x20 master{k} = dbOpenCellViewByType({l} {n} \"symbol\")\n  unless(master{k} error(\"no symbol {key}: export {} first\"))\n",
                    c.cell
                );
            }
            let k = masters[&key];
            let (x, y) = (inst.pos.x as f64 * scale, -(inst.pos.y as f64) * scale);
            s += &format!(
                "  inst = dbCreateInst(cv master{k} {} list({x} {y}) {})\n",
                skill_str(&inst.name),
                skill_str(orient_code(inst.orient))
            );
            for (pin, _) in &inst.ports {
                let net = conn.net_of(inst.id, pin).unwrap_or("").to_string();
                let net = if net == "0" || net.eq_ignore_ascii_case("gnd") { "gnd!".to_string() } else { net };
                if net.is_empty() {
                    warnings.push(format!("{}.{pin}: unconnected", inst.name));
                    continue;
                }
                s += &format!("  icsConnect(cv inst {} {})\n", skill_str(pin), skill_str(&net));
            }
            continue;
        }
        let Some(sym) = map.find(inst.kind, inst.param("model")) else {
            warnings.push(format!("{}: no Virtuoso symbol for {}", inst.name, inst.kind.label()));
            continue;
        };
        let key = format!("{}/{}/{}", sym.lib, sym.cell, sym.view);
        if !masters.contains_key(&key) {
            let k = masters.len();
            masters.insert(key.clone(), k);
            s += &format!(
                "  master{k} = dbOpenCellViewByType({} {} {})\n  unless(master{k} error(\"no symbol {key}\"))\n",
                skill_str(&sym.lib),
                skill_str(&sym.cell),
                skill_str(&sym.view)
            );
        }
        let k = masters[&key];
        let (x, y) = (inst.pos.x as f64 * scale, -(inst.pos.y as f64) * scale);
        s += &format!(
            "  inst = dbCreateInst(cv master{k} {} list({x} {y}) {})\n",
            skill_str(&inst.name),
            skill_str(orient_code(inst.orient))
        );
        for (ours, theirs) in &sym.params {
            let v = inst.param(ours);
            if !v.is_empty() {
                s += &format!("  dbReplaceProp(inst {} \"string\" {})\n", skill_str(theirs), skill_str(v));
            }
        }
        let body = inst.kind.implicit_bulk().then_some("b");
        for pin in inst.pin_positions().into_iter().map(|(p, _)| p).chain(body) {
            let Some(term) = sym.terms.get(pin) else { continue };
            let net = conn.net_of(inst.id, pin).unwrap_or("").to_string();
            let net = if net == "0" || net.eq_ignore_ascii_case("gnd") { "gnd!".to_string() } else { net };
            if net.is_empty() {
                warnings.push(format!("{}.{pin}: unconnected", inst.name));
                continue;
            }
            s += &format!("  icsConnect(cv inst {} {})\n", skill_str(term), skill_str(&net));
        }
    }
    // Schematic pins: basic ipin / opin / iopin at the pin label, joined to the net by name.
    for (k, l) in sch.labels.iter().filter(|l| l.port.is_some()).enumerate() {
        if sch.labels.iter().take_while(|o| o.id != l.id).any(|o| o.port.is_some() && o.name == l.name) {
            continue;
        }
        let dir = l.port.expect("filtered");
        let sym = match dir {
            PortDir::Input => "ipin",
            PortDir::Output => "opin",
            PortDir::InOut => "iopin",
        };
        let (x, y) = (l.pos.x as f64 * scale, -(l.pos.y as f64) * scale);
        s += &format!(
            "  schCreatePin(cv dbOpenCellViewByType(\"basic\" \"{sym}\" \"symbol\") {} {} nil list({} {y}) \"R0\") ; pin {k}\n",
            skill_str(&l.name),
            skill_str(dir.name()),
            x - 0.25
        );
    }
    s += "  schCheck(cv)\n  dbSave(cv)\n  dbClose(cv)\n  printf(\"IC Studio: wrote %s/%s schematic\\n\" ";
    s += &format!("{} {}))\n", skill_str(lib), skill_str(cell));
    // Declare the master variables in the let list.
    let decl: Vec<String> = (0..masters.len()).map(|k| format!(" master{k}")).collect();
    let s = s.replacen("let((cv master inst)", &format!("let((cv master inst{})", decl.concat()), 1);
    (s, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CDL: &str = r#"
* auCdl netlist
.GLOBAL vdd! gnd!
.SUBCKT inv in out wp=2u
*.PININFO in:I out:O
MM1 out in vdd! vdd! pch W=wp L=180n M=1
MM0 out in gnd! gnd! nch W='wp/2' L=180n M=1
.ENDS
.SUBCKT buf a y
*.PININFO a:I y:O
XI0 a mid / inv wp=4u
XI1 mid y inv
RR0 y gnd! $[rppoly] $W=1u $L=10u
CC0 mid gnd! 10f $[mimcap]
.ENDS
"#;

    #[test]
    fn hierarchical_cdl_keeps_its_hierarchy() {
        let h = import_cdl_hier(CDL, &Tech::generic(), None, "work").unwrap();
        assert_eq!(h.top, "buf");
        let names: Vec<&str> = h.cells.iter().map(|c| c.name.as_str()).collect();
        // XI0 passes wp=4u: a variant cell; XI1 uses the default inv. Children come first.
        assert_eq!(names.len(), 3, "{names:?}");
        assert_eq!(names[2], "buf");
        assert!(names.contains(&"inv") && names.iter().any(|n| n.starts_with("inv_wp4")), "{names:?}");
        let buf = &h.cells[2].imported.schematic;
        let xs: Vec<_> = buf.instances.iter().filter(|i| i.kind == DeviceKind::Cell).collect();
        assert_eq!(xs.len(), 2);
        assert_eq!(buf.ports().len(), 2);
        // Flattened again through a project, the circuit equals the flat import.
        let mut p = crate::Project::new("h");
        for c in &h.cells {
            let mut cell = crate::Cell::new(&c.name);
            cell.schematic = Some(c.imported.schematic.clone());
            p.add_cell("work", cell).unwrap();
        }
        let flat = p.circuit(&crate::CellRef::new("work", "buf")).unwrap();
        let ws: Vec<String> = flat.instances.iter().filter(|i| i.kind.is_mos()).map(|i| i.param("w").to_string()).collect();
        assert_eq!(ws.len(), 4);
        assert!(ws.iter().any(|w| parse_spice(w) == Some(4e-6)), "{ws:?}");
        let conn = connectivity::extract(&flat);
        let m = flat.instances.iter().find(|i| i.name == "XI0/MM1").expect("XI0/MM1");
        let mid = conn.net_of(m.id, "d").unwrap().to_string();
        let m2 = flat.instances.iter().find(|i| i.name == "XI1/MM1").expect("XI1/MM1");
        assert_eq!(conn.net_of(m2.id, "g"), Some(mid.as_str()), "XI0's output drives XI1");
    }

    #[test]
    fn skill_places_cell_instances() {
        let p = crate::examples::demo_project();
        let sch = p.cell(&crate::CellRef::new("work", "buffer_tb")).unwrap().schematic.as_ref().unwrap();
        let (skill, warnings) = skill_schematic(sch, "myLib", "buffer_tb", &SymbolMap::analog_lib(), 0.1875);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(skill.contains(r#"schPinListToSymbol("myLib" "inv" "symbol""#), "{skill}");
        assert_eq!(skill.matches(r#"dbCreateInst(cv master"#).count(), 5, "two inv, a capacitor, two sources");
        assert!(skill.contains(r#"icsConnect(cv inst "y" "out")"#), "{skill}");
    }

    #[test]
    fn hierarchical_cdl_flattens() {
        let cdl = Cdl::parse(CDL).unwrap();
        assert_eq!(cdl.subckts.len(), 2);
        assert_eq!(cdl.tops(), ["buf"]);
        assert_eq!(cdl.subckt("inv").unwrap().dirs["out"], "O");
        let f = cdl.flatten(Some("buf")).unwrap();
        assert_eq!(f.devices, 6, "{}", f.netlist);
        assert_eq!(f.pins, [("a".to_string(), "I".to_string()), ("y".to_string(), "O".to_string())]);
        // Instance paths, local nets, globals, parameters passed and evaluated.
        assert!(f.netlist.contains("MI0/M1 mid a vdd vdd pch w=4e-6 l=180n m=1"), "{}", f.netlist);
        assert!(f.netlist.contains("MI0/M0 mid a gnd gnd nch w=2e-6"), "{}", f.netlist);
        assert!(f.netlist.contains("MI1/M1 y mid vdd vdd pch w=2e-6"), "{}", f.netlist);
        assert!(f.netlist.contains("RR0 y gnd 1k") && f.warnings.iter().any(|w| w.contains("rppoly")), "{f:?}");
        assert!(f.netlist.contains("CC0 mid gnd 10f"));
        let imp = import_cdl(CDL, &Tech::generic(), None).unwrap();
        assert_eq!(imp.flat.top, "buf");
        let sch = &imp.imported.schematic;
        assert_eq!(sch.instances.iter().filter(|i| i.kind.is_mos()).count(), 4);
        let conn = connectivity::extract(sch);
        let m = sch.instance_by_name("MI0/M1").unwrap();
        assert_eq!(conn.net_of(m.id, "d"), Some("mid"));
        assert!(Cdl::parse(".SUBCKT a x\nM1 x x 0 0 nch\n").is_err());
        assert!(
            import_cdl(".SUBCKT a x\nXI0 x / b\n.ENDS\n.SUBCKT b y\nXI0 y / a\n.ENDS\n", &Tech::generic(), Some("a")).is_err()
        );
    }

    #[test]
    fn schematic_as_skill() {
        let p = crate::examples::demo_project();
        let c = p.cell(&crate::CellRef::new("work", "cmos_inverter")).unwrap();
        let sch = c.schematic.as_ref().unwrap();
        let (il, warnings) = skill_schematic(sch, "work", "cmos_inverter", &SymbolMap::analog_lib(), 0.0625);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(il.contains("dbOpenCellViewByType(\"analogLib\" \"nmos4\" \"symbol\")"), "{il}");
        assert!(il.contains("dbReplaceProp(inst \"w\" \"string\""));
        // Every MOS terminal is connected by name; ground becomes gnd!.
        let terms: usize = sch
            .instances
            .iter()
            .map(|i| match i.kind {
                DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => 4,
                DeviceKind::Ground | DeviceKind::Supply => 0,
                _ => 2,
            })
            .sum();
        assert_eq!(il.matches("  icsConnect(cv inst ").count(), terms);
        assert_eq!(orient_code(crate::geom::Orient { rot: 1, mirror: false }), "R270");
        assert_eq!(orient_code(crate::geom::Orient { rot: 2, mirror: true }), "MX");
        assert!(il.contains("\"gnd!\""));
        // The script is complete SKILL (balanced, parses).
        assert!(crate::skill::is_complete(&il));
        // A PDK map: model-specific symbols.
        let map = SymbolMap::from_toml(
            "[[symbol]]\nkind = \"nmos\"\nmodel = \"nmos\"\nlib = \"gpdk180\"\ncell = \"nmos1v\"\nterms = { d = \"D\", g = \"G\", s = \"S\", b = \"B\" }\nparams = { w = \"w\", l = \"l\" }\n",
        )
        .unwrap();
        let (il, warnings) = skill_schematic(sch, "work", "inv", &map, 0.0625);
        assert!(il.contains("\"gpdk180\" \"nmos1v\""));
        assert!(warnings.iter().any(|w| w.contains("PMOS")), "{warnings:?}");
    }

    /// IC Studio's CDL out, read back: the same devices on the same nets.
    #[test]
    fn cdl_round_trip() {
        let p = crate::examples::demo_project();
        for cell in ["cmos_inverter", "ota5t", "nand2"] {
            let r = crate::CellRef::new("work", cell);
            let Some(c) = p.cell(&r) else { continue };
            let sch = c.schematic.as_ref().unwrap();
            let (cdl, _) = crate::netlist::write(sch, &p.tech, &c.sim, &r.to_string(), crate::netlist::Format::Cdl);
            let imp = import_cdl(&cdl, &p.tech, None).unwrap_or_else(|e| panic!("{cell}: {e}\n{cdl}"));
            let back = &imp.imported.schematic;
            let (a, b) = (connectivity::extract(sch), connectivity::extract(back));
            let mos = |s: &Schematic| s.instances.iter().filter(|i| i.kind.is_mos()).count();
            assert_eq!(mos(sch), mos(back), "{cell}");
            for i in sch.instances.iter().filter(|i| i.kind.is_mos()) {
                let j = back
                    .instances
                    .iter()
                    .find(|j| j.name.eq_ignore_ascii_case(&i.name) || j.name.ends_with(&i.name))
                    .unwrap_or_else(|| panic!("{cell}: {} missing", i.name));
                for pin in ["d", "g", "s", "b"] {
                    let (na, nb) = (a.net_of(i.id, pin).unwrap_or(""), b.net_of(j.id, pin).unwrap_or(""));
                    let same = na.eq_ignore_ascii_case(nb) || (["0", "gnd"].contains(&na) && ["0", "gnd"].contains(&nb));
                    assert!(same, "{cell} {}.{pin}: {na} vs {nb}", i.name);
                }
                assert_eq!(i.param("w"), j.param("w"), "{cell} {}", i.name);
            }
        }
    }

    /// Schematic pins survive CDL export and import, and become Virtuoso pins in SKILL.
    #[test]
    fn schematic_pins_round_trip() {
        let p = crate::examples::demo_project();
        let r = p.all_cells().map(|(r, _)| r).find(|r| r.cell == "cmos_inverter").expect("inverter");
        let mut sch = p.cell(&r).and_then(|c| c.schematic.clone()).expect("schematic");
        let names: Vec<String> = sch.labels.iter().map(|l| l.name.clone()).collect();
        assert!(names.iter().any(|n| n == "in") && names.iter().any(|n| n == "out"), "{names:?}");
        let marked = sch.mark_ports(&[("in".into(), PortDir::Input), ("out".into(), PortDir::Output)]);
        assert_eq!(marked, 2);
        let cdl = crate::signoff::cdl(&sch, &p.tech, "inv");
        // Only the pins are ports (as Virtuoso writes it), in the LVS netlist too.
        assert!(cdl.contains(".subckt inv in out\n*.PININFO in:I out:O"), "{cdl}");
        let back = import_cdl(&cdl, &p.tech, Some("inv")).expect("import");
        assert_eq!(
            back.imported.schematic.ports(),
            vec![("in".to_string(), PortDir::Input), ("out".to_string(), PortDir::Output)]
        );
        let (il, _) = skill_schematic(&sch, "lib", "inv", &SymbolMap::analog_lib(), 0.0625);
        assert!(il.contains("schCreatePin(cv dbOpenCellViewByType(\"basic\" \"ipin\" \"symbol\") \"in\" \"input\""), "{il}");
    }
}
