//! A subset of SVRF (the Calibre rule file language) translated to the IC Studio DRC deck
//! ([`Deck`]), so foundry-style decks run on the built-in engine.
//!
//! Translated:
//! - Comments `//` and `/* */`; the preprocessor `#DEFINE`, `#UNDEFINE`, `#IFDEF`, `#IFNDEF`
//!   (with an optional value), `#ELSE`, `#ENDIF`; `VARIABLE name number`.
//! - `LAYER MAP g DATATYPE d n` and `LAYER name n ...`: a layer number maps to the
//!   technology layers with that GDS layer / datatype (all datatypes without a `LAYER MAP`).
//!   Technology layer names can be used directly too.
//! - Layer operations: `AND OR NOT XOR` (infix or prefix), `[NOT] INTERACT | INSIDE | OUTSIDE |
//!   ENCLOSE | TOUCH`, `SIZE a BY d [OVERUNDER | UNDEROVER]`, `HOLES`, `EXTENTS`, `COPY`,
//!   `MERGE`, `AREA a constraint`, parentheses, and the checks below used as layers.
//! - Rule checks `NAME { @ comment ... }`: `INT a < d` (width), `EXT a < d` (space),
//!   `EXT a b < d` (separation), `ENC a b < d` (enclosure of `a` by `b`), `AREA a < x`,
//!   `DENSITY a < x | > x [WINDOW w] [STEP s]`; any other layer expression reports all of its
//!   geometry. Layers defined inside a check are local to it.
//!
//! Constraints `<= d` become `< d + 1 dbu`. Check modifiers that only shape the output
//! (`REGION`, `SINGULAR`, `ABUT`, `MEASURE ALL`, ...) are ignored silently; others
//! (`OPPOSITE`, `PROJECTING`, `PARALLEL`, `NOT CONNECTED`, ...) are ignored with a note, as
//! the results may differ. Specification and connectivity statements (`LAYOUT`, `DRC`,
//! `PRECISION`, `CONNECT`, `DEVICE`, `LVS`, ...) are skipped. Everything else (edge
//! operations, `WITH WIDTH`, two-layer `INT`, ...) is reported as a [`Note`] and its rule
//! check is left out.

use std::collections::HashMap;
use std::path::Path;

use crate::drc::{Deck, DerivedLayer, LayerExpr, NamedRule, Rule, Select};
use crate::region::BoolOp;
use crate::tech::Tech;

/// A statement that was not translated (or only approximately), with its line.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Translation {
    pub deck: Deck,
    pub notes: Vec<Note>,
    /// Rule checks in the file and how many were translated.
    pub checks: usize,
    pub translated: usize,
}

const SPEC: &[&str] = &[
    "LAYOUT",
    "DRC",
    "PRECISION",
    "RESOLUTION",
    "TITLE",
    "TEXT",
    "PORT",
    "MASK",
    "PEX",
    "SVRF",
    "UNIT",
    "FLAG",
    "SNAP",
    "GROUP",
    "POLYGON",
    "DFM",
    "OPCBIAS",
    "INCLUDE",
];
const LVS: &[&str] =
    &["CONNECT", "SCONNECT", "DEVICE", "ATTACH", "LABEL", "LVS", "SOURCE", "ERC", "NET", "TRACE", "VIRTUAL", "EXPAND", "PUSH"];
const HARMLESS: &[&str] = &[
    "REGION", "SINGULAR", "ABUT", "MEASURE", "ALL", "EXCLUDE", "FALSE", "TRUE", "ALSO", "INSIDE", "OUTSIDE", "POLYGON", "EDGE",
    "BACKUP", "PRINT", "RDB", "ONLY",
];

/// Whether `text` looks like SVRF rather than an IC Studio deck.
pub fn looks_like_svrf(text: &str) -> bool {
    text.lines().map(str::trim).any(|l| {
        let up = l.to_ascii_uppercase();
        let mut t = up.split_whitespace();
        let (first, second) = (t.next().unwrap_or_default(), t.next().unwrap_or_default());
        l.contains('{')
            || l.starts_with("//")
            || ["#DEFINE", "#IFDEF", "#IFNDEF", "#INCLUDE"].contains(&first)
            || (["LAYER", "VARIABLE", "INCLUDE", "LAYOUT", "PRECISION", "DRC"].contains(&first) && second != "=")
    })
}

