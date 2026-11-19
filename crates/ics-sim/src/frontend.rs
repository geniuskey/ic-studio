//! SPICE front-end: turns a hierarchical deck as found in PDKs and foundry flows into the flat
//! deck [`crate::circuit::parse_deck`] reads.
//!
//! - `.include` / `.inc` files and `.lib <file> <section>` library sections (paths relative to
//!   the including file, or to the deck's folder)
//! - `.param` with expressions, `{expr}` and `'expr'` values, bare parameter names as values
//! - `.subckt` / `.ends` with port lists and default parameters (`params:` optional), nested
//!   definitions and instances `X... nodes subckt k=v`, flattened as `m.x1.m1` and `x1.net`
//! - `.model` cards inside subcircuits, evaluated per instance with the instance's parameters
//!   (identical cards are shared)
//! - model binning: `.model nch.1 nmos lmin= lmax= wmin= wmax=` cards selected by an
//!   instance's `l` and `w`
//! - `.global` nodes
//!
//! Every output line remembers where it came from, so errors and warnings of the flat deck
//! point at the user's file and line.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ics_core::units::parse_spice;

use crate::SimError;
use crate::expr::{ExprError, Params, eval, fmt_num};

const MAX_INCLUDE_DEPTH: usize = 32;
const MAX_SUBCKT_DEPTH: usize = 64;

/// Where a line of the flat deck comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    /// Included file, or None for the deck itself.
    pub file: Option<PathBuf>,
    pub line: usize,
}

impl Origin {
    fn describe(&self) -> String {
        match &self.file {
            None => format!("line {}", self.line),
            Some(f) => format!("{}:{}", f.display(), self.line),
        }
    }
}

/// A flattened deck.
#[derive(Debug, Clone, Default)]
pub struct Expanded {
    /// The flat deck: a title line, then one logical line per element or command.
    pub deck: String,
    /// `origins[i]` is the source of flat-deck line `i + 1` (the title line has the deck's line 1).
    pub origins: Vec<Origin>,
    pub warnings: Vec<String>,
}

impl Expanded {
    /// The source of a flat-deck line (1-based).
    pub fn origin(&self, line: usize) -> Option<&Origin> {
        self.origins.get(line.checked_sub(1)?)
    }

    /// `e` from parsing the flat deck, pointing at the source line.
    pub fn map_error(&self, e: SimError) -> SimError {
        match e {
            SimError::Parse { line, msg } => match self.origin(line) {
                Some(Origin { file: None, line }) => SimError::Parse { line: *line, msg },
                Some(o) => SimError::Engine(format!("{}: {msg}", o.describe())),
                None => SimError::Parse { line, msg },
            },
            e => e,
        }
    }

    /// A warning from parsing the flat deck, with `line N:` pointing at the source line.
    pub fn map_warning(&self, w: &str) -> String {
        if let Some(rest) = w.strip_prefix("line ")
            && let Some((n, msg)) = rest.split_once(':')
            && let Ok(n) = n.parse::<usize>()
            && let Some(o) = self.origin(n)
        {
            return format!("{}:{msg}", o.describe());
        }
        w.to_string()
    }
}

#[derive(Debug, Clone)]
struct Line {
    origin: Origin,
    text: String,
}

#[derive(Debug, Clone, Default)]
struct Subckt {
    ports: Vec<String>,
    /// Default parameters in definition order: (name, expression).
    defaults: Vec<(String, String)>,
    body: Vec<Line>,
}

/// A model card as visible in a scope: the name it is emitted under and its bin limits.
#[derive(Debug, Clone)]
struct ModelRef {
    emit: String,
    /// Element letter of the devices that use it (`m`, `d`, `q`, `j`).
    kind: char,
    /// (lmin, lmax, wmin, wmax) for a binned card.
    bin: Option<[f64; 4]>,
}

#[derive(Default)]
struct Ctx {
    subckts: HashMap<String, Subckt>,
    globals: HashSet<String>,
    out: Vec<Line>,
    warnings: Vec<String>,
    /// Evaluated model cards (`type params`) already emitted, by content.
    model_cards: HashMap<String, String>,
    /// `.option scale`: instance lengths are in these units (bins hold metres).
    scale: f64,
}

fn err_at(o: &Origin, msg: impl std::fmt::Display) -> SimError {
    match &o.file {
        None => SimError::Parse { line: o.line, msg: msg.to_string() },
        Some(_) => SimError::Engine(format!("{}: {msg}", o.describe())),
    }
}

fn unquote(s: &str) -> &str {
    s.trim_matches(['"', '\''])
}

/// Logical lines of `text`: continuations joined, comments dropped.
/// Logical lines of `text`, translated from the Spectre language where it uses it.
fn logical_any(text: &str, file: Option<&Path>, warnings: &mut Vec<String>) -> Vec<Line> {
    if !ics_core::spectre::is_spectre(text, file) {
        return logical(text, file);
    }
    let t = ics_core::spectre::to_spice(text, ics_core::spectre::starts_in_spectre(file));
    let at = |w: String| match file {
        Some(f) => format!("{}: {w}", f.display()),
        None => w,
    };
    warnings.extend(t.warnings.into_iter().map(at));
    t.lines
        .into_iter()
        .map(|l| Line { origin: Origin { file: file.map(Path::to_path_buf), line: l.line }, text: l.text })
        .collect()
}

