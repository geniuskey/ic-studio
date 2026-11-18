//! Virtuoso libraries without OpenAccess: the dump that [`DUMP_SKILL`] (`icsDump.il`) writes
//! from inside Virtuoso, read into IC Studio cells.
//!
//! - **Schematics**: every dumped schematic becomes a cell. Instances are matched to IC Studio
//!   devices through a [`SymbolMap`] (analogLib by default); instances of other dumped
//!   schematics become cell instances (a hierarchical schematic, [`crate::sch_hier`]); an
//!   instance passing other CDF parameter values places a variant cell of its own. The net on each terminal comes from Virtuoso's own connectivity, so
//!   wires are not needed: every pin gets a net label (or a ground symbol). A schematic without
//!   subcircuits keeps the Virtuoso placement and orientation (`scale` user units per grid
//!   point) when that gives the same connectivity; otherwise its devices are laid out in rows.
//!   Iterated instances and pin symbols are skipped; `pPar("x")` parameters refer to the
//!   cell's own CDF parameter `x`.
//! - **Layouts**: rectangles, polygons, paths and labels on the technology's layers (a purpose
//!   other than `drawing` looks for `layer.purpose` first), instances and mosaics. Masters from
//!   other libraries and PCell variants come in as cells of their own libraries.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::cadence::{SymbolDef, SymbolMap, import_cdl, import_cdl_hier};
use crate::connectivity;
use crate::geom::{IRect, Orient, Pt};
use crate::layout::{ArraySpec, Geometry, Layout, Trans};
use crate::project::{Cell, CellRef, Project, valid_name};
use crate::schematic::{DeviceKind, Schematic};
use crate::tech::Tech;
use crate::units::parse_spice;

/// The SKILL script that writes a dump: `load("icsDump.il")`, then
/// `icsDumpLib("myLib" "/tmp/myLib.icsdump")`.
pub const DUMP_SKILL: &str = include_str!("../skill/icsDump.il");

/// A value of the dump: a string, a number or a list.
#[derive(Debug, Clone, PartialEq)]
enum V {
    S(String),
    N(f64),
    L(Vec<V>),
}

impl V {
    fn items(&self) -> &[V] {
        match self {
            V::L(v) => v,
            _ => &[],
        }
    }
    fn str(&self) -> String {
        match self {
            V::S(s) => s.clone(),
            V::N(x) => format!("{x}"),
            V::L(_) => String::new(),
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            V::N(x) => Some(*x),
            V::S(s) => s.parse().ok(),
            V::L(_) => None,
        }
    }
    fn pt(&self) -> Option<(f64, f64)> {
        match self.items() {
            [x, y] => Some((x.num()?, y.num()?)),
            _ => None,
        }
    }
    fn pts(&self) -> Option<Vec<(f64, f64)>> {
        self.items().iter().map(V::pt).collect()
    }
    fn pairs(&self) -> Vec<(String, String)> {
        self.items()
            .iter()
            .filter_map(|p| match p.items() {
                [k, v] => Some((k.str(), v.str())),
                _ => None,
            })
            .collect()
    }
}

/// Parse SKILL data: `( ... )` lists, `"strings"` with escapes, numbers and bare words.
fn parse(text: &str) -> Result<Vec<V>, String> {
    let b = text.as_bytes();
    let mut stack: Vec<Vec<V>> = vec![vec![]];
    let (mut i, mut line) = (0, 1);
    while i < b.len() {
        match b[i] {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' => {
                stack.push(vec![]);
                i += 1;
            }
            b')' => {
                let done = stack.pop().filter(|_| !stack.is_empty()).ok_or_else(|| format!("line {line}: unbalanced ')'"))?;
                stack.last_mut().expect("checked").push(V::L(done));
                i += 1;
            }
            b'"' => {
                let mut s = Vec::new();
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                        s.push(match b[i] {
                            b'n' => b'\n',
                            b't' => b'\t',
                            c => c,
                        });
                    } else {
                        line += (b[i] == b'\n') as usize;
                        s.push(b[i]);
                    }
                    i += 1;
                }
                if i >= b.len() {
                    return Err(format!("line {line}: unterminated string"));
                }
                i += 1;
                stack.last_mut().expect("root").push(V::S(String::from_utf8_lossy(&s).into_owned()));
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'(' | b')' | b'"' | b';') {
                    i += 1;
                }
                let w = &text[start..i];
                stack.last_mut().expect("root").push(match w.parse::<f64>() {
                    Ok(x) => V::N(x),
                    Err(_) => V::S(w.to_string()),
                });
            }
        }
    }
    if stack.len() != 1 {
        return Err("unbalanced '(' at the end of the dump".into());
    }
    Ok(stack.pop().expect("root"))
}

/// A schematic instance of the dump.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchInst {
    pub name: String,
    pub lib: String,
    pub cell: String,
    pub view: String,
    pub xy: (f64, f64),
    pub orient: String,
    /// Instances of an iterated instance (`I0<0:3>` is 4).
    pub count: u32,
    pub params: Vec<(String, String)>,
    /// Terminal -> net.
    pub conns: Vec<(String, String)>,
}

/// A schematic view of the dump.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchView {
    pub lib: String,
    pub cell: String,
    /// Terminals with Virtuoso directions (`input`, `output`, `inputOutput`).
    pub terms: Vec<(String, String)>,
    /// CDF parameters with their defaults.
    pub cdf: Vec<(String, String)>,
    pub insts: Vec<SchInst>,
}

/// A layout shape of the dump, in user units.
#[derive(Debug, Clone, PartialEq)]
pub enum LShape {
    Rect { layer: String, purpose: String, a: (f64, f64), b: (f64, f64) },
    Poly { layer: String, purpose: String, pts: Vec<(f64, f64)> },
    Path { layer: String, purpose: String, width: f64, pts: Vec<(f64, f64)> },
    Label { layer: String, purpose: String, text: String, at: (f64, f64), height: f64 },
}