/// Replace `INCLUDE "file"` statements by the file's text (relative to `base`, nested up to 8
/// levels). Returns the text and the includes that could not be read.
pub fn inline_includes(text: &str, base: &Path) -> (String, Vec<String>) {
    fn go(text: &str, base: &Path, depth: usize, out: &mut String, errs: &mut Vec<String>) {
        for line in text.lines() {
            let t = line.trim();
            let file = t
                .strip_prefix("INCLUDE")
                .or_else(|| t.strip_prefix("include"))
                .filter(|r| r.starts_with(char::is_whitespace))
                .map(|r| r.trim().trim_matches('"').to_string());
            match file {
                Some(f) if depth < 8 => {
                    let path = base.join(&f);
                    match std::fs::read_to_string(&path) {
                        Ok(inner) => {
                            go(&inner, path.parent().unwrap_or(base), depth + 1, out, errs);
                            continue;
                        }
                        Err(e) => errs.push(format!("INCLUDE {}: {e}", path.display())),
                    }
                }
                Some(_) => errs.push(format!("INCLUDE {f:?} nested too deeply", f = t)),
                None => {}
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    let (mut out, mut errs) = (String::new(), Vec::new());
    go(text, base, 0, &mut out, &mut errs);
    (out, errs)
}

/// `text` without comments; line breaks are kept so line numbers stay valid.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut block, mut quoted) = (false, false);
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if block {
            if c == '*' && b.get(i + 1) == Some(&b'/') {
                block = false;
                i += 2;
                continue;
            }
            if c == '\n' {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        } else if c == '\n' {
            quoted = false;
        } else if !quoted && c == '/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if !quoted && c == '/' && b.get(i + 1) == Some(&b'*') {
            block = true;
            i += 2;
            continue;
        }
        let ch = text[i..].chars().next().expect("in bounds");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn tokenize(s: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let flush = |cur: &mut String, toks: &mut Vec<String>| {
        if !cur.is_empty() {
            toks.push(std::mem::take(cur));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                flush(&mut cur, &mut toks);
                let mut q = String::from('"');
                for d in chars.by_ref() {
                    q.push(d);
                    if d == '"' {
                        break;
                    }
                }
                toks.push(q);
            }
            '(' | ')' | '[' | ']' => {
                flush(&mut cur, &mut toks);
                toks.push(c.to_string());
            }
            '<' | '>' | '=' | '!' => {
                flush(&mut cur, &mut toks);
                let mut op = c.to_string();
                while let Some(&d) = chars.peek().filter(|d| matches!(d, '<' | '>' | '=' | '!')) {
                    op.push(d);
                    chars.next();
                }
                toks.push(op);
            }
            c if c.is_whitespace() => flush(&mut cur, &mut toks),
            c => cur.push(c),
        }
    }
    flush(&mut cur, &mut toks);
    toks
}

struct Block {
    name: String,
    line: usize,
    comment: Vec<String>,
    stmts: Vec<(usize, String)>,
}

enum Item {
    Stmt(usize, String),
    Block(Block),
}

/// Statements and rule check blocks of the preprocessed text.
fn structure(lines: &[(usize, String)], notes: &mut Vec<Note>) -> Vec<Item> {
    let mut items = Vec::new();
    let mut block: Option<Block> = None;
    for (ln, line) in lines {
        let mut rest = line.as_str();
        loop {
            let cut = rest.find(['{', '}']);
            let (piece, brace) = match cut {
                Some(i) => (&rest[..i], Some(rest.as_bytes()[i] as char)),
                None => (rest, None),
            };
            let piece = piece.trim();
            match block.as_mut() {
                Some(b) => {
                    if let Some(c) = piece.strip_prefix('@') {
                        b.comment.push(c.trim().to_string());
                    } else if !piece.is_empty() {
                        b.stmts.push((*ln, piece.to_string()));
                    }
                }
                None if brace == Some('{') => {
                    let name = if piece.is_empty() {
                        match items.pop() {
                            Some(Item::Stmt(_, s)) => s,
                            other => {
                                items.extend(other);
                                format!("check_{ln}")
                            }
                        }
                    } else {
                        piece.to_string()
                    };
                    block = Some(Block { name: name.trim().to_string(), line: *ln, comment: vec![], stmts: vec![] });
                }
                None if !piece.is_empty() => items.push(Item::Stmt(*ln, piece.to_string())),
                None => {}
            }
            if brace == Some('}') {
                match block.take() {
                    Some(b) => items.push(Item::Block(b)),
                    None => notes.push(Note { line: *ln, message: "unmatched '}'".into() }),
                }
            }
            match cut {
                Some(i) => rest = &rest[i + 1..],
                None => break,
            }
        }
    }
    if let Some(b) = block {
        notes.push(Note { line: b.line, message: format!("rule check {} is not closed", b.name) });
        items.push(Item::Block(b));
    }
    items
}

/// Lines kept by the preprocessor, with `defines` set beforehand.
fn preprocess(text: &str, defines: &[String], notes: &mut Vec<Note>) -> Vec<(usize, String)> {
    let mut defs: HashMap<String, String> = defines.iter().map(|d| (d.to_ascii_uppercase(), String::new())).collect();
    // (branch taken, parent active)
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let active = |stack: &[(bool, bool)]| stack.last().is_none_or(|&(t, p)| t && p);
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let ln = i + 1;
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#') {
            let mut w = rest.split_whitespace();
            let dir = w.next().unwrap_or_default().to_ascii_uppercase();
            let name = w.next().unwrap_or_default().to_ascii_uppercase();
            let value = w.next().map(str::to_string);
            let on = active(&stack);
            match dir.as_str() {
                "DEFINE" if on => {
                    defs.insert(name, value.unwrap_or_default());
                }
                "UNDEFINE" if on => {
                    defs.remove(&name);
                }
                "IFDEF" | "IFNDEF" => {
                    let set = defs.get(&name).is_some_and(|v| value.as_ref().is_none_or(|x| x.eq_ignore_ascii_case(v)));
                    stack.push((set == (dir == "IFDEF"), on));
                }
                "ELSE" => match stack.last_mut() {
                    Some(top) => top.0 = !top.0,
                    None => notes.push(Note { line: ln, message: "#ELSE without #IFDEF".into() }),
                },
                "ENDIF" => {
                    if stack.pop().is_none() {
                        notes.push(Note { line: ln, message: "#ENDIF without #IFDEF".into() });
                    }
                }
                "DEFINE" | "UNDEFINE" => {}
                _ if on => notes.push(Note { line: ln, message: format!("preprocessor directive #{dir} ignored") }),
                _ => {}
            }
            continue;
        }
        if active(&stack) && !t.is_empty() {
            out.push((ln, line.to_string()));
        }
    }
    if !stack.is_empty() {
        notes.push(Note { line: text.lines().count(), message: "#IFDEF without #ENDIF".into() });
    }
    out
}