fn logical(text: &str, file: Option<&Path>) -> Vec<Line> {
    let mut out: Vec<Line> = vec![];
    for (n, raw) in text.lines().enumerate() {
        let mut line = raw.split(';').next().unwrap_or("");
        // `$` starts an inline comment (HSPICE) when preceded by whitespace.
        if let Some(i) = line.find(" $") {
            line = &line[..i];
        }
        let t = line.trim();
        if t.is_empty() || t.starts_with('*') || t.starts_with('$') {
            continue;
        }
        if let Some(cont) = t.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.text.push(' ');
                last.text.push_str(cont.trim());
            }
            continue;
        }
        out.push(Line { origin: Origin { file: file.map(Path::to_path_buf), line: n + 1 }, text: t.to_string() });
    }
    out
}

fn cmd(text: &str) -> String {
    text.split_whitespace().next().unwrap_or("").to_ascii_lowercase()
}

fn resolve_path(name: &str, dir: Option<&Path>) -> PathBuf {
    let p = PathBuf::from(unquote(name));
    match dir {
        Some(d) if p.is_relative() => d.join(p),
        _ => p,
    }
}

fn read_file(path: &Path, at: &Origin) -> Result<String, SimError> {
    std::fs::read_to_string(path).map_err(|e| err_at(at, format!("cannot read '{}': {e}", path.display())))
}

/// The lines of `.lib <section>` ... `.endl` in `lines`.
fn lib_section(lines: Vec<Line>, section: &str) -> Option<Vec<Line>> {
    let mut out = None;
    let mut depth = 0usize;
    for l in lines {
        let c = cmd(&l.text);
        let args: Vec<&str> = l.text.split_whitespace().collect();
        if out.is_none() {
            if c == ".lib" && args.len() == 2 && unquote(args[1]).eq_ignore_ascii_case(section) {
                out = Some(vec![]);
            }
            continue;
        }
        if c == ".lib" && args.len() == 2 {
            depth += 1;
        } else if c == ".endl" {
            if depth == 0 {
                return out;
            }
            depth -= 1;
        }
        out.as_mut().unwrap().push(l);
    }
    out
}

/// Inline `.include` and `.lib file section` statements; drop `.lib section` definitions that
/// are not selected.
fn load(lines: Vec<Line>, dir: Option<&Path>, depth: usize, out: &mut Vec<Line>, warn: &mut Vec<String>) -> Result<(), SimError> {
    let mut skip = 0usize;
    for l in lines {
        let c = cmd(&l.text);
        let args: Vec<&str> = l.text.split_whitespace().collect();
        if skip > 0 {
            if c == ".lib" && args.len() == 2 {
                skip += 1;
            } else if c == ".endl" {
                skip -= 1;
            }
            continue;
        }
        let file_dir = |f: &Path| f.parent().map(Path::to_path_buf);
        match c.as_str() {
            ".include" | ".inc" | ".lib" if depth >= MAX_INCLUDE_DEPTH => {
                return Err(err_at(&l.origin, "includes nested too deeply (recursive include?)"));
            }
            ".include" | ".inc" => {
                let arg = l.text.split_once(char::is_whitespace).map(|x| x.1.trim()).unwrap_or("");
                if arg.is_empty() {
                    return Err(err_at(&l.origin, format!("'{c}' without a file name")));
                }
                let path = resolve_path(arg, dir);
                let text = read_file(&path, &l.origin)?;
                let lines = logical_any(&text, Some(&path), warn);
                load(lines, file_dir(&path).as_deref(), depth + 1, out, warn)?;
            }
            ".lib" if args.len() >= 3 => {
                let path = resolve_path(args[1], dir);
                let text = read_file(&path, &l.origin)?;
                let section = unquote(args[2]);
                let lines = lib_section(logical_any(&text, Some(&path), warn), section)
                    .ok_or_else(|| err_at(&l.origin, format!("library '{}' has no section '{section}'", path.display())))?;
                load(lines, file_dir(&path).as_deref(), depth + 1, out, warn)?;
            }
            // A section definition in the deck itself: only used when selected.
            ".lib" if args.len() == 2 => skip = 1,
            // HSPICE encryption markers around library text.
            ".endl" | ".protect" | ".unprotect" | ".prot" | ".unprot" => {}
            _ => out.push(l),
        }
    }
    Ok(())
}

/// Split statements into the top level and subcircuit definitions (registered in `ctx`).
fn collect(lines: Vec<Line>, ctx: &mut Ctx) -> Result<Vec<Line>, SimError> {
    let mut top = vec![];
    // Open definitions: (name, subckt, origin).
    let mut stack: Vec<(String, Subckt, Origin)> = vec![];
    let mut in_control = false;
    for l in lines {
        let c = cmd(&l.text);
        if in_control || c == ".control" {
            in_control = c != ".endc";
            top.push(l);
            continue;
        }
        match c.as_str() {
            ".subckt" | ".macro" => {
                let (name, s) = parse_subckt_header(&l)?;
                stack.push((name, s, l.origin));
            }
            ".ends" | ".eom" => {
                let (name, s, _) = stack.pop().ok_or_else(|| err_at(&l.origin, "'.ends' without '.subckt'"))?;
                ctx.subckts.insert(name, s);
            }
            ".global" => {
                ctx.globals.extend(l.text.split_whitespace().skip(1).map(str::to_ascii_lowercase));
            }
            _ => match stack.last_mut() {
                Some((_, s, _)) => s.body.push(l),
                None => {
                    let end = c == ".end";
                    top.push(l);
                    if end {
                        break;
                    }
                }
            },
        }
    }
    if let Some((name, _, o)) = stack.pop() {
        return Err(err_at(&o, format!("subcircuit '{name}' has no '.ends'")));
    }
    Ok(top)
}