/// A layout instance (a mosaic when `mosaic` is `(rows, columns, uX, uY)`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LInst {
    pub name: String,
    pub lib: String,
    pub cell: String,
    pub xy: (f64, f64),
    pub orient: String,
    pub mosaic: Option<(u32, u32, f64, f64)>,
}

/// A layout view of the dump.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayView {
    pub lib: String,
    pub cell: String,
    pub dbu_per_uu: f64,
    pub shapes: Vec<LShape>,
    pub insts: Vec<LInst>,
    /// Shapes of kinds that are not imported (`ellipse`, `donut`, ...), by kind.
    pub skipped: BTreeMap<String, usize>,
}

/// A parsed dump.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dump {
    pub lib: String,
    pub schematics: Vec<SchView>,
    pub layouts: Vec<LayView>,
}

impl Dump {
    pub fn parse(text: &str) -> Result<Dump, String> {
        let recs = parse(text)?;
        let mut d = Dump::default();
        let mut cur: Option<bool> = None; // Some(true): schematic, Some(false): layout
        let mut saw_header = false;
        for (k, r) in recs.iter().enumerate() {
            let it = r.items();
            let head = it.first().map(V::str).unwrap_or_default();
            let a = |i: usize| it.get(i).map(V::str).unwrap_or_default();
            let bad = || format!("record {}: cannot read ({head} ...)", k + 1);
            match head.as_str() {
                "icsdump" => {
                    saw_header = true;
                    d.lib = a(2);
                }
                "end" => break,
                "sch" => {
                    d.schematics.push(SchView { lib: a(1), cell: a(2), ..Default::default() });
                    cur = Some(true);
                }
                "lay" => {
                    let dbu = it.get(3).and_then(V::num).unwrap_or(1000.0);
                    d.layouts.push(LayView { lib: a(1), cell: a(2), dbu_per_uu: dbu, ..Default::default() });
                    cur = Some(false);
                }
                "term" | "cdf" | "inst" if cur == Some(true) => {
                    let s = d.schematics.last_mut().expect("in a schematic");
                    match head.as_str() {
                        "term" => s.terms.push((a(1), a(2))),
                        "cdf" => s.cdf.push((a(1), a(2))),
                        _ => s.insts.push(SchInst {
                            name: a(1),
                            lib: a(2),
                            cell: a(3),
                            view: a(4),
                            xy: it.get(5).and_then(V::pt).ok_or_else(bad)?,
                            orient: a(6),
                            count: it.get(7).and_then(V::num).unwrap_or(1.0) as u32,
                            params: it.get(8).map(V::pairs).unwrap_or_default(),
                            conns: it.get(9).map(V::pairs).unwrap_or_default(),
                        }),
                    }
                }
                "rect" | "poly" | "path" | "label" | "linst" | "mosaic" | "skip" if cur == Some(false) => {
                    let l = d.layouts.last_mut().expect("in a layout");
                    let (layer, purpose) = (a(1), a(2));
                    match head.as_str() {
                        "rect" => {
                            let bb = it.get(3).ok_or_else(bad)?;
                            let (a, b) = match bb.items() {
                                [p, q] if p.pt().is_some() => (p.pt().ok_or_else(bad)?, q.pt().ok_or_else(bad)?),
                                [x0, y0, x1, y1] => (
                                    (x0.num().ok_or_else(bad)?, y0.num().ok_or_else(bad)?),
                                    (x1.num().ok_or_else(bad)?, y1.num().ok_or_else(bad)?),
                                ),
                                _ => return Err(bad()),
                            };
                            l.shapes.push(LShape::Rect { layer, purpose, a, b });
                        }
                        "poly" => {
                            let pts = it.get(3).and_then(V::pts).ok_or_else(bad)?;
                            l.shapes.push(LShape::Poly { layer, purpose, pts });
                        }
                        "path" => {
                            let width = it.get(3).and_then(V::num).ok_or_else(bad)?;
                            let pts = it.get(4).and_then(V::pts).ok_or_else(bad)?;
                            l.shapes.push(LShape::Path { layer, purpose, width, pts });
                        }
                        "label" => {
                            let at = it.get(4).and_then(V::pt).ok_or_else(bad)?;
                            let height = it.get(5).and_then(V::num).unwrap_or(0.1);
                            l.shapes.push(LShape::Label { layer, purpose, text: a(3), at, height });
                        }
                        "skip" => *l.skipped.entry(a(1)).or_default() += 1,
                        _ => {
                            let mosaic = if head == "mosaic" {
                                let n = |i: usize| it.get(i).and_then(V::num).ok_or_else(bad);
                                Some((n(6)? as u32, n(7)? as u32, n(8)?, n(9)?))
                            } else {
                                None
                            };
                            l.insts.push(LInst {
                                name: a(1),
                                lib: a(2),
                                cell: a(3),
                                xy: it.get(4).and_then(V::pt).ok_or_else(bad)?,
                                orient: a(5),
                                mosaic,
                            });
                        }
                    }
                }
                _ => return Err(format!("record {}: unexpected ({head} ...)", k + 1)),
            }
        }
        if !saw_header {
            return Err("not an IC Studio dump (no (\"icsdump\" ...) header); write it with icsDumpLib in Virtuoso".into());
        }
        Ok(d)
    }
}

/// One imported cell.
#[derive(Debug, Clone, PartialEq)]
pub struct CellImport {
    pub cell: CellRef,
    pub schematic: Option<Schematic>,
    pub layout: Option<Layout>,
    /// Devices of the (flattened) schematic.
    pub devices: usize,
    /// The schematic keeps the Virtuoso placement.
    pub placed: bool,
}

/// The result of [`import_dump`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VirtuosoImport {
    pub cells: Vec<CellImport>,
    pub warnings: Vec<String>,
}