#[derive(Default, Debug, Clone, Copy)]
struct Constraint {
    /// Lower bound (value, inclusive).
    lo: Option<(f64, bool)>,
    hi: Option<(f64, bool)>,
}

struct Builder<'a> {
    tech: &'a Tech,
    deck: Deck,
    names: HashMap<String, usize>,
    vars: HashMap<String, f64>,
    layer_map: HashMap<u32, Vec<(u16, u16)>>,
    tmp: usize,
    /// Modifiers ignored with a note in the current check.
    ignored: Vec<String>,
}

type Res<T> = Result<T, String>;

struct Toks<'t> {
    t: &'t [String],
    i: usize,
}

impl Toks<'_> {
    fn peek(&self) -> Option<&str> {
        self.t.get(self.i).map(String::as_str)
    }
    fn peek_up(&self) -> String {
        self.peek().unwrap_or_default().to_ascii_uppercase()
    }
    fn next(&mut self) -> Option<&str> {
        let t = self.t.get(self.i).map(String::as_str);
        self.i += 1;
        t
    }
    fn is_op(&self) -> bool {
        matches!(self.peek(), Some("<" | "<=" | ">" | ">=" | "==" | "!="))
    }
    fn done(&self) -> bool {
        self.i >= self.t.len()
    }
}

impl<'a> Builder<'a> {
    fn dbu(&self, um: f64) -> i64 {
        (um * self.tech.dbu_per_um as f64).round() as i64
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.names.get(&name.to_ascii_lowercase()).copied().or_else(|| self.tech.layer_index(name))
    }