/// Remove whitespace around `=` so that `w = 1u` reads as one `w=1u` token.
fn tight_equals(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '=' {
            while out.ends_with(char::is_whitespace) {
                out.pop();
            }
            out.push('=');
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Split on whitespace outside `{}`, `''` and `""`.
fn split_tokens(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') if depth == 0 => {
                quote = Some(c);
                cur.push(c);
            }
            (None, '{') => {
                depth += 1;
                cur.push(c);
            }
            (None, '}') => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            (None, c) if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `{expr}` / `'expr'` (or a plain expression) as text: braces and quotes stripped.
fn expr_text(v: &str) -> &str {
    let v = v.trim();
    if let Some(i) = v.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
        return i;
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return &v[1..v.len() - 1];
    }
    v
}

fn parse_subckt_header(l: &Line) -> Result<(String, Subckt), SimError> {
    let toks = split_tokens(&tight_equals(&l.text));
    let name = toks.get(1).ok_or_else(|| err_at(&l.origin, "'.subckt' without a name"))?.to_ascii_lowercase();
    let mut s = Subckt::default();
    for t in &toks[2..] {
        if t.eq_ignore_ascii_case("params:") {
            continue;
        }
        match t.split_once('=') {
            Some((k, v)) => s.defaults.push((k.to_ascii_lowercase(), expr_text(v).to_string())),
            None => s.ports.push(t.to_ascii_lowercase()),
        }
    }
    Ok((name, s))
}

/// Evaluate assignments that may refer to each other in any order into `params`.
fn eval_assignments(assign: &[(String, String, Origin)], params: &mut Params) -> Result<(), SimError> {
    let mut pending: Vec<&(String, String, Origin)> = assign.iter().collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = vec![];
        for a in pending {
            match eval(&a.1, params) {
                Ok(v) => {
                    params.insert(a.0.clone(), v);
                }
                Err(ExprError::Unknown(_)) => next.push(a),
                Err(e) => return Err(err_at(&a.2, format!("parameter {}: {e}", a.0))),
            }
        }
        if next.len() == before {
            let a = next[0];
            let e = eval(&a.1, params).unwrap_err();
            return Err(err_at(&a.2, format!("parameter {}: {e}", a.0)));
        }
        pending = next;
    }
    Ok(())
}

/// `.param a=1 b='a*2' c = {b+1}` as (name, expression) pairs.
fn param_assignments(l: &Line) -> Result<Vec<(String, String, Origin)>, SimError> {
    let rest = l.text.split_once(char::is_whitespace).map(|x| x.1).unwrap_or("");
    let toks = split_tokens(&tight_equals(rest));
    let mut out: Vec<(String, String, Origin)> = vec![];
    for t in toks {
        match t.split_once('=') {
            Some((k, v)) if !k.is_empty() && !k.contains(['(', '{', '\'']) => {
                out.push((k.to_ascii_lowercase(), expr_text(v).to_string(), l.origin.clone()))
            }
            // An expression written with spaces (`a=1 + b`) continues the previous value.
            _ => match out.last_mut() {
                Some(last) => {
                    last.1.push(' ');
                    last.1.push_str(expr_text(&t));
                }
                None => return Err(err_at(&l.origin, format!("bad '.param' statement '{}'", l.text))),
            },
        }
    }
    Ok(out)
}

/// Replace `{expr}` and `'expr'` groups in `s` with their values.
fn subst_groups(s: &str, params: &Params, o: &Origin) -> Result<String, SimError> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let next = rest.find(['{', '\'']);
        let Some(i) = next else {
            out.push_str(rest);
            return Ok(out);
        };
        out.push_str(&rest[..i]);
        let open = rest.as_bytes()[i] as char;
        let after = &rest[i + 1..];
        let end = if open == '{' {
            let mut depth = 1usize;
            after.char_indices().find_map(|(k, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(k)
            })
        } else {
            after.find('\'')
        };
        let end = end.ok_or_else(|| err_at(o, format!("unbalanced '{open}' in '{s}'")))?;
        let e = &after[..end];
        let v = eval(e, params).map_err(|err| err_at(o, format!("{err} in '{e}'")))?;
        out.push_str(&fmt_num(v));
        rest = &after[end + 1..];
    }
}

/// A value token: numbers stay, other text is evaluated as an expression when it can be.
fn value_token(t: &str, params: &Params) -> Option<String> {
    if parse_spice(t).is_some_and(|_| t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')) {
        return None;
    }
    eval(t, params).ok().map(fmt_num)
}

/// Rewrite `k=expr` tokens (and the given positional tokens) to numbers.
fn subst_values(toks: &mut [String], positional: impl Fn(usize) -> bool, params: &Params) {
    for (i, t) in toks.iter_mut().enumerate() {
        if let Some((k, v)) = t.split_once('=') {
            if let Some(n) = value_token(v, params) {
                *t = format!("{k}={n}");
            }
        } else if positional(i)
            && let Some(n) = value_token(t, params)
        {
            *t = n;
        }
    }
}

fn kv(toks: &[String]) -> HashMap<String, f64> {
    toks.iter().filter_map(|t| t.split_once('=')).filter_map(|(k, v)| Some((k.to_ascii_lowercase(), parse_spice(v)?))).collect()
}

/// Number of node fields of an element by its first letter.
fn node_count(first: char, toks: &[String]) -> usize {
    match first {
        'r' | 'c' | 'l' | 'v' | 'i' | 'd' | 'f' | 'h' => 2,
        'q' | 'j' => 3,
        'm' | 'e' | 'g' => 4,
        // Subcircuit: every positional token but the last (the subcircuit name).
        'x' => toks[1..].iter().take_while(|t| !t.contains('=') && !t.eq_ignore_ascii_case("params:")).count().saturating_sub(1),
        _ => 0,
    }
}

const SOURCE_KEYWORDS: &[&str] = &["dc", "ac", "pulse", "sin", "pwl", "exp", "sffm", "am"];