/// IC Studio orientation of a Virtuoso one (the inverse of the SKILL export's).
fn sch_orient(code: &str) -> Orient {
    let (mirror, rot) = match code {
        "R270" => (false, 1),
        "R180" => (false, 2),
        "R90" => (false, 3),
        "MY" => (true, 0),
        "MXR90" => (true, 1),
        "MX" => (true, 2),
        "MYR90" => (true, 3),
        _ => (false, 0),
    };
    Orient { rot, mirror }
}

/// Layout transform of a Virtuoso orientation: mirror about the x axis, then rotate.
fn lay_trans(code: &str, x: i64, y: i64) -> Trans {
    let (rot, mirror) = match code {
        "R90" => (1, false),
        "R180" => (2, false),
        "R270" => (3, false),
        "MX" => (0, true),
        "MY" => (2, true),
        "MXR90" => (1, true),
        "MYR90" => (3, true),
        _ => (0, false),
    };
    Trans::with(x, y, rot, mirror)
}

fn kind_of(name: &str) -> Option<DeviceKind> {
    Some(match name.to_ascii_lowercase().as_str() {
        "nmos" => DeviceKind::Nmos,
        "pmos" => DeviceKind::Pmos,
        "resistor" => DeviceKind::Resistor,
        "capacitor" => DeviceKind::Capacitor,
        "inductor" => DeviceKind::Inductor,
        "vsource" => DeviceKind::VSource,
        "isource" => DeviceKind::ISource,
        "diode" => DeviceKind::Diode,
        _ => return None,
    })
}

fn pin_order(kind: DeviceKind) -> &'static [&'static str] {
    match kind {
        DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => &["d", "g", "s", "b"],
        DeviceKind::Diode => &["a", "k"],
        _ => &["p", "n"],
    }
}

/// The symbol of a Virtuoso instance: a definition for its lib/cell whose model matches, else
/// the first one for it.
fn symbol_for<'a>(map: &'a SymbolMap, inst: &SchInst) -> Option<&'a SymbolDef> {
    let cands: Vec<&SymbolDef> = map
        .symbols
        .iter()
        .filter(|s| {
            s.lib.eq_ignore_ascii_case(&inst.lib) && s.cell.eq_ignore_ascii_case(&inst.cell) && kind_of(&s.kind).is_some()
        })
        .collect();
    let model = inst.params.iter().find(|(k, _)| k == "model").map(|(_, v)| v.as_str()).unwrap_or("");
    cands.iter().find(|s| s.model.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(model))).or_else(|| cands.first()).copied()
}

/// Pin, supply and no-connect symbols: their nets are already on the instance terminals.
fn is_connector(inst: &SchInst) -> bool {
    inst.lib == "basic"
        || ["gnd", "vdd", "vcc", "vss", "vee", "noConn", "vdc_gnd"].iter().any(|c| inst.cell.eq_ignore_ascii_case(c))
}

/// A CDF value for CDL: `pPar("x")` refers to the parent's parameter `x`; spaces dropped.
fn cdf_value(v: &str) -> String {
    let mut s = v.trim().to_string();
    while let Some(i) = s.find("pPar(") {
        let rest = &s[i + 5..];
        let Some(j) = rest.find(')') else { break };
        let name = rest[..j].trim().trim_matches('"').to_string();
        s = format!("{}{}{}", &s[..i], name, &rest[j + 1..]);
    }
    s.split_whitespace().collect::<String>()
}

/// The CDL element name of a Virtuoso instance, as auCdl writes it: the element letter and
/// the instance name (`MM0` for `M0`), so a flattened device is `MI0/M0`.
fn element_name(letter: char, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
    format!("{letter}{clean}")
}

/// Net names for CDL: spaces would split the line.
fn net_name(n: &str) -> String {
    n.trim().replace(char::is_whitespace, "_")
}

fn cdl_dir(d: &str) -> &'static str {
    match d {
        "input" => "I",
        "output" => "O",
        _ => "B",
    }
}

/// The CDL of every dumped schematic, one subcircuit each. Returns the text and, per cell, the
/// Virtuoso instance behind each element name (lowercase) and whether it has subcircuits.
type CdlOut = (String, HashMap<String, (HashMap<String, SchInst>, bool)>);

