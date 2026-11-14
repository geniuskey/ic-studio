//! SPICE deck parser (subset) producing a flat circuit for the built-in engine.

use std::collections::HashMap;

use ics_core::Analysis;
use ics_core::units::parse_spice;

use crate::SimError;

#[derive(Debug, Clone, PartialEq)]
pub enum Wave {
    Pulse { v1: f64, v2: f64, td: f64, tr: f64, tf: f64, pw: f64, per: f64 },
    Sin { vo: f64, va: f64, freq: f64, td: f64, theta: f64 },
    Pwl(Vec<(f64, f64)>),
}

impl Wave {
    pub fn value(&self, t: f64) -> f64 {
        match self {
            Wave::Pulse { v1, v2, td, tr, tf, pw, per } => {
                if t < *td {
                    return *v1;
                }
                let mut tt = t - td;
                if *per > 0.0 {
                    tt %= per;
                }
                let tr = tr.max(1e-15);
                let tf = tf.max(1e-15);
                if tt < tr {
                    v1 + (v2 - v1) * tt / tr
                } else if tt < tr + pw {
                    *v2
                } else if tt < tr + pw + tf {
                    v2 + (v1 - v2) * (tt - tr - pw) / tf
                } else {
                    *v1
                }
            }
            Wave::Sin { vo, va, freq, td, theta } => {
                if t < *td {
                    *vo
                } else {
                    let tt = t - td;
                    vo + va * (-tt * theta).exp() * (2.0 * std::f64::consts::PI * freq * tt).sin()
                }
            }
            Wave::Pwl(pts) => {
                if pts.is_empty() {
                    return 0.0;
                }
                if t <= pts[0].0 {
                    return pts[0].1;
                }
                for w in pts.windows(2) {
                    if t <= w[1].0 {
                        let f = if w[1].0 > w[0].0 { (t - w[0].0) / (w[1].0 - w[0].0) } else { 1.0 };
                        return w[0].1 + f * (w[1].1 - w[0].1);
                    }
                }
                pts.last().unwrap().1
            }
        }
    }