/// Flatten `body` into `ctx.out`.
struct Scope<'a> {
    /// Hierarchical prefix (`x1.x2`), empty at the top level.
    prefix: String,
    /// Node of the parent for each port.
    ports: HashMap<String, String>,
    params: Params,
    /// Parameters the instance sets: a `.param` of the subcircuit body does not change them
    /// (as ngspice; the SKY130 devices declare their defaults so).
    overridden: Vec<String>,
    /// Visible model cards by name (bins under their base name).
    models: HashMap<String, Vec<ModelRef>>,
    parent: Option<&'a Scope<'a>>,
}

impl Scope<'_> {
    fn node(&self, n: &str, ctx: &Ctx) -> String {
        let key = n.to_ascii_lowercase();
        if self.prefix.is_empty() || key == "0" || key == "gnd" || ctx.globals.contains(&key) {
            return key;
        }
        if let Some(p) = self.ports.get(&key) {
            return p.clone();
        }
        format!("{}.{key}", self.prefix)
    }

    fn element_name(&self, name: &str) -> String {
        let name = name.to_ascii_lowercase();
        if self.prefix.is_empty() {
            return name;
        }
        let first = &name[..name.chars().next().map_or(0, char::len_utf8)];
        format!("{first}.{}.{name}", self.prefix)
    }

    fn model(&self, name: &str) -> Option<&Vec<ModelRef>> {
        self.models.get(name).or_else(|| self.parent.and_then(|p| p.model(name)))
    }
}

fn expand_body(body: &[Line], scope: &mut Scope, ctx: &mut Ctx, depth: usize) -> Result<(), SimError> {
    // Parameters first: they may be used before their definition.
    let mut assign = vec![];
    for l in body {
        if cmd(&l.text) == ".param" {
            assign.extend(param_assignments(l)?.into_iter().filter(|a| !scope.overridden.contains(&a.0)));
        }
    }
    eval_assignments(&assign, &mut scope.params)?;

    // Model cards, evaluated in this scope.
    for l in body.iter().filter(|l| cmd(&l.text) == ".model") {
        let text = subst_groups(&tight_equals(&l.text), &scope.params, &l.origin)?;
        let text = text.replace(['(', ')'], " ");
        let mut toks = split_tokens(&text);
        if toks.len() < 3 {
            return Err(err_at(&l.origin, format!("bad model card '{}'", l.text)));
        }
        subst_values(&mut toks[3..], |_| false, &scope.params);
        let name = toks[1].to_ascii_lowercase();
        let ty = toks[2].to_ascii_lowercase();
        let p = kv(&toks[3..]);
        let bin = match (name.rsplit_once('.'), p.get("lmin"), p.get("lmax"), p.get("wmin"), p.get("wmax")) {
            (Some((base, n)), Some(&a), Some(&b), Some(&c), Some(&d)) if n.parse::<u32>().is_ok() => {
                Some((base.to_string(), [a, b, c, d]))
            }
            _ => None,
        };
        let card = format!("{ty} {}", toks[3..].join(" "));
        let emit = if scope.prefix.is_empty() {
            ctx.out.push(Line { origin: l.origin.clone(), text: format!(".model {name} {card}") });
            name.clone()
        } else if let Some(e) = ctx.model_cards.get(&card) {
            e.clone()
        } else {
            let emit = format!("{}.{name}", scope.prefix);
            ctx.out.push(Line { origin: l.origin.clone(), text: format!(".model {emit} {card}") });
            ctx.model_cards.insert(card, emit.clone());
            emit
        };
        let (key, bin) = match bin {
            Some((base, b)) => (base, Some(b)),
            None => (name, None),
        };
        let kind = match ty.as_str() {
            "d" => 'd',
            "npn" | "pnp" => 'q',
            "njf" | "pjf" => 'j',
            _ => 'm',
        };
        scope.models.entry(key).or_default().push(ModelRef { emit, kind, bin });
    }

    let mut in_control = false;
    for l in body {
        let c = cmd(&l.text);
        if in_control || c == ".control" {
            in_control = c != ".endc";
            if scope.prefix.is_empty() {
                ctx.out.push(l.clone());
            }
            continue;
        }
        if c == ".param" || c == ".model" {
            continue;
        }
        if c.starts_with('.') {
            if scope.prefix.is_empty() {
                // Analyses take parameter values; other commands (`.meas`, `.print`) may hold
                // expressions of their own and stay as written when they do not evaluate.
                let text = subst_groups(&l.text, &scope.params, &l.origin).unwrap_or_else(|_| l.text.clone());
                ctx.out.push(Line { origin: l.origin.clone(), text });
            } else {
                ctx.warnings.push(format!("{}: '{c}' inside a subcircuit is ignored", l.origin.describe()));
            }
            continue;
        }
        expand_element(l, scope, ctx, depth)?;
    }
    Ok(())
}

/// The element line of a Spectre instance (`@name nodes master params`): an `X` line for a
/// subcircuit master, otherwise the element letter of the master model.
fn resolve_generic(l: &Line, scope: &Scope, ctx: &Ctx) -> Result<String, SimError> {
    let toks = split_tokens(&tight_equals(&l.text[1..]));
    let master = toks.iter().take_while(|t| !t.contains('=')).last().cloned().unwrap_or_default();
    let key = master.to_ascii_lowercase();
    let letter = if ctx.subckts.contains_key(&key) {
        'x'
    } else if let Some(m) = scope.model(&key).and_then(|v| v.first()) {
        m.kind
    } else {
        return Err(err_at(&l.origin, format!("'{}': unknown model or subcircuit '{master}'", toks[0])));
    };
    let name = &toks[0];
    let name = if name.chars().next().is_some_and(|c| c.eq_ignore_ascii_case(&letter)) {
        name.clone()
    } else {
        format!("{letter}{name}")
    };
    Ok(format!("{name} {}", toks[1..].join(" ")))
}