fn dump_cdl(dump: &Dump, map: &SymbolMap, warnings: &mut Vec<String>) -> CdlOut {
    let by_cell: HashMap<String, &SchView> = dump.schematics.iter().map(|s| (s.cell.to_ascii_lowercase(), s)).collect();
    let mut text = String::from("* CDL from a Virtuoso dump (IC Studio)\n.GLOBAL gnd! vdd!\n");
    let mut info = HashMap::new();
    for s in &dump.schematics {
        let pins: Vec<String> = s.terms.iter().map(|(n, _)| net_name(n)).collect();
        let params: Vec<String> = s
            .cdf
            .iter()
            .filter(|(_, v)| parse_spice(&cdf_value(v)).is_some())
            .map(|(k, v)| format!("{}={}", k.to_ascii_lowercase(), cdf_value(v)))
            .collect();
        text += &format!("\n.SUBCKT {} {} {}\n", s.cell, pins.join(" "), params.join(" "));
        if !s.terms.is_empty() {
            let info: Vec<String> = s.terms.iter().map(|(n, d)| format!("{}:{}", net_name(n), cdl_dir(d))).collect();
            text += &format!("*.PININFO {}\n", info.join(" "));
        }
        let mut elems: HashMap<String, SchInst> = HashMap::new();
        let mut hier = false;
        let at = |inst: &SchInst| format!("{}/{} {}", s.lib, s.cell, inst.name);
        for inst in &s.insts {
            if is_connector(inst) {
                continue;
            }
            if inst.count > 1 || inst.name.contains('<') {
                warnings.push(format!("{}: iterated instance skipped ({}/{})", at(inst), inst.lib, inst.cell));
                continue;
            }
            let mut open: Vec<String> = vec![];
            let mut conn = |term: &str| -> String {
                inst.conns.iter().find(|(t, _)| t == term).map(|(_, n)| net_name(n)).filter(|n| !n.is_empty()).unwrap_or_else(
                    || {
                        open.push(term.to_string());
                        format!("nc_{}_{term}", net_name(&inst.name))
                    },
                )
            };
            let param = |k: &str| inst.params.iter().find(|(p, _)| p == k).map(|(_, v)| cdf_value(v)).unwrap_or_default();
            if let Some(sub) = by_cell.get(&inst.cell.to_ascii_lowercase()).filter(|_| symbol_for(map, inst).is_none()) {
                hier = true;
                let mut l = element_name('X', &inst.name);
                for (t, _) in &sub.terms {
                    l += &format!(" {}", conn(t));
                }
                l += &format!(" / {}", sub.cell);
                for (k, v) in &inst.params {
                    let v = cdf_value(v);
                    let declared = sub.cdf.iter().any(|(c, d)| c == k && parse_spice(&cdf_value(d)).is_some());
                    if declared && !v.is_empty() {
                        l += &format!(" {}={v}", k.to_ascii_lowercase());
                    }
                }
                for t in open {
                    warnings.push(format!("{}: terminal {t} is not connected", at(inst)));
                }
                elems.insert(element_name('X', &inst.name).to_ascii_lowercase(), inst.clone());
                text += &l;
                text += "\n";
                continue;
            }
            let Some(sym) = symbol_for(map, inst) else {
                warnings.push(format!(
                    "{}: no IC Studio device for {}/{} (add it to the symbol map); skipped",
                    at(inst),
                    inst.lib,
                    inst.cell
                ));
                continue;
            };
            let kind = kind_of(&sym.kind).expect("filtered");
            let theirs = |ours: &str| sym.params.get(ours).map(|t| param(t)).unwrap_or_default();
            let letter = match kind {
                DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => 'M',
                DeviceKind::Resistor => 'R',
                DeviceKind::Capacitor => 'C',
                DeviceKind::Inductor => 'L',
                DeviceKind::VSource => 'V',
                DeviceKind::ISource => 'I',
                _ => 'D',
            };
            let name = element_name(letter, &inst.name);
            let mut l = name.clone();
            for pin in pin_order(kind) {
                let term = sym.terms.get(*pin).cloned().unwrap_or_else(|| pin.to_ascii_uppercase());
                l += &format!(" {}", conn(&term));
            }
            match kind {
                DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => {
                    let model = Some(theirs("model"))
                        .filter(|m| !m.is_empty())
                        .or_else(|| sym.model.clone())
                        .unwrap_or_else(|| sym.cell.clone());
                    l += &format!(" {model}");
                    for k in ["w", "l", "m"] {
                        let v = theirs(k);
                        if !v.is_empty() {
                            l += &format!(" {k}={v}");
                        }
                    }
                }
                DeviceKind::Resistor | DeviceKind::Capacitor | DeviceKind::Inductor => {
                    let key = match kind {
                        DeviceKind::Resistor => "r",
                        DeviceKind::Capacitor => "c",
                        _ => "l",
                    };
                    let v = theirs(key);
                    if !v.is_empty() {
                        l += &format!(" {v}");
                    }
                }
                DeviceKind::VSource | DeviceKind::ISource => {
                    for (k, kw) in [("dc", "DC"), ("ac", "AC")] {
                        let v = theirs(k);
                        if !v.is_empty() {
                            l += &format!(" {kw} {v}");
                        }
                    }
                }
                _ => {
                    let model = Some(theirs("model")).filter(|m| !m.is_empty()).unwrap_or_else(|| "dmod".into());
                    l += &format!(" {model}");
                }
            }
            for t in open {
                warnings.push(format!("{}: terminal {t} is not connected", at(inst)));
            }
            elems.insert(name.to_ascii_lowercase(), inst.clone());
            text += &l;
            text += "\n";
        }
        text += ".ENDS\n";
        info.insert(s.cell.to_ascii_lowercase(), (elems, hier));
    }
    (text, info)
}

fn norm_net(n: &str) -> String {
    let l = n.to_ascii_lowercase();
    if l == "0" || l == "gnd" || l == "gnd!" { "0".into() } else { l.trim_end_matches('!').to_string() }
}