    fn taken(&self, name: &str) -> bool {
        self.tech.layer_index(name).is_some() || self.deck.derived.iter().any(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Add a derived layer (named after `name` when given and free) and return its index.
    fn derive(&mut self, name: Option<&str>, expr: LayerExpr) -> usize {
        let mut n = match name {
            Some(n) => n.to_string(),
            None => {
                self.tmp += 1;
                format!("_svrf{}", self.tmp)
            }
        };
        if self.taken(&n) {
            let base = n.clone();
            let mut k = 2;
            while self.taken(&format!("{base}_{k}")) {
                k += 1;
            }
            n = format!("{base}_{k}");
        }
        self.deck.derived.push(DerivedLayer { name: n, expr });
        self.tech.layers.len() + self.deck.derived.len() - 1
    }

    fn number(&self, tok: Option<&str>) -> Res<f64> {
        let t = tok.ok_or("missing number")?;
        t.parse::<f64>()
            .ok()
            .or_else(|| self.vars.get(&t.to_ascii_uppercase()).copied())
            .ok_or_else(|| format!("'{t}' is not a number or a numeric VARIABLE"))
    }

    fn constraint(&self, p: &mut Toks) -> Res<Constraint> {
        let mut c = Constraint::default();
        while p.is_op() {
            let op = p.next().expect("operator").to_string();
            let v = self.number(p.next())?;
            match op.as_str() {
                "<" => c.hi = Some((v, false)),
                "<=" => c.hi = Some((v, true)),
                ">" => c.lo = Some((v, false)),
                ">=" => c.lo = Some((v, true)),
                "==" => (c.lo, c.hi) = (Some((v, true)), Some((v, true))),
                _ => return Err(format!("constraint '{op} {v}' is not supported")),
            }
        }
        Ok(c)
    }

    fn expr(&mut self, p: &mut Toks) -> Res<usize> {
        let mut left = self.term(p)?;
        loop {
            let up = p.peek_up();
            let bool_op = match up.as_str() {
                "AND" => Some(BoolOp::And),
                "OR" => Some(BoolOp::Or),
                "XOR" => Some(BoolOp::Xor),
                _ => None,
            };
            if let Some(op) = bool_op {
                p.next();
                let b = self.term(p)?;
                left = self.derive(None, LayerExpr::Bool { a: left, op, b });
                continue;
            }
            let mut not = false;
            if up == "NOT" {
                p.next();
                not = true;
            }
            let sel_kw = p.peek_up();
            match Select::from_svrf(&sel_kw) {
                Some(how) => {
                    p.next();
                    if p.peek_up() == "EDGE" {
                        return Err(format!("{sel_kw} EDGE is an edge operation (not supported)"));
                    }
                    let b = self.term(p)?;
                    if p.is_op() {
                        self.constraint(p)?;
                        self.ignored.push(format!("{sel_kw} count"));
                    }
                    while matches!(p.peek_up().as_str(), "SINGULAR" | "ALSO" | "BY" | "NET" | "LAYER") {
                        let w = p.next().expect("peeked").to_ascii_uppercase();
                        if w == "BY" {
                            p.next();
                            self.ignored.push(format!("{sel_kw} BY"));
                        }
                    }
                    left = self.derive(None, LayerExpr::Select { a: left, how, not, b });
                }
                None if not => {
                    let b = self.term(p)?;
                    left = self.derive(None, LayerExpr::Bool { a: left, op: BoolOp::Not, b });
                }
                None => {
                    if matches!(up.as_str(), "CUT" | "STRADDLE" | "COINCIDENT" | "IN" | "WITH") {
                        return Err(format!("operation {up} is not supported"));
                    }
                    return Ok(left);
                }
            }
        }
    }

    fn term(&mut self, p: &mut Toks) -> Res<usize> {
        let tok = p.next().ok_or("expected a layer")?.to_string();
        let up = tok.to_ascii_uppercase();
        match up.as_str() {
            "(" => {
                let l = self.expr(p)?;
                match p.next() {
                    Some(")") => Ok(l),
                    other => Err(format!("expected ')' but found {other:?}")),
                }
            }
            "SIZE" => {
                let a = self.term(p)?;
                if p.next().map(str::to_ascii_uppercase).as_deref() != Some("BY") {
                    return Err("SIZE needs BY".into());
                }
                let by = self.dbu(self.number(p.next())?);
                match p.peek_up().as_str() {
                    "OVERUNDER" | "UNDEROVER" => {
                        let first = if p.next().expect("peeked").eq_ignore_ascii_case("OVERUNDER") { by } else { -by };
                        let s = self.derive(None, LayerExpr::Size { a, by: first });
                        Ok(self.derive(None, LayerExpr::Size { a: s, by: -first }))
                    }
                    "INSIDE" | "OUTSIDE" => Err("SIZE ... INSIDE OF / OUTSIDE OF is not supported".into()),
                    _ => {
                        if matches!(p.peek_up().as_str(), "TRUNCATE" | "STEP") {
                            let w = p.next().expect("peeked").to_ascii_uppercase();
                            p.next();
                            self.ignored.push(format!("SIZE {w}"));
                        }
                        Ok(self.derive(None, LayerExpr::Size { a, by }))
                    }
                }
            }
            "HOLES" => {
                let a = self.term(p)?;
                if matches!(p.peek_up().as_str(), "INNER" | "EMPTY") {
                    self.ignored.push(format!("HOLES {}", p.next().expect("peeked")));
                }
                Ok(self.derive(None, LayerExpr::Holes { a }))
            }
            "EXTENTS" => {
                let a = self.term(p)?;
                Ok(self.derive(None, LayerExpr::Extents { a }))
            }
            "COPY" | "MERGE" => self.term(p),
            "AND" | "OR" | "NOT" | "XOR" => {
                let a = self.term(p)?;
                let b = self.term(p)?;
                let op = match up.as_str() {
                    "AND" => BoolOp::And,
                    "OR" => BoolOp::Or,
                    "NOT" => BoolOp::Not,
                    _ => BoolOp::Xor,
                };
                Ok(self.derive(None, LayerExpr::Bool { a, op, b }))
            }
            "AREA" => {
                let a = self.term(p)?;
                let c = self.constraint(p)?;
                Ok(self.area_select(a, c))
            }
            "INT" | "INTERNAL" | "EXT" | "EXTERNAL" | "ENC" | "ENCLOSURE" | "DENSITY" => {
                let rule = self.check(&up, p)?;
                Ok(self.derive(None, LayerExpr::Check(Box::new(rule))))
            }
            ")" | "<" | "<=" | ">" | ">=" | "==" | "!=" => Err(format!("unexpected '{tok}'")),
            _ => match self.lookup(&tok) {
                Some(l) => Ok(l),
                None if is_operation(&up) => Err(format!("operation {up} is not supported")),
                None => Err(format!("unknown layer '{tok}'")),
            },
        }
    }

    fn area_select(&mut self, a: usize, c: Constraint) -> usize {
        let d = self.tech.dbu_per_um as f64;
        let um2 = |v: f64| (v * d * d).round() as i128;
        let min = c.lo.map(|(v, incl)| um2(v) + i128::from(!incl));
        let max = c.hi.map(|(v, incl)| um2(v) + i128::from(incl));
        self.derive(None, LayerExpr::AreaRange { a, min, max })
    }

    /// Upper bound of a spacing-like constraint in dbu (`<= d` is `< d + 1 dbu`).
    fn upper(&mut self, kw: &str, c: Constraint) -> Res<i64> {
        let (v, incl) = c.hi.ok_or_else(|| format!("{kw} without an upper bound ('< value') is not supported"))?;
        if c.lo.is_some() {
            self.ignored.push(format!("{kw} lower bound"));
        }
        Ok(self.dbu(v) + i64::from(incl))
    }

    /// A check statement (keyword already read) as a rule.
    fn check(&mut self, kw: &str, p: &mut Toks) -> Res<Rule> {
        let a = self.term(p)?;
        let b = if !p.is_op() && !p.done() && p.peek() != Some(")") { Some(self.term(p)?) } else { None };
        let c = self.constraint(p)?;
        let (mut window, mut step) = (None, None);
        while let Some(t) = p.peek().filter(|t| *t != ")") {
            let up = t.to_ascii_uppercase();
            p.next();
            match up.as_str() {
                "WINDOW" if kw == "DENSITY" => window = Some(self.number(p.next())?),
                "STEP" if kw == "DENSITY" => step = Some(self.number(p.next())?),
                "[" => return Err("DENSITY expressions in [ ] are not supported".into()),
                "<" | "<=" | ">" | ">=" | "==" | "!=" => {
                    p.next();
                }
                _ if up.parse::<f64>().is_ok() || up.starts_with('"') => {}
                _ if HARMLESS.contains(&up.as_str()) => {}
                _ => self.ignored.push(up),
            }
        }
        Ok(match (kw, b) {
            ("INT" | "INTERNAL", None) => Rule::Width { layer: a, min: self.upper("INT", c)? },
            ("INT" | "INTERNAL", Some(_)) => return Err("INT with two layers is not supported".into()),
            ("EXT" | "EXTERNAL", None) => Rule::Space { layer: a, min: self.upper("EXT", c)? },
            ("EXT" | "EXTERNAL", Some(b)) => Rule::Separation { a, b, min: self.upper("EXT", c)? },
            ("ENC" | "ENCLOSURE", Some(b)) => Rule::Enclosure { inner: a, outer: b, min: self.upper("ENC", c)? },
            ("ENC" | "ENCLOSURE", None) => return Err("ENC needs two layers".into()),
            ("DENSITY", None) => {
                let pct = |v: f64| (v * 100.0).clamp(0.0, 100.0);
                let min = c.hi.map_or(0.0, |(v, _)| pct(v));
                let max = c.lo.map_or(100.0, |(v, _)| pct(v));
                if min > max {
                    return Err("DENSITY with both bounds reporting the range between them is not supported".into());
                }
                let window = window.map_or(1_000_000_000_000, |w| self.dbu(w)).max(1);
                let step = step.map_or(window / 2, |s| self.dbu(s)).max(1);
                Rule::Density { layer: a, min, max, window, step }
            }
            ("DENSITY", Some(_)) => return Err("DENSITY of several layers is not supported".into()),
            _ => return Err(format!("{kw} is not supported")),
        })
    }

    /// Translate one statement that outputs geometry (in a rule check).
    fn output(&mut self, toks: &[String]) -> Res<Rule> {
        let mut p = Toks { t: toks, i: 0 };
        let up = p.peek_up();
        let rule = match up.as_str() {
            "INT" | "INTERNAL" | "EXT" | "EXTERNAL" | "ENC" | "ENCLOSURE" | "DENSITY" => {
                p.next();
                self.check(&up, &mut p)?
            }
            "AREA" => {
                p.next();
                let a = self.term(&mut p)?;
                let c = self.constraint(&mut p)?;
                match c {
                    Constraint { lo: None, hi: Some((v, incl)) } => {
                        let d = self.tech.dbu_per_um as f64;
                        Rule::Area { layer: a, min: (v * d * d).round() as i128 + i128::from(incl) }
                    }
                    c => Rule::Flag { layer: self.area_select(a, c) },
                }
            }
            _ => Rule::Flag { layer: self.expr(&mut p)? },
        };
        if !p.done() {
            return Err(format!("unexpected '{}'", p.t[p.i..].join(" ")));
        }
        Ok(rule)
    }

    /// `name = expression`.
    fn assign(&mut self, name: &str, toks: &[String]) -> Res<()> {
        let mut p = Toks { t: toks, i: 0 };
        let l = self.expr(&mut p)?;
        if !p.done() {
            return Err(format!("unexpected '{}'", p.t[p.i..].join(" ")));
        }
        // Name the result after the SVRF layer when it is new; an alias otherwise.
        let n = self.tech.layers.len();
        let fresh = l + 1 == n + self.deck.derived.len() && l >= n && self.deck.derived[l - n].name.starts_with("_svrf");
        let idx = if fresh {
            let d = self.deck.derived.pop().expect("fresh layer");
            self.derive(Some(name), d.expr)
        } else {
            l
        };
        self.names.insert(name.to_ascii_lowercase(), idx);
        Ok(())
    }

    /// `LAYER name n ...`.
    fn layer_def(&mut self, name: &str, nums: &[String]) -> Res<()> {
        let mut found = Vec::new();
        for n in nums {
            let n: u32 = n.parse().map_err(|_| format!("bad layer number '{n}'"))?;
            let pairs = self.layer_map.get(&n);
            for (i, l) in self.tech.layers.iter().enumerate() {
                let hit = match pairs {
                    Some(ps) => ps.contains(&(l.gds_layer, l.gds_datatype)),
                    None => u32::from(l.gds_layer) == n,
                };
                if hit && !found.contains(&i) {
                    found.push(i);
                }
            }
        }
        let idx = match found.as_slice() {
            [one] => *one,
            _ => self.derive(Some(name), LayerExpr::Union(found.clone())),
        };
        self.names.insert(name.to_ascii_lowercase(), idx);
        if found.is_empty() {
            return Err(format!("layer {name} ({}) has no technology layer; it is empty", nums.join(" ")));
        }
        Ok(())
    }
}

impl Select {
    fn from_svrf(up: &str) -> Option<Select> {
        Some(match up {
            "INTERACT" => Select::Interact,
            "INSIDE" => Select::Inside,
            "OUTSIDE" => Select::Outside,
            "ENCLOSE" => Select::Enclose,
            "TOUCH" => Select::Touch,
            _ => return None,
        })
    }
}

fn is_operation(up: &str) -> bool {
    [
        "WITH",
        "RECTANGLE",
        "ANGLE",
        "LENGTH",
        "EXPAND",
        "CONVEX",
        "COINCIDENT",
        "SHRINK",
        "GROW",
        "ROTATE",
        "SHIFT",
        "DONUT",
        "VERTEX",
        "PERIMETER",
        "NET",
        "STAMP",
        "RECTANGLES",
        "OFFGRID",
        "DFM",
        "TVF",
        "EXTENT",
        "BOOLEAN",
        "INTERNAL1",
        "CUT",
        "STRADDLE",
        "OR_EDGE",
    ]
    .contains(&up)
}

/// Translate SVRF `text` for `tech`, with `defines` set as by `#DEFINE`.
pub fn translate(text: &str, tech: &Tech, defines: &[String]) -> Translation {
    let mut notes = Vec::new();
    let lines = preprocess(&strip_comments(text), defines, &mut notes);
    let items = structure(&lines, &mut notes);
    let mut b = Builder {
        tech,
        deck: Deck::default(),
        names: HashMap::new(),
        vars: HashMap::new(),
        layer_map: HashMap::new(),
        tmp: 0,
        ignored: Vec::new(),
    };
    // Specification statements first: LAYER MAP and VARIABLE apply to the whole file.
    for item in &items {
        let Item::Stmt(ln, s) = item else { continue };
        let t = tokenize(s);
        let up: Vec<String> = t.iter().map(|x| x.to_ascii_uppercase()).collect();
        match up.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["LAYER", "MAP", g, "DATATYPE", d, n] => match (g.parse::<u16>(), d.parse::<u16>(), n.parse::<u32>()) {
                (Ok(g), Ok(d), Ok(n)) => b.layer_map.entry(n).or_default().push((g, d)),
                _ => notes.push(Note { line: *ln, message: format!("LAYER MAP '{s}' is not supported") }),
            },
            ["LAYER", "MAP", _, "TEXTTYPE", ..] => {}
            ["LAYER", "MAP", ..] => notes.push(Note { line: *ln, message: format!("LAYER MAP '{s}' is not supported") }),
            ["VARIABLE", name, ..] if t.len() == 3 => match t[2].parse::<f64>() {
                Ok(v) => {
                    b.vars.insert(name.to_string(), v);
                }
                Err(_) => notes.push(Note { line: *ln, message: format!("VARIABLE {} is not a number", t[1]) }),
            },
            ["VARIABLE", ..] => notes.push(Note { line: *ln, message: format!("VARIABLE '{s}' is not supported") }),
            _ => {}
        }
    }
    let (mut checks, mut translated, mut lvs) = (0, 0, 0);
    for item in items {
        match item {
            Item::Stmt(ln, s) => {
                let t = tokenize(&s);
                let first = t.first().map(|x| x.to_ascii_uppercase()).unwrap_or_default();
                let res = if t.get(1).map(String::as_str) == Some("=") {
                    b.assign(&t[0], &t[2..])
                } else if first == "LAYER" && t.get(1).is_some_and(|x| !x.eq_ignore_ascii_case("MAP")) {
                    b.layer_def(&t[1], &t[2..])
                } else if first == "VARIABLE" || (first == "LAYER") {
                    Ok(())
                } else if first == "INCLUDE" {
                    Err("INCLUDE was not resolved (load the deck from a file)".into())
                } else if SPEC.contains(&first.as_str()) {
                    Ok(())
                } else if LVS.contains(&first.as_str()) {
                    lvs += 1;
                    Ok(())
                } else {
                    Err(format!("statement '{s}' is not supported"))
                };
                if let Err(message) = res {
                    notes.push(Note { line: ln, message });
                }
            }
            Item::Block(blk) => {
                checks += 1;
                let saved = b.names.clone();
                b.ignored.clear();
                let mut rules = Vec::new();
                let mut err = None;
                for (ln, s) in &blk.stmts {
                    let t = tokenize(s);
                    let r = if t.get(1).map(String::as_str) == Some("=") {
                        b.assign(&t[0], &t[2..]).map(|_| None)
                    } else {
                        b.output(&t).map(Some)
                    };
                    match r {
                        Ok(Some(rule)) => rules.push(rule),
                        Ok(None) => {}
                        Err(e) => {
                            err = Some((*ln, e));
                            break;
                        }
                    }
                }
                b.names = saved;
                let comment = (!blk.comment.is_empty()).then(|| blk.comment.join(" "));
                match err {
                    Some((ln, e)) => notes.push(Note { line: ln, message: format!("rule check {} left out: {e}", blk.name) }),
                    None if rules.is_empty() => {
                        notes.push(Note { line: blk.line, message: format!("rule check {} has no output", blk.name) })
                    }
                    None => {
                        translated += 1;
                        if !b.ignored.is_empty() {
                            b.ignored.sort();
                            b.ignored.dedup();
                            notes.push(Note {
                                line: blk.line,
                                message: format!("rule check {}: {} ignored; results may differ", blk.name, b.ignored.join(", ")),
                            });
                        }
                        for rule in rules {
                            b.deck.rules.push(NamedRule { name: blk.name.clone(), rule, comment: comment.clone() });
                        }
                    }
                }
            }
        }
    }
    if lvs > 0 {
        notes.push(Note { line: 0, message: format!("{lvs} connectivity / LVS statement(s) skipped") });
    }
    notes.sort_by_key(|n| n.line);
    Translation { deck: b.deck, notes, checks, translated }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::{parse_deck, run};
    use crate::geom::IRect;
    use crate::hier::NoCells;
    use crate::layout::Layout;

    const DECK: &str = r#"// generic180 rules, SVRF style
LAYOUT PATH "chip.gds"   // specification: skipped
PRECISION 1000
LAYER MAP 7 DATATYPE 0 1007
LAYER M1 1007
LAYER POLY 5
LAYER ACT 2
LAYER CONT 6
LAYER NW 1
LAYER FILL 99
VARIABLE M1_W 0.23
#DEFINE STRICT
GATE = POLY AND ACT
CONNECT M1 CONT
/* block comment
   over two lines */
M1.W { @ Metal1 width < 0.23 um
  INT M1 < M1_W ABUT < 90 SINGULAR REGION }
M1.S {
  @ Metal1 spacing
  EXT M1 < 0.23
}
CO.EN { ENC CONT M1 <= 0.059 }
#IFDEF STRICT
M1.A { AREA M1 < 0.2 }
#ELSE
BAD { nonsense here }
#ENDIF
#IFNDEF STRICT
BAD2 { more nonsense }
#ENDIF
PO.FLOAT { @ poly not touching active
  POLY NOT INTERACT ACT }
G.W { INT GATE < 0.18 OPPOSITE }
P.EX { INT POLY ACT < 0.2 }
M1.DEN { DENSITY M1 < 0.2 WINDOW 10 STEP 5 }
M1.HOLE { X = HOLES M1
  AREA X < 1 }
M1.BIG { (SIZE M1 BY 0.1) AND NW }
E.L { LENGTH M1 < 1 }
"#;

    #[test]
    fn detects_svrf() {
        assert!(looks_like_svrf(DECK));
        assert!(!looks_like_svrf(&crate::drc::default_deck_text(&Tech::generic())));
        assert!(!looks_like_svrf("layer = metal1 and poly\nwidth layer 1\n"));
        assert!(looks_like_svrf("LAYER M1 7\n"));
    }

    #[test]
    fn translates_subset_with_notes() {
        let tech = Tech::generic();
        let t = translate(DECK, &tech, &[]);
        let names: Vec<&str> = t.deck.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["M1.W", "M1.S", "CO.EN", "M1.A", "PO.FLOAT", "G.W", "M1.DEN", "M1.HOLE", "M1.BIG"], "{:?}", t.notes);
        assert_eq!((t.checks, t.translated), (11, 9));
        let li = |n: &str| tech.layer_index(n).unwrap();
        assert_eq!(t.deck.rules[0].rule, Rule::Width { layer: li("metal1"), min: 230 });
        assert_eq!(t.deck.rules[0].comment.as_deref(), Some("Metal1 width < 0.23 um"));
        assert_eq!(t.deck.rules[1].rule, Rule::Space { layer: li("metal1"), min: 230 });
        assert_eq!(t.deck.rules[2].rule, Rule::Enclosure { inner: li("contact"), outer: li("metal1"), min: 60 });
        assert_eq!(t.deck.rules[3].rule, Rule::Area { layer: li("metal1"), min: 200_000 });
        assert!(matches!(t.deck.rules[6].rule, Rule::Density { min: 20.0, max: 100.0, window: 10_000, step: 5_000, .. }));
        let msgs: Vec<String> = t.notes.iter().map(|n| n.to_string()).collect();
        let has = |line: usize, part: &str| t.notes.iter().any(|n| n.line == line && n.message.contains(part));
        assert!(has(10, "layer FILL"), "{msgs:#?}");
        assert!(has(34, "OPPOSITE ignored"), "{msgs:#?}");
        assert!(has(35, "INT with two layers"), "{msgs:#?}");
        assert!(has(40, "LENGTH is not supported"), "{msgs:#?}");
        assert!(has(0, "1 connectivity"), "{msgs:#?}");
        assert!(!msgs.iter().any(|m| m.contains("BAD")), "{msgs:#?}");
    }