fn expand_element(l: &Line, scope: &mut Scope, ctx: &mut Ctx, depth: usize) -> Result<(), SimError> {
    let resolved;
    let l = if l.text.starts_with('@') {
        resolved = Line { origin: l.origin.clone(), text: resolve_generic(l, scope, ctx)? };
        &resolved
    } else {
        l
    };
    let text = subst_groups(&tight_equals(&l.text), &scope.params, &l.origin)?;
    let first = text.chars().next().unwrap_or(' ').to_ascii_lowercase();
    let text = if matches!(first, 'v' | 'i') { text.replace('(', " ( ").replace(')', " ) ").replace(',', " ") } else { text };
    let mut toks = split_tokens(&text);
    let nodes = node_count(first, &toks).min(toks.len().saturating_sub(1));

    if first == 'x' {
        return expand_instance(l, toks, nodes, scope, ctx, depth);
    }

    // Values: the value field of two-terminal passives, source values, `k=v` parameters.
    let params = &scope.params;
    match first {
        'r' | 'c' | 'l' => subst_values(&mut toks, |i| i == 3, params),
        'v' | 'i' => {
            let kw: Vec<bool> = toks.iter().map(|t| SOURCE_KEYWORDS.contains(&t.to_ascii_lowercase().as_str())).collect();
            subst_values(&mut toks, |i| i > nodes && !kw[i], params)
        }
        _ => subst_values(&mut toks, |_| false, params),
    }
    for i in 1..=nodes {
        toks[i] = scope.node(&toks[i], ctx);
    }
    // Model reference (diodes, MOS, BJTs, JFETs) with binning.
    let model_at = match first {
        'd' | 'q' | 'j' | 'm' => Some(nodes + 1),
        _ => None,
    };
    if let Some(k) = model_at.filter(|&k| k < toks.len()) {
        let name = toks[k].to_ascii_lowercase();
        if let Some(refs) = scope.model(&name) {
            let p = kv(&toks[k + 1..]);
            toks[k] = pick_model(refs, &p, &name, ctx.scale).map_err(|m| err_at(&l.origin, format!("{}: {m}", toks[0])))?;
        }
    }
    toks[0] = scope.element_name(&toks[0]);
    ctx.out.push(Line { origin: l.origin.clone(), text: toks.join(" ") });
    Ok(())
}

/// The `.option scale` value of `lines` (the last one), if any.
fn option_scale(lines: &[Line]) -> Option<f64> {
    let mut out = None;
    for l in lines {
        let c = cmd(&l.text);
        if c == ".option" || c == ".options" || c == ".opt" {
            let toks = split_tokens(&tight_equals(&l.text));
            for t in &toks[1..] {
                if let Some((k, v)) = t.split_once('=')
                    && k.eq_ignore_ascii_case("scale")
                    && let Some(v) = parse_spice(v).filter(|v| *v > 0.0)
                {
                    out = Some(v);
                }
            }
        }
    }
    out
}

/// The model card for an instance with parameters `p` (lengths in units of `scale` metres):
/// the plain card, or the bin that holds its `l` and `w`.
fn pick_model(refs: &[ModelRef], p: &HashMap<String, f64>, name: &str, scale: f64) -> Result<String, String> {
    if let Some(m) = refs.iter().find(|m| m.bin.is_none()) {
        return Ok(m.emit.clone());
    }
    let l = p.get("l").map_or(1e-6, |l| l * scale);
    let w = p.get("w").map_or(1e-6, |w| w * scale) / p.get("nf").copied().unwrap_or(1.0).max(1.0);
    // Bin limits are inclusive below and exclusive above, with a small tolerance for rounding.
    let inside = |v: f64, lo: f64, hi: f64| v >= lo * (1.0 - 1e-9) && v < hi * (1.0 - 1e-9);
    refs.iter()
        .find(|m| m.bin.is_some_and(|[l0, l1, w0, w1]| inside(l, l0, l1) && inside(w, w0, w1)))
        .map(|m| m.emit.clone())
        .ok_or_else(|| format!("no bin of model '{name}' covers l={} w={}", fmt_num(l), fmt_num(w)))
}