/// Place the devices of `auto` where Virtuoso has them; `None` when that changes the
/// connectivity (pins of different nets meeting).
fn virtuoso_placed(auto: &Schematic, elems: &HashMap<String, &SchInst>, scale: f64) -> Option<Schematic> {
    let conn = connectivity::extract(auto);
    let mut s = Schematic::default();
    let mut want: Vec<(u64, &'static str, String)> = vec![];
    for i in auto.instances.iter().filter(|i| !matches!(i.kind, DeviceKind::Ground | DeviceKind::Supply)) {
        let v = elems.get(&i.name.to_ascii_lowercase())?;
        let pos = Pt::new((v.xy.0 / scale).round() as i32, (-v.xy.1 / scale).round() as i32);
        let id = s.add_instance(i.kind, pos, sch_orient(&v.orient));
        let inst = s.instance_mut(id).expect("just added");
        inst.name = i.name.clone();
        inst.params = i.params.clone();
        for (pin, at) in inst.pin_positions() {
            let net = conn.net_of(i.id, pin)?.to_string();
            want.push((id, pin, norm_net(&net)));
            if norm_net(&net) == "0" {
                let g = s.add_instance(DeviceKind::Ground, at, Orient::default());
                let name = s.next_name(DeviceKind::Ground);
                s.instance_mut(g).expect("just added").name = name;
            } else {
                s.add_label(net, at);
            }
        }
    }
    let got = connectivity::extract(&s);
    let same = want.iter().all(|(id, pin, net)| got.net_of(*id, pin).is_some_and(|n| norm_net(n) == *net));
    // Distinct nets must stay distinct: compare the partition, not only the names.
    let mut by_net: HashMap<String, HashSet<String>> = HashMap::new();
    for (id, pin, net) in &want {
        by_net.entry(norm_net(got.net_of(*id, pin).unwrap_or(""))).or_default().insert(net.clone());
    }
    (same && by_net.values().all(|v| v.len() == 1)).then_some(s)
}

/// The technology layer of a Virtuoso layer-purpose pair.
fn layer_index(tech: &Tech, layer: &str, purpose: &str) -> Option<usize> {
    if !purpose.is_empty() && purpose != "drawing" {
        for t in [format!("{layer}.{purpose}"), format!("{layer}_{purpose}")] {
            if let Some(i) = tech.layer_index(&t) {
                return Some(i);
            }
        }
        if !["pin", "net", "label", "boundary"].contains(&purpose) {
            return None;
        }
    }
    tech.layer_index(layer)
}

fn import_layout(l: &LayView, tech: &Tech, warnings: &mut Vec<String>) -> Layout {
    let k = tech.dbu_per_um as f64;
    let d = |x: f64| (x * k).round() as i64;
    let p = |(x, y): (f64, f64)| (d(x), d(y));
    let mut out = Layout::default();
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    for s in &l.shapes {
        let (layer, purpose) = match s {
            LShape::Rect { layer, purpose, .. }
            | LShape::Poly { layer, purpose, .. }
            | LShape::Path { layer, purpose, .. }
            | LShape::Label { layer, purpose, .. } => (layer, purpose),
        };
        let Some(li) = layer_index(tech, layer, purpose) else {
            *unknown.entry(format!("{layer} {purpose}")).or_default() += 1;
            continue;
        };
        let g = match s {
            LShape::Rect { a, b, .. } => Geometry::Rect(IRect::new(d(a.0), d(a.1), d(b.0), d(b.1))),
            LShape::Poly { pts, .. } => Geometry::Polygon { points: pts.iter().map(|q| p(*q)).collect() },
            LShape::Path { width, pts, .. } => Geometry::Path { points: pts.iter().map(|q| p(*q)).collect(), width: d(*width) },
            LShape::Label { text, at, height, .. } => Geometry::Label { text: text.clone(), at: p(*at), size: d(*height) },
        };
        out.add(li, g);
    }
    for (lp, n) in &unknown {
        warnings.push(format!("{}/{} layout: {n} shape(s) on {lp} skipped (no such layer in {})", l.lib, l.cell, tech.name));
    }
    for (kind, n) in &l.skipped {
        warnings.push(format!("{}/{} layout: {n} {kind} shape(s) skipped", l.lib, l.cell));
    }
    for i in &l.insts {
        let t = lay_trans(&i.orient, d(i.xy.0), d(i.xy.1));
        let array = i.mosaic.map(|(rows, cols, ux, uy)| ArraySpec { cols, rows, col: (d(ux), 0), row: (0, d(uy)) });
        let id = out.add_instance(CellRef::new(&i.lib, &i.cell), t, array);
        if !i.name.is_empty() {
            out.instance_mut(id).expect("just added").name = Some(i.name.clone());
        }
    }
    out
}

/// Import a dump: one [`CellImport`] per dumped cell. `scale` is the Virtuoso distance (user
/// units) of one schematic grid point.
pub fn import_dump(dump: &Dump, tech: &Tech, map: &SymbolMap, scale: f64) -> Result<VirtuosoImport, String> {
    let mut out = VirtuosoImport::default();
    let scale = if scale > 0.0 { scale } else { 0.1875 };
    let (cdl, info) = dump_cdl(dump, map, &mut out.warnings);
    // Library of each dumped schematic, for the cell instances of hierarchical ones.
    let lib_of: HashMap<String, String> = dump.schematics.iter().map(|s| (s.cell.to_ascii_lowercase(), s.lib.clone())).collect();
    for s in &dump.schematics {
        let (elems, hier) = &info[&s.cell.to_ascii_lowercase()];
        if *hier {
            // Instances of other dumped schematics stay instances of their cells.
            match import_cdl_hier(&cdl, tech, Some(&s.cell), &s.lib) {
                Ok(h) => {
                    out.warnings.extend(h.warnings.iter().map(|w| format!("{}/{}: {w}", s.lib, s.cell)));
                    for c in h.cells {
                        let dumped = lib_of.contains_key(&c.name.to_ascii_lowercase());
                        if dumped && !c.name.eq_ignore_ascii_case(&s.cell) {
                            continue; // imported on its own
                        }
                        let mut sch = c.imported.schematic;
                        for i in sch.instances.iter_mut() {
                            if let Some(r) = i.cell.as_mut()
                                && let Some(l) = lib_of.get(&r.cell.to_ascii_lowercase())
                            {
                                r.lib = l.clone();
                            }
                            // The cell's own devices keep their Virtuoso names (`C0`, not `CC0`).
                            if dumped
                                && let Some(v) = elems.get(&i.name.to_ascii_lowercase())
                                && v.name
                                    .chars()
                                    .next()
                                    .zip(i.name.chars().next())
                                    .is_some_and(|(a, b)| a.eq_ignore_ascii_case(&b))
                            {
                                i.name = v.name.clone();
                            }
                        }
                        out.warnings.extend(c.imported.warnings.iter().map(|w| format!("{}/{}: {w}", s.lib, c.name)));
                        let devices = sch.instances.iter().filter(|i| i.kind.spice_prefix().is_some()).count();
                        let cell = if dumped { CellRef::new(&s.lib, &s.cell) } else { CellRef::new(&s.lib, &c.name) };
                        out.cells.push(CellImport { cell, devices, placed: false, schematic: Some(sch), layout: None });
                    }
                    continue;
                }
                Err(e) => out.warnings.push(format!("{}/{}: hierarchy not kept ({e}); flattened", s.lib, s.cell)),
            }
        }
        let imp = match import_cdl(&cdl, tech, Some(&s.cell)) {
            Ok(i) => i,
            Err(e) => {
                out.warnings.push(format!("{}/{} schematic not imported: {e}", s.lib, s.cell));
                continue;
            }
        };
        out.warnings.extend(imp.imported.warnings.iter().map(|w| format!("{}/{}: {w}", s.lib, s.cell)));
        let mut auto = imp.imported.schematic;
        // Top-level devices keep their Virtuoso names (`M0`, not auCdl's `MM0`).
        for i in &mut auto.instances {
            if let Some(v) = elems.get(&i.name.to_ascii_lowercase())
                && v.name.chars().next().zip(i.name.chars().next()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(&b))
            {
                i.name = v.name.clone();
            }
        }
        let elems: HashMap<String, &SchInst> = elems.values().map(|v| (v.name.to_ascii_lowercase(), v)).collect();
        let placed = if *hier { None } else { virtuoso_placed(&auto, &elems, scale) };
        if !*hier && placed.is_none() {
            out.warnings.push(format!(
                "{}/{}: the Virtuoso placement joins pins of different nets at --scale {scale}; devices laid out in rows",
                s.lib, s.cell
            ));
        }
        // The cell's pins, so that schematics placing it connect to them.
        let ports = auto.ports();
        let placed = placed.map(|mut p| {
            if p.mark_ports(&ports) < ports.len() {
                // A pin net the placement does not label: keep the placement but add the label.
                for (n, _) in &ports {
                    if !p.labels.iter().any(|l| l.name.eq_ignore_ascii_case(n)) {
                        p.add_label(n.clone(), Pt::new(-20, 0));
                    }
                }
                p.mark_ports(&ports);
            }
            p
        });
        out.cells.push(CellImport {
            cell: CellRef::new(&s.lib, &s.cell),
            devices: imp.flat.devices,
            placed: placed.is_some(),
            schematic: Some(placed.unwrap_or(auto)),
            layout: None,
        });
    }
    for l in &dump.layouts {
        if (l.dbu_per_uu - tech.dbu_per_um as f64).abs() > 1e-6 && l.dbu_per_uu > 0.0 {
            out.warnings.push(format!(
                "{}/{} layout: Virtuoso has {} dbu per µm, the technology {}; coordinates are kept in µm",
                l.lib, l.cell, l.dbu_per_uu, tech.dbu_per_um
            ));
        }
        let layout = import_layout(l, tech, &mut out.warnings);
        let r = CellRef::new(&l.lib, &l.cell);
        match out.cells.iter_mut().find(|c| c.cell == r) {
            Some(c) => c.layout = Some(layout),
            None => out.cells.push(CellImport { cell: r, layout: Some(layout), schematic: None, devices: 0, placed: false }),
        }
    }
    if out.cells.is_empty() {
        return Err("the dump has no schematic or layout views".into());
    }
    Ok(out)
}

/// What [`apply`] did to one cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub cell: CellRef,
    pub schematic: bool,
    pub layout: bool,
}