    /// Times where the waveform has slope discontinuities (for time-step control).
    pub fn breakpoints(&self, tstop: f64) -> Vec<f64> {
        match self {
            Wave::Pulse { td, tr, tf, pw, per, .. } => {
                let mut v = vec![];
                let mut base = *td;
                loop {
                    for t in [base, base + tr, base + tr + pw, base + tr + pw + tf] {
                        if t <= tstop {
                            v.push(t);
                        }
                    }
                    if *per <= 0.0 || base > tstop || v.len() > 10_000 {
                        break;
                    }
                    base += per;
                }
                v
            }
            Wave::Pwl(pts) => pts.iter().map(|p| p.0).filter(|t| *t <= tstop).collect(),
            Wave::Sin { .. } => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    pub dc: f64,
    pub ac_mag: f64,
    pub ac_phase: f64,
    pub tran: Option<Wave>,
}

impl Source {
    pub fn at(&self, t: Option<f64>) -> f64 {
        match (t, &self.tran) {
            (Some(t), Some(w)) => w.value(t),
            _ => self.dc,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiodeModel {
    pub is: f64,
    pub n: f64,
    /// Zero-bias depletion capacitance, its built-in potential, grading and the forward-bias
    /// linearisation point (fraction of `vj`).
    pub cjo: f64,
    pub vj: f64,
    pub m: f64,
    pub fc: f64,
    /// Transit time: diffusion charge `tt · id`.
    pub tt: f64,
    /// Series resistance (an internal node in the circuit).
    pub rs: f64,
    /// Reverse breakdown voltage (infinite: none) and the current at it.
    pub bv: f64,
    pub ibv: f64,
    /// Flicker noise coefficient and exponent.
    pub kf: f64,
    pub af: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MosModel {
    pub pmos: bool,
    pub vto: f64,
    pub kp: f64,
    pub lambda: f64,
    pub gamma: f64,
    pub phi: f64,
    /// Oxide thickness; 0 when not given (no intrinsic gate capacitance, as in SPICE).
    pub tox: f64,
    /// Lateral diffusion: the effective channel length is `l - 2 ld`.
    pub ld: f64,
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,
    /// Zero-bias junction capacitance per area (F/m²) and per perimeter (F/m), their grading
    /// coefficients, the junction potential and the forward-bias linearisation point.
    pub cj: f64,
    pub cjsw: f64,
    pub mj: f64,
    pub mjsw: f64,
    pub pb: f64,
    pub fc: f64,
    /// Bulk junction saturation current, absolute (A) or per junction area (A/m²) when `js > 0`.
    pub is: f64,
    pub js: f64,
    /// Drain / source series resistance, absolute (Ω) or `rsh` per square times `nrd` / `nrs`.
    pub rd: f64,
    pub rs: f64,
    pub rsh: f64,
    /// Flicker noise coefficient and exponent.
    pub kf: f64,
    pub af: f64,
}

/// Drain / source junction geometry of a MOS instance (m², m), per device of `m`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Junctions {
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
}

const EPS_OX: f64 = 3.453e-11;

impl MosModel {
    /// Gate oxide capacitance per area (F/m²), 0 without `tox`.
    pub fn cox(&self) -> f64 {
        if self.tox > 0.0 { EPS_OX / self.tox } else { 0.0 }
    }

    pub fn leff(&self, l: f64) -> f64 {
        (l - 2.0 * self.ld).max(1e-9)
    }
}

/// Node index: 0 is ground.
pub type Node = usize;

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    R { name: String, a: Node, b: Node, r: f64 },
    C { name: String, a: Node, b: Node, c: f64 },
    L { name: String, a: Node, b: Node, l: f64 },
    V { name: String, a: Node, b: Node, src: Source },
    I { name: String, a: Node, b: Node, src: Source },
    D { name: String, a: Node, k: Node, model: DiodeModel },
    M { name: String, d: Node, g: Node, s: Node, b: Node, model: MosModel, w: f64, l: f64, m: f64, junctions: Junctions },
}

impl Element {
    pub fn name(&self) -> &str {
        match self {
            Element::R { name, .. }
            | Element::C { name, .. }
            | Element::L { name, .. }
            | Element::V { name, .. }
            | Element::I { name, .. }
            | Element::D { name, .. }
            | Element::M { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Circuit {
    pub title: String,
    /// Node names; index 0 is ground ("0").
    pub nodes: Vec<String>,
    pub elements: Vec<Element>,
    pub analyses: Vec<Analysis>,
    pub temp: f64,
    /// Deck content the built-in engine ignores or approximates (unsupported model levels and
    /// parameters, dot-commands); empty when it evaluates the deck as written.
    pub warnings: Vec<String>,
}

impl Circuit {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// MOS model parameters the built-in engine evaluates.
const MOS_PARAMS: &[&str] = &[
    "level", "vto", "kp", "u0", "lambda", "gamma", "phi", "tox", "ld", "cgso", "cgdo", "cgbo", "cj", "cjsw", "mj", "mjsw", "pb",
    "fc", "is", "js", "rd", "rs", "rsh", "kf", "af",
];
/// Diode model parameters the built-in engine evaluates.
const DIODE_PARAMS: &[&str] = &["is", "n", "cjo", "cj0", "vj", "m", "fc", "tt", "rs", "bv", "ibv", "kf", "af"];
/// MOS instance parameters the built-in engine evaluates.
const MOS_INSTANCE_PARAMS: &[&str] = &["w", "l", "m", "ad", "as", "pd", "ps", "nrd", "nrs"];
/// Dot-commands the built-in engine reads.
const DOT_COMMANDS: &[&str] = &[".model", ".temp", ".end", ".title", ".op", ".dc", ".ac", ".tran", ".noise"];

/// The device-side node of terminal `ext` behind a series resistance `r` (a new internal node
/// `dev#tag` and a resistor), or `ext` itself without one.
fn series(c: &mut Circuit, node: &mut impl FnMut(&str, &mut Circuit) -> Node, dev: &str, tag: &str, ext: Node, r: f64) -> Node {
    if !(r > 0.0 && r.is_finite()) {
        return ext;
    }
    let int = node(&format!("{dev}#{tag}"), c);
    c.elements.push(Element::R { name: format!("{dev}#r{tag}"), a: ext, b: int, r });
    int
}

fn ignored_params(p: &HashMap<String, f64>, known: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = p.keys().filter(|k| !known.contains(&k.as_str())).cloned().collect();
    v.sort();
    v
}

fn tokens(s: &str) -> Vec<String> {
    s.replace(['(', ')', ',', '='], " ").split_whitespace().map(|t| t.to_string()).collect()
}

fn kv_params(toks: &[String]) -> HashMap<String, f64> {
    // tokens are already split on '=' so pairs alternate key, value
    let mut m = HashMap::new();
    let mut i = 0;
    while i + 1 < toks.len() {
        if let Some(v) = parse_spice(&toks[i + 1]) {
            m.insert(toks[i].to_ascii_lowercase(), v);
            i += 2;
        } else {
            i += 1;
        }
    }
    m
}

fn parse_source(rest: &str) -> Result<Source, String> {
    let toks = tokens(rest);
    let mut src = Source::default();
    let mut i = 0;
    let is_kw = |t: &str| matches!(t.to_ascii_lowercase().as_str(), "dc" | "ac" | "pulse" | "sin" | "pwl" | "exp");
    let nums_from = |i: &mut usize| -> Vec<f64> {
        let mut v = vec![];
        while *i < toks.len() && !is_kw(&toks[*i]) {
            if let Some(x) = parse_spice(&toks[*i]) {
                v.push(x);
            }
            *i += 1;
        }
        v
    };
    while i < toks.len() {
        let t = toks[i].to_ascii_lowercase();
        i += 1;
        match t.as_str() {
            "dc" => {
                let v = nums_from(&mut i);
                src.dc = *v.first().ok_or("DC without value")?;
            }
            "ac" => {
                let v = nums_from(&mut i);
                src.ac_mag = v.first().copied().unwrap_or(1.0);
                src.ac_phase = v.get(1).copied().unwrap_or(0.0);
            }
            "pulse" => {
                let v = nums_from(&mut i);
                let g = |k: usize, d: f64| v.get(k).copied().unwrap_or(d);
                src.tran = Some(Wave::Pulse {
                    v1: g(0, 0.0),
                    v2: g(1, 0.0),
                    td: g(2, 0.0),
                    tr: g(3, 0.0),
                    tf: g(4, 0.0),
                    pw: g(5, f64::INFINITY),
                    per: g(6, 0.0),
                });
            }
            "sin" => {
                let v = nums_from(&mut i);
                let g = |k: usize, d: f64| v.get(k).copied().unwrap_or(d);
                src.tran = Some(Wave::Sin { vo: g(0, 0.0), va: g(1, 0.0), freq: g(2, 0.0), td: g(3, 0.0), theta: g(4, 0.0) });
            }
            "pwl" => {
                let v = nums_from(&mut i);
                src.tran = Some(Wave::Pwl(v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect()));
            }
            other => {
                if let Some(x) = parse_spice(other) {
                    src.dc = x;
                } else {
                    return Err(format!("unsupported source keyword '{other}'"));
                }
            }
        }
    }
    Ok(src)
}

/// Join continuation lines and drop comments. Returns (line number, text).
fn logical_lines(deck: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = vec![];
    for (n, raw) in deck.lines().enumerate() {
        let line = raw.split(';').next().unwrap_or("").trim_end();
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('*') {
            continue;
        }
        if let Some(cont) = t.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(cont);
            }
            continue;
        }
        out.push((n + 1, t.to_string()));
    }
    out
}

/// The analysis of a `.op` / `.dc` / `.ac` / `.tran` line.
fn dot_analysis(l: &str) -> Option<Analysis> {
    let t: Vec<&str> = l.split_whitespace().collect();
    let num = |k: usize| t.get(k).map(|s| s.to_string()).unwrap_or_default();
    Some(match t.first()?.to_ascii_lowercase().as_str() {
        ".op" => Analysis::Op,
        ".dc" => Analysis::Dc { source: num(1), start: num(2), stop: num(3), step: num(4) },
        ".ac" => {
            Analysis::Ac { points_per_decade: t.get(2).and_then(|s| s.parse().ok()).unwrap_or(10), fstart: num(3), fstop: num(4) }
        }
        ".tran" => Analysis::Tran { tstep: num(1), tstop: num(2) },
        ".noise" => Analysis::Noise {
            output: num(1).trim_start_matches(['v', 'V']).trim_matches(['(', ')']).to_string(),
            source: num(2),
            points_per_decade: t.get(4).and_then(|s| s.parse().ok()).unwrap_or(10),
            fstart: num(5),
            fstop: num(6),
        },
        _ => return None,
    })
}

/// The deck's own analyses, without parsing its elements (for backends that read the deck
/// themselves, such as ngspice with PDK subcircuits and model libraries).
pub fn deck_analyses(deck: &str) -> Vec<Analysis> {
    let mut out = vec![];
    let mut in_control = false;
    for (_, l) in logical_lines(deck) {
        let lower = l.to_ascii_lowercase();
        if lower.starts_with(".control") {
            in_control = true;
        } else if lower.starts_with(".endc") {
            in_control = false;
        } else if lower.split_whitespace().next() == Some(".end") {
            break;
        } else if !in_control && let Some(a) = dot_analysis(&l) {
            out.push(a);
        }
    }
    out
}

pub fn parse_deck(deck: &str) -> Result<Circuit, SimError> {
    let mut c = Circuit { nodes: vec!["0".into()], temp: 27.0, ..Default::default() };
    c.title = deck.lines().next().unwrap_or("").trim_start_matches('*').trim().to_string();
    let mut node_map: HashMap<String, usize> = HashMap::new();
    node_map.insert("0".into(), 0);
    node_map.insert("gnd".into(), 0);
    let mut node = |name: &str, c: &mut Circuit| -> usize {
        let key = name.to_ascii_lowercase();
        if let Some(&i) = node_map.get(&key) {
            return i;
        }
        c.nodes.push(key.clone());
        node_map.insert(key, c.nodes.len() - 1);
        c.nodes.len() - 1
    };

    let lines = logical_lines(deck);
    // First pass: models.
    let mut diode_models: HashMap<String, DiodeModel> = HashMap::new();
    let mut mos_models: HashMap<String, MosModel> = HashMap::new();
    let mut in_control = false;
    for (_, l) in &lines {
        let lower = l.to_ascii_lowercase();
        if lower.starts_with(".control") {
            in_control = true;
        }
        if lower.starts_with(".endc") {
            in_control = false;
            continue;
        }
        if in_control || !lower.starts_with(".model") {
            continue;
        }
        let toks = tokens(l);
        if toks.len() < 3 {
            continue;
        }
        let name = toks[1].to_ascii_lowercase();
        let ty = toks[2].to_ascii_lowercase();
        let p = kv_params(&toks[3..]);
        let g = |k: &str, d: f64| p.get(k).copied().unwrap_or(d);
        let known = match ty.as_str() {
            "d" => Some(DIODE_PARAMS),
            "nmos" | "pmos" => Some(MOS_PARAMS),
            _ => None,
        };
        if let Some(known) = known {
            let ignored = ignored_params(&p, known);
            if !ignored.is_empty() {
                c.warnings.push(format!("model {name}: parameters not evaluated by the built-in engine: {}", ignored.join(" ")));
            }
        }
        if known == Some(MOS_PARAMS) && g("level", 1.0) != 1.0 {
            c.warnings.push(format!("model {name}: level={} is evaluated as level 1", g("level", 1.0)));
        }
        match ty.as_str() {
            "d" => {
                let d = DiodeModel {
                    is: g("is", 1e-14),
                    n: g("n", 1.0),
                    cjo: g("cjo", g("cj0", 0.0)),
                    vj: g("vj", 1.0),
                    m: g("m", 0.5),
                    fc: g("fc", 0.5),
                    tt: g("tt", 0.0),
                    rs: g("rs", 0.0),
                    bv: g("bv", f64::INFINITY),
                    ibv: g("ibv", 1e-3),
                    kf: g("kf", 0.0),
                    af: g("af", 1.0),
                };
                diode_models.insert(name, d);
            }
            "nmos" | "pmos" => {
                // SPICE level 1 defaults: without `tox` there is no intrinsic gate capacitance
                // and `kp` defaults to 2e-5; with it, `kp = u0 · cox`.
                let tox = g("tox", 0.0);
                let kp = match p.get("kp") {
                    Some(kp) => *kp,
                    None if tox > 0.0 => g("u0", 600.0) * 1e-4 * EPS_OX / tox,
                    None => 2e-5,
                };
                let m = MosModel {
                    pmos: ty == "pmos",
                    vto: g("vto", 0.0),
                    kp,
                    lambda: g("lambda", 0.0),
                    gamma: g("gamma", 0.0),
                    phi: g("phi", 0.6),
                    tox,
                    ld: g("ld", 0.0),
                    cgso: g("cgso", 0.0),
                    cgdo: g("cgdo", 0.0),
                    cgbo: g("cgbo", 0.0),
                    cj: g("cj", 0.0),
                    cjsw: g("cjsw", 0.0),
                    mj: g("mj", 0.5),
                    mjsw: g("mjsw", 0.5),
                    pb: g("pb", 0.8),
                    fc: g("fc", 0.5),
                    is: g("is", 1e-14),
                    js: g("js", 0.0),
                    rd: g("rd", 0.0),
                    rs: g("rs", 0.0),
                    rsh: g("rsh", 0.0),
                    kf: g("kf", 0.0),
                    af: g("af", 1.0),
                };
                mos_models.insert(name, m);
            }
            _ => {}
        }
    }

    let err = |line: usize, msg: String| SimError::Parse { line, msg };
    in_control = false;
    for (ln, l) in &lines {
        let ln = *ln;
        let lower = l.to_ascii_lowercase();
        if lower.starts_with(".control") {
            in_control = true;
            continue;
        }
        if lower.starts_with(".endc") {
            in_control = false;
            continue;
        }
        if in_control {
            continue;
        }
        if lower.starts_with('.') {
            if lower.split_whitespace().next() == Some(".end") {
                break;
            }
            let cmd = lower.split_whitespace().next().unwrap_or("");
            if let Some(a) = dot_analysis(l) {
                c.analyses.push(a);
            } else if cmd == ".temp" {
                c.temp = l.split_whitespace().nth(1).and_then(parse_spice).unwrap_or(27.0);
            } else if !DOT_COMMANDS.contains(&cmd) {
                c.warnings.push(format!("line {ln}: '{cmd}' is ignored by the built-in engine"));
            }
            continue;
        }
        let t: Vec<&str> = l.split_whitespace().collect();
        let name = t[0].to_ascii_lowercase();
        let need = |n: usize| -> Result<(), SimError> {
            if t.len() < n { Err(err(ln, format!("'{}' needs at least {} fields", t[0], n))) } else { Ok(()) }
        };
        let value = |k: usize| -> Result<f64, SimError> {
            t.get(k).and_then(|s| parse_spice(s)).ok_or_else(|| err(ln, format!("bad value in '{l}'")))
        };
        let first = name.chars().next().unwrap_or(' ');
        let el = match first {
            'r' | 'c' | 'l' => {
                need(4)?;
                let (a, b) = (node(t[1], &mut c), node(t[2], &mut c));
                let v = value(3)?;
                match first {
                    'r' => {
                        if v == 0.0 {
                            return Err(err(ln, format!("{name}: zero resistance")));
                        }
                        Element::R { name, a, b, r: v }
                    }
                    'c' => Element::C { name, a, b, c: v },
                    _ => Element::L { name, a, b, l: v },
                }
            }
            'v' | 'i' => {
                need(3)?;
                let (a, b) = (node(t[1], &mut c), node(t[2], &mut c));
                let rest = t[3..].join(" ");
                let src = parse_source(&rest).map_err(|m| err(ln, m))?;
                if first == 'v' { Element::V { name, a, b, src } } else { Element::I { name, a, b, src } }
            }
            'd' => {
                need(4)?;
                let (a, k) = (node(t[1], &mut c), node(t[2], &mut c));
                let model = diode_models
                    .get(&t[3].to_ascii_lowercase())
                    .cloned()
                    .ok_or_else(|| err(ln, format!("unknown diode model '{}'", t[3])))?;
                let a = series(&mut c, &mut node, &name, "a", a, model.rs);
                Element::D { name, a, k, model }
            }
            'm' => {
                need(6)?;
                let d = node(t[1], &mut c);
                let g = node(t[2], &mut c);
                let s = node(t[3], &mut c);
                let b = node(t[4], &mut c);
                let model = mos_models
                    .get(&t[5].to_ascii_lowercase())
                    .cloned()
                    .ok_or_else(|| err(ln, format!("unknown MOS model '{}'", t[5])))?;
                let p = kv_params(&tokens(&t[6..].join(" ")));
                let ignored = ignored_params(&p, MOS_INSTANCE_PARAMS);
                if !ignored.is_empty() {
                    c.warnings.push(format!("{name}: instance parameters not evaluated: {}", ignored.join(" ")));
                }
                let mult = p.get("m").copied().unwrap_or(1.0).max(1e-12);
                let r = |abs: f64, squares: &str| {
                    if abs > 0.0 { abs } else { model.rsh * p.get(squares).copied().unwrap_or(0.0) }
                };
                let d = series(&mut c, &mut node, &name, "d", d, r(model.rd, "nrd") / mult);
                let s = series(&mut c, &mut node, &name, "s", s, r(model.rs, "nrs") / mult);
                Element::M {
                    name,
                    d,
                    g,
                    s,
                    b,
                    model,
                    w: p.get("w").copied().unwrap_or(1e-6),
                    l: p.get("l").copied().unwrap_or(1e-6),
                    m: p.get("m").copied().unwrap_or(1.0),
                    junctions: Junctions {
                        ad: p.get("ad").copied().unwrap_or(0.0),
                        as_: p.get("as").copied().unwrap_or(0.0),
                        pd: p.get("pd").copied().unwrap_or(0.0),
                        ps: p.get("ps").copied().unwrap_or(0.0),
                    },
                }
            }
            _ => return Err(err(ln, format!("unsupported element '{}' (built-in engine supports R C L V I D M)", t[0]))),
        };
        c.elements.push(el);
    }
    let mut seen = std::collections::HashSet::new();
    c.warnings.retain(|w| seen.insert(w.clone()));
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sources() {
        let s = parse_source("DC 0 AC 1 PULSE(0 1.8 1n 100p 100p 4n 10n)").unwrap();
        assert_eq!(s.dc, 0.0);
        assert_eq!(s.ac_mag, 1.0);
        let w = s.tran.unwrap();
        assert!((w.value(3e-9) - 1.8).abs() < 1e-12);
        assert_eq!(w.value(0.0), 0.0);
        let s = parse_source("5").unwrap();
        assert_eq!(s.dc, 5.0);
    }

    #[test]
    fn warns_about_what_it_does_not_evaluate() {
        let deck = "* t\n.param wn=1u\n.option reltol=1e-4\n.model nm nmos (level=54 vto=0.4 u0=0.03)\n\
                    .model dm d (is=1e-14 rs=10 xti=3)\nM1 d g 0 0 nm w=1u l=1u nf=2 sa=0.2u\nV1 d 0 1\nV2 g 0 1\n.op\n.end\n";
        let c = parse_deck(deck).unwrap();
        let w = c.warnings.join("\n");
        for s in ["'.param'", "'.option'", "level=54", "parameters not evaluated by the built-in engine: xti", "nf sa"] {
            assert!(w.contains(s), "{s} missing from\n{w}");
        }
        let clean = format!(
            "* t\nV1 a 0 1\nR1 a 0 1k\nM1 a a 0 0 nmos w=1u l=1u ad=1p\n{}\n.op\n.end\n",
            ics_core::netlist::DEFAULT_MODELS
        );
        assert!(parse_deck(&clean).unwrap().warnings.is_empty());
    }

    #[test]
    fn series_resistances_get_internal_nodes() {
        let deck = "* t\n.model nm nmos (rsh=10)\n.model dm d (rs=5)\nM1 d g 0 0 nm w=1u l=1u nrd=2 m=2\nD1 a 0 dm\n.end\n";
        let c = parse_deck(deck).unwrap();
        let rs: Vec<(&str, f64)> = c
            .elements
            .iter()
            .filter_map(|e| if let Element::R { name, r, .. } = e { Some((name.as_str(), *r)) } else { None })
            .collect();
        assert_eq!(rs, vec![("m1#rd", 10.0), ("d1#ra", 5.0)]);
        assert!(c.nodes.iter().any(|n| n == "m1#d") && c.nodes.iter().any(|n| n == "d1#a"));
    }

    #[test]
    fn parse_basic_deck() {
        let deck = "* t\nV1 in 0 DC 1\nR1 in out 1k\nC1 out 0 1n\n+ \n.model nmos nmos (vto=0.5 kp=100u)\nM1 out in 0 0 nmos w=1u l=1u\n.tran 1n 10n\n.end\n";
        let c = parse_deck(deck).unwrap();
        assert_eq!(c.elements.len(), 4);
        assert_eq!(c.nodes.len(), 3);
        assert_eq!(c.analyses.len(), 1);
    }
}