fn expand_instance(
    l: &Line,
    mut toks: Vec<String>,
    nodes: usize,
    scope: &mut Scope,
    ctx: &mut Ctx,
    depth: usize,
) -> Result<(), SimError> {
    if depth >= MAX_SUBCKT_DEPTH {
        return Err(err_at(&l.origin, "subcircuits nested too deeply (recursive subcircuit?)"));
    }
    let name_at = nodes + 1;
    let sub_name = toks.get(name_at).ok_or_else(|| err_at(&l.origin, format!("'{}' has no subcircuit name", toks[0])))?;
    let sub_name = sub_name.to_ascii_lowercase();
    let Some(sub) = ctx.subckts.get(&sub_name).cloned() else {
        return Err(err_at(&l.origin, format!("unknown subcircuit '{sub_name}'")));
    };
    if sub.ports.len() != nodes {
        return Err(err_at(
            &l.origin,
            format!("'{}' connects {nodes} nodes, subcircuit '{sub_name}' has {} ports", toks[0], sub.ports.len()),
        ));
    }
    let ports: HashMap<String, String> =
        sub.ports.iter().zip(&toks[1..=nodes]).map(|(p, n)| (p.clone(), scope.node(n, ctx))).collect();

    // Instance parameters, evaluated in the caller's scope.
    let mut overrides = Params::new();
    let inst = toks[0].clone();
    for t in toks.drain(name_at + 1..) {
        if t.eq_ignore_ascii_case("params:") {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            return Err(err_at(&l.origin, format!("'{}': unexpected '{t}' after the subcircuit name", inst)));
        };
        let v = eval(expr_text(v), &scope.params).map_err(|e| err_at(&l.origin, format!("{inst}: {k}: {e}")))?;
        let k = k.to_ascii_lowercase();
        let in_body = || {
            sub.body
                .iter()
                .filter(|b| cmd(&b.text) == ".param")
                .any(|b| param_assignments(b).is_ok_and(|a| a.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(&k))))
        };
        if !sub.defaults.iter().any(|(d, _)| *d == k) && !in_body() {
            ctx.warnings.push(format!("{}: subcircuit '{sub_name}' has no parameter '{k}'", l.origin.describe()));
        }
        overrides.insert(k, v);
    }

    // The subcircuit sees the global parameters, its defaults and the overrides.
    let mut params = top_params(scope).clone();
    params.extend(overrides.iter().map(|(k, v)| (k.clone(), *v)));
    let defaults: Vec<(String, String, Origin)> = sub
        .defaults
        .iter()
        .filter(|(k, _)| !overrides.contains_key(k))
        .map(|(k, v)| (k.clone(), v.clone(), l.origin.clone()))
        .collect();
    eval_assignments(&defaults, &mut params)?;

    let iname = inst.to_ascii_lowercase();
    let prefix = if scope.prefix.is_empty() { iname } else { format!("{}.{iname}", scope.prefix) };
    let overridden = overrides.keys().cloned().collect();
    let mut child = Scope { prefix, ports, params, overridden, models: HashMap::new(), parent: Some(scope) };
    expand_body(&sub.body, &mut child, ctx, depth + 1)
}

fn top_params<'a>(s: &'a Scope<'a>) -> &'a Params {
    match s.parent {
        Some(p) => top_params(p),
        None => &s.params,
    }
}

/// Whether `deck` uses anything the front-end resolves (so that a plain deck can skip it).
pub fn needs_expansion(deck: &str) -> bool {
    ics_core::spectre::is_spectre(deck, None)
        || deck.lines().skip(1).any(|l| {
            let t = l.trim_start().to_ascii_lowercase();
            let c = t.split_whitespace().next().unwrap_or("");
            matches!(c, ".param" | ".subckt" | ".macro" | ".include" | ".inc" | ".lib" | ".global")
                || t.contains('{')
                || (!t.starts_with('*') && t.contains('\''))
                || c.starts_with('x')
        })
}