/// Write an import into a project: libraries and cells are created on demand; existing views
/// are replaced only with `overwrite` (else kept, with a warning).
pub fn apply(p: &mut Project, imp: &VirtuosoImport, overwrite: bool, warnings: &mut Vec<String>) -> Result<Vec<Applied>, String> {
    let mut done = vec![];
    for c in &imp.cells {
        if !valid_name(&c.cell.lib) || !valid_name(&c.cell.cell) {
            warnings.push(format!("{}: not a valid IC Studio name; skipped", c.cell));
            continue;
        }
        if p.library_mut(&c.cell.lib).is_none() {
            p.add_library(&c.cell.lib)?;
        }
        if p.cell(&c.cell).is_none() {
            p.add_cell(&c.cell.lib, Cell::new(&c.cell.cell))?;
        }
        let cell = p.cell_mut(&c.cell).expect("just ensured");
        let mut a = Applied { cell: c.cell.clone(), schematic: false, layout: false };
        if let Some(s) = &c.schematic {
            if cell.schematic.is_none() || overwrite {
                cell.schematic = Some(s.clone());
                a.schematic = true;
            } else {
                warnings.push(format!("{}: schematic kept (pass --overwrite to replace it)", c.cell));
            }
        }
        if let Some(l) = &c.layout {
            if cell.layout.is_none() || overwrite {
                cell.layout = Some(l.clone());
                a.layout = true;
            } else {
                warnings.push(format!("{}: layout kept (pass --overwrite to replace it)", c.cell));
            }
        }
        done.push(a);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dump as icsDumpLib writes it: an inverter (flat), a buffer of two inverters
    /// (hierarchical, with a pPar parameter) and layouts with a PCell variant.
    const DUMP: &str = r#"("icsdump" 1 "demo" )
("sch" "demo" "inv" )
("term" "in" "input" )
("term" "out" "output" )
("cdf" "wp" "2u" )
("cdf" "model" "nch" )
("inst" "M1" "analogLib" "pmos4" "symbol" (0.0 0.75 ) "R0" 1 (("model" "pch" ) ("w" "pPar(\"wp\")" ) ("l" "180n" ) ("m" "1" ) ) (("D" "out" ) ("G" "in" ) ("S" "vdd!" ) ("B" "vdd!" ) ) )
("inst" "M0" "analogLib" "nmos4" "symbol" (0.0 -0.75 ) "MX" 1 (("model" "nch" ) ("w" "1u" ) ("l" "180n" ) ("m" "1" ) ) (("D" "out" ) ("G" "in" ) ("S" "gnd!" ) ("B" "gnd!" ) ) )
("inst" "I2" "basic" "ipin" "symbol" (-1.0 0.0 ) "R0" 1 () (("in" "in" ) ) )
("inst" "I3" "analogLib" "gnd" "symbol" (0.0 -2.0 ) "R0" 1 () (("gnd!" "gnd!" ) ) )
("sch" "demo" "buf" )
("term" "a" "input" )
("term" "y" "output" )
("inst" "I0" "demo" "inv" "symbol" (0.0 0.0 ) "R0" 1 (("wp" "4u" ) ) (("in" "a" ) ("out" "mid" ) ) )
("inst" "I1" "demo" "inv" "symbol" (2.0 0.0 ) "R0" 1 () (("in" "mid" ) ("out" "y" ) ) )
("inst" "C0" "analogLib" "cap" "symbol" (4.0 0.0 ) "R0" 1 (("c" "10f" ) ) (("PLUS" "y" ) ("MINUS" "gnd!" ) ) )
("inst" "I9<0:1>" "demo" "inv" "symbol" (6.0 0.0 ) "R0" 2 () () )
("lay" "demo" "inv" 1000 )
("rect" "metal1" "drawing" ((0.0 0.0 ) (1.0 0.5 ) ) )
("poly" "poly" "drawing" ((0.0 0.0 ) (0.2 0.0 ) (0.2 2.0 ) (0.0 2.0 ) ) )
("path" "metal2" "drawing" 0.2 ((0.0 1.0 ) (3.0 1.0 ) ) )
("label" "metal1" "pin" "out" (0.5 0.25 ) 0.1 )
("rect" "nosuch" "drawing" ((0.0 0.0 ) (1.0 1.0 ) ) )
("skip" "ellipse" "metal1" "drawing" )
("linst" "M0" "gpdk" "nmos1v_pc1" (0.5 0.0 ) "MX" )
("lay" "demo" "buf" 1000 )
("linst" "I0" "demo" "inv" (0.0 0.0 ) "R0" )
("mosaic" "M2" "demo" "inv" (0.0 5.0 ) "R0" 2 3 4.0 2.5 )
("lay" "gpdk" "nmos1v_pc1" 1000 )
("rect" "active" "drawing" ((-0.1 0.0 ) (0.5 0.4 ) ) )
("end" )
"#;

    #[test]
    fn script_is_complete_skill() {
        assert!(crate::skill::is_complete(DUMP_SKILL));
        assert!(DUMP_SKILL.contains("procedure(icsDumpLib("));
        // It loads in the built-in interpreter (the Virtuoso database calls are not run).
        let mut p = Project::new("t");
        let src = format!("{DUMP_SKILL}\nlist(icsStr(\"a\") icsStr(2) icsStr(nil))");
        let r = crate::skill::run(&mut p, &src, None).unwrap_or_else(|e| panic!("{}: line {}", e.message, e.line));
        assert_eq!(r.value, "(\"a\" \"2\" \"\")");
    }

    /// The layout half of the script, run by the built-in SKILL interpreter on a demo layout,
    /// reads back as the same shapes and instances.
    #[test]
    fn script_dumps_a_layout() {
        let mut p = crate::examples::demo_project();
        let (r, lay) = p
            .all_cells()
            .filter_map(|(r, c)| {
                c.layout.as_ref().filter(|l| !l.shapes.is_empty() && !l.instances.is_empty()).map(|l| (r, l.clone()))
            })
            .next()
            .expect("a demo layout with shapes and instances");
        let src = format!(
            "{DUMP_SKILL}\nicsMasters = makeTable(\"m\" nil)\nicsQueue = nil\nicsPcellCount = 0\n\
             fprintf(nil \"(\\\"icsdump\\\" 1 \\\"{lib}\\\")\\n\")\n\
             icsDumpLay(nil dbOpenCellViewByType(\"{lib}\" \"{cell}\" \"layout\" nil \"r\") list(\"{lib}\" \"{cell}\"))\n",
            lib = r.lib,
            cell = r.cell
        );
        let out = crate::skill::run(&mut p, &src, None)
            .unwrap_or_else(|e| panic!("{}: line {}\n{}", e.message, e.line, e.output))
            .output;
        let d = Dump::parse(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
        let imp = import_dump(&d, &p.tech, &SymbolMap::analog_lib(), 0.1875).unwrap();
        assert!(imp.warnings.is_empty(), "{:?}\n{out}", imp.warnings);
        let back = imp.cells[0].layout.as_ref().unwrap();
        assert_eq!(back.shapes.len(), lay.shapes.len(), "{out}");
        for s in &lay.shapes {
            assert!(back.shapes.iter().any(|b| b.layer == s.layer && b.geom == s.geom), "{s:?} missing\n{out}");
        }
        assert_eq!(back.instances.len(), lay.instances.len());
        for i in &lay.instances {
            assert!(back.instances.iter().any(|b| b.cell == i.cell && b.trans == i.trans), "{i:?} missing\n{out}");
        }
    }

    #[test]
    fn parses_the_dump() {
        let d = Dump::parse(DUMP).unwrap();
        assert_eq!(d.lib, "demo");
        assert_eq!(d.schematics.len(), 2);
        assert_eq!(d.layouts.len(), 3);
        let inv = &d.schematics[0];
        assert_eq!(inv.terms, [("in".to_string(), "input".to_string()), ("out".to_string(), "output".to_string())]);
        assert_eq!(inv.insts[0].params[1].1, "pPar(\"wp\")");
        assert_eq!(inv.insts[1].orient, "MX");
        let l = &d.layouts[0];
        assert_eq!(l.shapes.len(), 5);
        assert_eq!(l.skipped["ellipse"], 1);
        assert_eq!(d.layouts[1].insts[1].mosaic, Some((2, 3, 4.0, 2.5)));
        assert!(Dump::parse("(\"sch\" \"a\" \"b\")").is_err(), "no header");
        assert!(Dump::parse("(\"icsdump\" 1 \"a\"").is_err(), "unbalanced");
    }

    #[test]
    fn cdf_values() {
        assert_eq!(cdf_value("pPar(\"wp\")"), "wp");
        assert_eq!(cdf_value("pPar(\"wp\") * 2"), "wp*2");
        assert_eq!(cdf_value(" 1u "), "1u");
        assert_eq!(element_name('M', "M0"), "MM0");
        assert_eq!(element_name('M', "N3"), "MN3");
        assert_eq!(element_name('X', "I0"), "XI0");
    }

    #[test]
    fn imports_schematics_and_layouts() {
        let mut tech = Tech::generic();
        let d = Dump::parse(DUMP).unwrap();
        let imp = import_dump(&d, &tech, &SymbolMap::analog_lib(), 0.1875).unwrap();
        let w = imp.warnings.join("\n");
        assert!(w.contains("iterated instance skipped"), "{w}");
        assert!(w.contains("nosuch drawing skipped"), "{w}");
        assert!(w.contains("ellipse"), "{w}");

        // The flat inverter keeps the Virtuoso placement.
        let inv = imp.cells.iter().find(|c| c.cell.cell == "inv").unwrap();
        assert!(inv.placed, "{w}");
        let sch = inv.schematic.as_ref().unwrap();
        let m1 = sch.instance_by_name("M1").unwrap();
        assert_eq!(m1.kind, DeviceKind::Pmos);
        assert_eq!(m1.pos, Pt::new(0, -4));
        assert_eq!(m1.param("w"), "2e-6", "pPar(wp) from the CDF default");
        let m0 = sch.instance_by_name("M0").unwrap();
        assert_eq!((m0.pos, m0.orient), (Pt::new(0, 4), Orient { rot: 2, mirror: true }));
        let conn = connectivity::extract(sch);
        assert_eq!(conn.net_of(m0.id, "d"), conn.net_of(m1.id, "d"));
        assert_eq!(conn.net_of(m0.id, "g"), Some("in"));
        assert_eq!(norm_net(conn.net_of(m0.id, "s").unwrap()), "0");
        assert_eq!(conn.net_of(m1.id, "s"), Some("vdd"));

        // The buffer keeps its hierarchy: two inv instances and a capacitor; XI0 passes
        // wp=4u, so it places a variant cell of inv.
        let buf = imp.cells.iter().find(|c| c.cell.cell == "buf").unwrap();
        assert_eq!(buf.devices, 3);
        assert!(!buf.placed);
        let sch = buf.schematic.as_ref().unwrap();
        assert!(sch.instance_by_name("C0").is_some());
        let xs: Vec<_> = sch.instances.iter().filter(|i| i.kind == DeviceKind::Cell).collect();
        assert_eq!(xs.len(), 2);
        assert!(xs.iter().any(|x| x.cell.as_ref().is_some_and(|c| c.cell == "inv")));
        let variant = xs.iter().find_map(|x| x.cell.as_ref().filter(|c| c.cell != "inv")).expect("a variant cell").clone();
        assert!(imp.cells.iter().any(|c| c.cell == variant), "the variant cell is imported too");
        // Flattened, it is the old flat circuit.
        let mut p = Project::new("v");
        let mut w = vec![];
        apply(&mut p, &imp, false, &mut w).unwrap();
        let flat = p.circuit(&buf.cell).unwrap();
        let mos: Vec<_> = flat.instances.iter().filter(|i| i.kind.is_mos()).collect();
        assert_eq!(mos.len(), 4);
        let p0 = mos.iter().find(|i| i.name.starts_with("XI0/") && i.kind == DeviceKind::Pmos).unwrap();
        assert_eq!(p0.param("w"), "4e-6");
        let conn = connectivity::extract(&flat);
        let p1 = mos.iter().find(|i| i.name.starts_with("XI1/") && i.kind == DeviceKind::Pmos).unwrap();
        assert_eq!(conn.net_of(p0.id, "d"), conn.net_of(p1.id, "g"));
        assert_eq!(conn.net_of(p0.id, "g"), Some("a"));

        // Layouts: shapes in dbu, a PCell variant in its own library, a mosaic as an array.
        let lay = inv.layout.as_ref().unwrap();
        assert_eq!(lay.shapes.len(), 4);
        assert!(lay.shapes.iter().any(|s| s.geom == Geometry::Rect(IRect::new(0, 0, 1000, 500))));
        assert!(lay.shapes.iter().any(|s| matches!(&s.geom, Geometry::Path { width: 200, .. })));
        assert_eq!(lay.instances[0].cell, CellRef::new("gpdk", "nmos1v_pc1"));
        assert_eq!(lay.instances[0].trans, Trans::with(500, 0, 0, true));
        assert_eq!(lay.instances[0].name.as_deref(), Some("M0"));
        let blay = buf.layout.as_ref().unwrap();
        assert_eq!(blay.instances[1].array, Some(ArraySpec { cols: 3, rows: 2, col: (4000, 0), row: (0, 2500) }));

        let mut p = Project::new("t");
        let mut warns = vec![];
        let done = apply(&mut p, &imp, false, &mut warns).unwrap();
        assert_eq!(done.len(), 4, "inv, its wp=4u variant, buf and the PCell variant");
        assert!(p.cell(&CellRef::new("gpdk", "nmos1v_pc1")).unwrap().layout.is_some());
        let again = apply(&mut p, &imp, false, &mut warns).unwrap();
        assert!(again.iter().all(|a| !a.schematic && !a.layout));
        assert!(warns.iter().any(|w| w.contains("--overwrite")));

        // A coarse scale piles the devices up: rows instead.
        let imp = import_dump(&d, &tech, &SymbolMap::analog_lib(), 10.0).unwrap();
        assert!(!imp.cells.iter().find(|c| c.cell.cell == "inv").unwrap().placed);
        tech.dbu_per_um = 2000;
        let imp = import_dump(&d, &tech, &SymbolMap::analog_lib(), 0.1875).unwrap();
        assert!(imp.warnings.iter().any(|w| w.contains("dbu per µm")));
    }
}