    type Shape<'a> = (&'a str, (i64, i64, i64, i64));

    fn layout(tech: &Tech, shapes: &[Shape]) -> Layout {
        let mut l = Layout::default();
        for &(n, (x0, y0, x1, y1)) in shapes {
            l.add_rect(tech.layer_index(n).unwrap(), x0, y0, x1, y1);
        }
        l
    }

    /// The translated rules flag the same geometry as the equivalent IC Studio deck.
    #[test]
    fn matches_native_deck() {
        let tech = Tech::generic();
        let native = parse_deck(
            "M1.W: width metal1 0.23\nM1.S: space metal1 0.23\nCO.EN: enclosure contact metal1 0.06\nM1.A: area metal1 0.2\nf = poly not interact active\nPO.FLOAT: flag f\n",
            &tech,
        )
        .unwrap();
        let svrf = translate(DECK, &tech, &[]).deck;
        let lay = layout(
            &tech,
            &[
                ("metal1", (0, 0, 200, 2000)),
                ("metal1", (400, 0, 1400, 1000)),
                ("metal1", (1600, 0, 2600, 1000)),
                ("contact", (1650, 100, 1870, 320)),
                ("metal1", (5000, 0, 5300, 300)),
                ("active", (0, 3000, 1000, 4000)),
                ("poly", (400, 2800, 580, 4200)),
                ("poly", (3000, 3000, 3180, 4000)),
            ],
        );
        let markers = |deck: &Deck, rule: &str| -> Vec<IRect> {
            let mut v: Vec<IRect> =
                run(deck, &lay, &NoCells, &tech).violations.into_iter().filter(|v| v.rule == rule).map(|v| v.marker).collect();
            v.sort_by_key(|r| (r.x0, r.y0));
            v
        };
        for rule in ["M1.W", "M1.S", "CO.EN", "M1.A", "PO.FLOAT"] {
            let (a, b) = (markers(&native, rule), markers(&svrf, rule));
            assert!(!a.is_empty(), "{rule}: the test layout should violate it");
            assert_eq!(a, b, "{rule}");
        }
        assert_eq!(markers(&svrf, "PO.FLOAT"), [IRect::new(3000, 3000, 3180, 4000)]);
    }

    #[test]
    fn preprocessor_and_structure() {
        let mut notes = vec![];
        let lines =
            preprocess("#DEFINE A 1\n#IFDEF A 2\nx\n#ELSE\ny\n#ENDIF\n#IFDEF A 1\nz\n#ENDIF\n", &["B".into()], &mut notes);
        assert_eq!(lines.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(), ["y", "z"]);
        let items = structure(&[(1, "R1".into()), (2, "{ EXT a < 1 } R2 { INT b < 2".into()), (3, "}".into())], &mut notes);
        let blocks: Vec<(&str, usize)> = items
            .iter()
            .filter_map(|i| if let Item::Block(b) = i { Some((b.name.as_str(), b.stmts.len())) } else { None })
            .collect();
        assert_eq!(blocks, [("R1", 1), ("R2", 1)]);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(tokenize("INT m1<=0.2 ABUT<90 (a)"), ["INT", "m1", "<=", "0.2", "ABUT", "<", "90", "(", "a", ")"]);
        assert_eq!(strip_comments("a // c\nb /* x\ny */ c \"//k\""), "a \nb \n c \"//k\"");
    }

    #[test]
    fn includes_are_inlined() {
        let dir = std::env::temp_dir().join(format!("ics-svrf-inc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("m1.svrf"), "M1.W { INT M1 < 0.23 }\n").unwrap();
        let (text, errs) = inline_includes("LAYER M1 7\nINCLUDE \"sub/m1.svrf\"\nINCLUDE missing.svrf\n", &dir);
        assert!(text.contains("M1.W { INT M1 < 0.23 }"), "{text}");
        assert_eq!(errs.len(), 1);
        assert_eq!(translate(&text, &Tech::generic(), &[]).translated, 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