/// Flatten `deck`. Relative `.include` and `.lib` paths are taken from `dir` (the deck's
/// folder), or the current folder without one.
pub fn expand(deck: &str, dir: Option<&Path>) -> Result<Expanded, SimError> {
    // The title is repeated as a comment; a first line that is not a comment is also read as a
    // statement, as the built-in engine always did.
    let title = deck.lines().next().unwrap_or("");
    let mut loaded = vec![];
    let mut warnings = vec![];
    let lines = logical_any(deck, None, &mut warnings);
    load(lines, dir, 0, &mut loaded, &mut warnings)?;
    let mut ctx = Ctx { warnings, scale: 1.0, ..Ctx::default() };
    let top = collect(loaded, &mut ctx)?;
    // `.option scale` anywhere (a model library sets it after the instances that need it).
    ctx.scale = option_scale(&top).unwrap_or(1.0);
    let mut scope = Scope {
        prefix: String::new(),
        ports: HashMap::new(),
        params: Params::new(),
        overridden: Vec::new(),
        models: HashMap::new(),
        parent: None,
    };
    expand_body(&top, &mut scope, &mut ctx, 0)?;

    let title_text = title.trim_start_matches('*').trim();
    let mut text = format!("* {title_text}\n");
    let mut origins = vec![Origin { file: None, line: 1 }];
    for l in ctx.out {
        text.push_str(&l.text);
        text.push('\n');
        origins.push(l.origin);
    }
    let mut seen = HashSet::new();
    ctx.warnings.retain(|w| seen.insert(w.clone()));
    Ok(Expanded { deck: text, origins, warnings: ctx.warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Element, parse_deck};

    fn flat(deck: &str) -> Expanded {
        expand(deck, None).unwrap()
    }

    fn lines(e: &Expanded) -> Vec<&str> {
        e.deck.lines().skip(1).collect()
    }

    #[test]
    fn bins_take_the_option_scale() {
        // Lengths in µm with `.option scale=1u` (as the SKY130 libraries set, after the
        // instances): the bin of 0.4 µm is chosen.
        let deck = "* t\nM1 d g 0 0 nm w=1 l=0.4\nV1 d 0 1\nV2 g 0 1\n\
                    .model nm.0 nmos (level=1 lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-5 vto=0.4)\n\
                    .model nm.1 nmos (level=1 lmin=1e-6 lmax=1e-5 wmin=1e-7 wmax=1e-5 vto=0.6)\n.option scale=1u\n.end\n";
        let ex = expand(deck, None).unwrap();
        assert!(ex.deck.contains("vto=0.4"), "{}", ex.deck);
    }

    #[test]
    fn instance_parameters_win_over_a_subcircuit_param() {
        // The SKY130 devices declare their defaults with `.param` in the body; the instance's
        // values still apply (as in ngspice), without a warning.
        let deck =
            "* t\n.subckt dev a b\n.param r = 1k\nR1 a b {r}\n.ends\nX1 n1 0 dev r=2k\nX2 n2 0 dev\nV1 n1 0 1\nV2 n2 0 1\n.end\n";
        let ex = expand(deck, None).unwrap();
        assert!(ex.deck.to_ascii_lowercase().contains("r.x1.r1 n1 0 2000"), "{}", ex.deck);
        assert!(ex.deck.to_ascii_lowercase().contains("r.x2.r1 n2 0 1000"), "{}", ex.deck);
        assert!(ex.warnings.is_empty(), "{:?}", ex.warnings);
    }

    #[test]
    fn params_and_expressions() {
        let e = flat(
            "* t\n.param vdd=1.8 r0 = 1k\n.param half='vdd/2' rr={r0*2}\nV1 a 0 dc {vdd}\nV2 b 0 half\n\
             R1 a b rr\nC1 b 0 {1p*2}\n.tran {1n} 10n\n.end\n",
        );
        assert_eq!(lines(&e), ["v1 a 0 dc 1.8", "v2 b 0 0.9", "r1 a b 2000", "c1 b 0 2e-12", ".tran 1e-9 10n", ".end"]);
    }

    #[test]
    fn forward_references_and_errors() {
        let e = flat("* t\n.param b=a*2\n.param a=3\nR1 x 0 {b}\n.end\n");
        assert_eq!(lines(&e)[0], "r1 x 0 6");
        let err = expand("* t\n.param b=c\nR1 x 0 {b}\n.end\n", None).unwrap_err().to_string();
        assert!(err.contains("line 2") && err.contains("unknown parameter 'c'"), "{err}");
        let err = expand("* t\nR1 x 0 {zz}\n.end\n", None).unwrap_err().to_string();
        assert!(err.contains("line 2") && err.contains("zz"), "{err}");
    }

    #[test]
    fn subcircuits_flatten_with_parameters() {
        let deck = "* inv chain\n.param wp0=2u\n\
            .subckt inv a y vdd vss wn=1u wp=wp0 l=0.18u\n\
            M1 y a vss vss nch w={wn} l=l\nM2 y a vdd vdd pch w=wp l=l\nC1 y vss 1f\n.ends inv\n\
            .subckt buf in out vdd params: s=2\nX1 in mid vdd 0 inv\nX2 mid out vdd 0 inv wn={s*1u} wp='s*2u'\n.ends\n\
            .model nch nmos vto=0.4\n.model pch pmos vto=-0.4\n\
            Xb a b vdd buf s=3\nV1 vdd 0 1.8\n.op\n.end\n";
        let e = flat(deck);
        let l = lines(&e);
        assert!(l.contains(&"m.xb.x1.m1 xb.mid a 0 0 nch w=1e-6 l=1.8e-7"), "{l:#?}");
        assert!(l.contains(&"m.xb.x1.m2 xb.mid a vdd vdd pch w=2e-6 l=1.8e-7"), "{l:#?}");
        assert!(l.contains(&"m.xb.x2.m1 b xb.mid 0 0 nch w=3e-6 l=1.8e-7"), "{l:#?}");
        assert!(l.contains(&"m.xb.x2.m2 b xb.mid vdd vdd pch w=6e-6 l=1.8e-7"), "{l:#?}");
        assert!(l.contains(&"c.xb.x2.c1 b 0 1f"), "{l:#?}");
        let c = parse_deck(&e.deck).unwrap();
        assert_eq!(c.elements.iter().filter(|e| matches!(e, Element::M { .. })).count(), 4);
    }

    #[test]
    fn subcircuit_errors() {
        let err = expand("* t\nX1 a b nosuch\n.end\n", None).unwrap_err().to_string();
        assert!(err.contains("unknown subcircuit 'nosuch'"), "{err}");
        let err = expand("* t\n.subckt r2 a b\nR1 a b 1\n.ends\nX1 a r2\n.end\n", None).unwrap_err().to_string();
        assert!(err.contains("connects 1 nodes") && err.contains("2 ports"), "{err}");
        let err = expand("* t\n.subckt loop a\nX1 a loop\n.ends\nX1 a loop\n.end\n", None).unwrap_err().to_string();
        assert!(err.contains("recursive"), "{err}");
        let err = expand("* t\n.subckt open a\nR1 a 0 1\n.end\n", None).unwrap_err().to_string();
        assert!(err.contains("no '.ends'"), "{err}");
    }

    #[test]
    fn global_nodes_stay_global() {
        let e = flat("* t\n.global vdd!\n.subckt ld a\nR1 a vdd! 1k\n.ends\nX1 n ld\n.end\n");
        assert_eq!(lines(&e)[0], "r.x1.r1 n vdd! 1k");
    }

    #[test]
    fn model_binning_and_models_inside_subcircuits() {
        // Like a PDK: a wrapper subcircuit with binned model cards that use its parameters.
        let deck = "* pdk\n.param mc_vt=0.01\n\
            .subckt nfet d g s b l=0.15u w=1u mult=1 dvt=0\n\
            .model nmodel.0 nmos lmin=0.15u lmax=0.5u wmin=0.42u wmax=7u vto={0.45+mc_vt+dvt}\n\
            .model nmodel.1 nmos lmin=0.5u lmax=20u wmin=0.42u wmax=7u vto={0.40+mc_vt+dvt}\n\
            M0 d g s b nmodel l={l} w={w} m={mult}\n.ends\n\
            X1 d1 g 0 0 nfet l=0.15u w=1u\nX2 d2 g 0 0 nfet l=1u w=1u\nX3 d3 g 0 0 nfet l=1u w=2u\n\
            X4 d4 g 0 0 nfet l=1u w=2u dvt=0.05\n.end\n";
        let e = flat(deck);
        let l = lines(&e);
        let card = |name: &str| l.iter().find(|x| x.starts_with(&format!(".model {name} "))).copied();
        // X1 uses bin 0, X2 and X3 bin 1 (shared card), X4 its own card with a shifted vto.
        assert!(card("x1.nmodel.0").unwrap().contains("vto=0.46"), "{l:#?}");
        assert!(card("x1.nmodel.1").unwrap().contains("vto=0.41"), "{l:#?}");
        assert!(l.contains(&"m.x1.m0 d1 g 0 0 x1.nmodel.0 l=1.5e-7 w=1e-6 m=1"), "{l:#?}");
        assert!(l.contains(&"m.x2.m0 d2 g 0 0 x1.nmodel.1 l=1e-6 w=1e-6 m=1"), "{l:#?}");
        assert!(l.contains(&"m.x3.m0 d3 g 0 0 x1.nmodel.1 l=1e-6 w=2e-6 m=1"), "{l:#?}");
        assert!(card("x4.nmodel.1").unwrap().contains("vto=0.46"), "{l:#?}");
        assert!(l.iter().any(|x| x.starts_with("m.x4.m0 d4 g 0 0 x4.nmodel.1 ")), "{l:#?}");
        let c = parse_deck(&e.deck).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        let err = expand(&deck.replace("w=1u\nX2", "w=10u\nX2"), None).unwrap_err().to_string();
        assert!(err.contains("no bin of model 'nmodel' covers l=1.5e-7 w=1e-5"), "{err}");
    }

    #[test]
    fn include_and_library_sections() {
        let dir = std::env::temp_dir().join(format!("ics-frontend-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(
            dir.join("models/corners.lib"),
            "* corners\n.lib tt\n.param dvt=0\n.include 'core.spi'\n.endl tt\n\
             .lib ff\n.param dvt=-0.05\n.include \"core.spi\"\n.endl ff\n",
        )
        .unwrap();
        std::fs::write(dir.join("models/core.spi"), "* core\n.model nch nmos vto={0.5+dvt}\n+ kp=200u\n").unwrap();
        let deck = "* t\n.lib models/corners.lib ff\nM1 d g 0 0 nch w=1u l=1u\n.end\n";
        let e = expand(deck, Some(&dir)).unwrap();
        assert_eq!(lines(&e)[0], ".model nch nmos vto=0.45 kp=200u");
        assert_eq!(e.origins[1], Origin { file: Some(dir.join("models").join("core.spi")), line: 2 });
        let e = expand(&deck.replace(" ff", " tt"), Some(&dir)).unwrap();
        assert_eq!(lines(&e)[0], ".model nch nmos vto=0.5 kp=200u");
        let err = expand(&deck.replace(" ff", " ss"), Some(&dir)).unwrap_err().to_string();
        assert!(err.contains("no section 'ss'"), "{err}");
        let err = expand("* t\n.include nope.spi\n.end\n", Some(&dir)).unwrap_err().to_string();
        assert!(err.contains("line 2") && err.contains("cannot read"), "{err}");
        // Errors inside an included file name the file.
        std::fs::write(dir.join("bad.spi"), "* bad\nR1 a 0 {nope}\n").unwrap();
        let err = expand("* t\n.inc bad.spi\n.end\n", Some(&dir)).unwrap_err().to_string();
        assert!(err.contains("bad.spi:2"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn spectre_decks_and_libraries() {
        let dir = std::env::temp_dir().join(format!("ics-spectre-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("lib.scs"),
            "// corners\nsimulator lang=spectre\nlibrary l\nsection tt\nparameters dvt=0\ninclude \"dev.scs\"\nendsection tt\n\
             section ff\nparameters dvt=-0.1\ninclude \"dev.scs\"\nendsection ff\nendlibrary l\n",
        )
        .unwrap();
        std::fs::write(dir.join("dev.scs"), "model nch mos1 type=n vto=0.5+dvt kp=200u\nmodel dd diode is=1e-14\n").unwrap();
        let deck = "// t\nsimulator lang=spectre\ninclude \"lib.scs\" section=ff\n\
            subckt cell (a b)\nparameters w=1u\nM1 (a a b b) nch w=w l=1u\nD1 (b a) dd\nends cell\n\
            V1 (x 0) vsource dc=1.2\nX1 (x 0) cell w=2u\nop1 dc\n";
        let e = expand(deck, Some(&dir)).unwrap();
        let l = lines(&e);
        assert!(l.contains(&".model nch nmos level=1 vto=0.4 kp=200u"), "{l:#?}");
        assert!(l.contains(&"m.x1.m1 x x 0 0 nch w=2e-6 l=1u"), "{l:#?}");
        assert!(l.contains(&"d.x1.d1 0 x dd"), "{l:#?}");
        assert!(l.contains(&".op"), "{l:#?}");
        let c = parse_deck(&e.deck).unwrap();
        assert_eq!(c.elements.len(), 3);
        let err = expand(&deck.replace("cell w=2u", "nosuch w=2u"), Some(&dir)).unwrap_err().to_string();
        assert!(err.contains("unknown model or subcircuit 'nosuch'"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn control_blocks_and_comments_pass_through() {
        let e = flat("* t\nR1 a 0 1k $ load\nV1 a 0 1 ; src\n.control\nlet x = {1}\n.endc\n.op\n.end\n.tran 1 2\n");
        assert_eq!(lines(&e), ["r1 a 0 1k", "v1 a 0 1", ".control", "let x = {1}", ".endc", ".op", ".end"]);
    }

    #[test]
    fn plain_decks_need_no_expansion() {
        assert!(!needs_expansion("* t\nV1 a 0 1\nR1 a 0 1k\n.op\n.end\n"));
        assert!(needs_expansion("* t\n.param r=1k\nR1 a 0 {r}\n.end\n"));
        assert!(needs_expansion("* t\nX1 a b inv\n.end\n"));
    }
}
