//! Spectre netlists.
//!
//! Reading: [`to_spice`] turns Spectre-language text (decks and PDK model libraries, which switch
//! between languages with `simulator lang=spice|spectre`) into SPICE lines the SPICE front-end
//! reads. Instances whose master is a model or a subcircuit come out as `@name nodes master
//! params`; the front-end decides the element type once all models and subcircuits are known.
//!
//! Writing: [`from_spice`] writes a flat SPICE deck as produced by the netlister (R C L V I D M
//! X elements, models, libraries, analyses) in the Spectre language.

use std::collections::BTreeMap;
use std::path::Path;

/// A SPICE line from the source line it starts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Translated {
    pub lines: Vec<Line>,
    pub warnings: Vec<String>,
}

/// Whether `text` (from `path`) is, or switches to, the Spectre language.
pub fn is_spectre(text: &str, path: Option<&Path>) -> bool {
    if path.and_then(|p| p.extension()).is_some_and(|e| e.eq_ignore_ascii_case("scs")) {
        return true;
    }
    text.lines().any(|l| {
        let t = l.trim().to_ascii_lowercase();
        t.starts_with("simulator") && t.replace(' ', "").contains("lang=spectre")
    })
}

/// Whether the text starts in the Spectre language (a `.scs` file).
pub fn starts_in_spectre(path: Option<&Path>) -> bool {
    path.and_then(|p| p.extension()).is_some_and(|e| e.eq_ignore_ascii_case("scs"))
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    /// A parenthesised node list.
    Nodes(Vec<String>),
}

/// Split a Spectre statement into words and node lists; brackets, parentheses and quotes inside
/// a word (`w=(a+b)`, `wave=[0 0 1n 1]`) stay in it.
fn lex(s: &str) -> Vec<Tok> {
    let s = tight_equals(s);
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if quote {
            cur.push(c);
            if c == '"' {
                quote = false;
            }
            continue;
        }
        match c {
            '"' => {
                quote = true;
                cur.push(c);
            }
            '(' if cur.is_empty() && depth == 0 => {
                let mut inner = String::new();
                let mut d = 1;
                for c in chars.by_ref() {
                    match c {
                        '(' => d += 1,
                        ')' => d -= 1,
                        _ => {}
                    }
                    if d == 0 {
                        break;
                    }
                    inner.push(c);
                }
                out.push(Tok::Nodes(inner.split_whitespace().map(str::to_string).collect()));
            }
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            c if c.is_whitespace() && depth <= 0 => {
                if !cur.is_empty() {
                    out.push(Tok::Word(std::mem::take(&mut cur)));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(Tok::Word(cur));
    }
    out
}

fn tight_equals(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '=' && chars.peek() != Some(&'=') && !out.ends_with(['=', '!', '<', '>']) {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('=');
            while chars.peek() == Some(&' ') {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Spectre scale factors in SPICE spelling: `M` (mega) becomes `meg`, `a` (atto) `e-18`,
/// `c` / `%` (centi) `e-2`. Only numbers are touched, not names.
pub fn convert_numbers(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 8);
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let prev_ident = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.');
        let starts_number = (c.is_ascii_digit() || (c == '.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))) && !prev_ident;
        if !starts_number {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
        }
        if i < b.len() && (b[i] | 0x20) == b'e' {
            let sign = matches!(b.get(i + 1), Some(b'+' | b'-'));
            if b.get(i + 1 + sign as usize).is_some_and(u8::is_ascii_digit) {
                i += 1 + sign as usize;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }
        out.push_str(&s[start..i]);
        let next_alpha = |k: usize| b.get(k).is_some_and(|c| c.is_ascii_alphabetic());
        match b.get(i) {
            Some(b'M') if !next_alpha(i + 1) => {
                out.push_str("meg");
                i += 1;
            }
            Some(b'a') if !next_alpha(i + 1) => {
                out.push_str("e-18");
                i += 1;
            }
            Some(b'c') if !next_alpha(i + 1) => {
                out.push_str("e-2");
                i += 1;
            }
            Some(b'%') => {
                out.push_str("e-2");
                i += 1;
            }
            _ => {}
        }
    }
    out
}

/// Split `k=v` words into a map (lower-case keys) and the other words.
fn kv_words(words: &[String]) -> (BTreeMap<String, String>, Vec<String>) {
    let mut kv = BTreeMap::new();
    let mut rest = vec![];
    for w in words {
        match w.split_once('=') {
            Some((k, v)) if !k.is_empty() => {
                kv.insert(k.to_ascii_lowercase(), v.to_string());
            }
            _ => rest.push(w.clone()),
        }
    }
    (kv, rest)
}

fn words(toks: &[Tok]) -> Vec<String> {
    toks.iter()
        .flat_map(|t| match t {
            Tok::Word(w) => vec![w.clone()],
            Tok::Nodes(n) => n.clone(),
        })
        .collect()
}

fn unquote(s: &str) -> &str {
    s.trim_matches('"')
}

/// SPICE element name with the right type letter.
fn element(prefix: char, name: &str) -> String {
    if name.chars().next().is_some_and(|c| c.eq_ignore_ascii_case(&prefix)) {
        name.to_string()
    } else {
        format!("{prefix}{name}")
    }
}

const ANALYSES: &[&str] = &[
    "dc",
    "ac",
    "tran",
    "noise",
    "xf",
    "sp",
    "stb",
    "pss",
    "pac",
    "pnoise",
    "hb",
    "montecarlo",
    "sweep",
    "info",
    "alter",
    "altergroup",
    "set",
];

struct Reader {
    out: Vec<Line>,
    warnings: Vec<String>,
    /// Index in `out` of the open `.subckt` headers.
    subckt: Vec<usize>,
}

impl Reader {
    fn push(&mut self, line: usize, text: String) {
        self.out.push(Line { line, text });
    }

    fn warn(&mut self, line: usize, msg: String) {
        self.warnings.push(format!("line {line}: {msg}"));
    }

    fn statement(&mut self, line: usize, stmt: &str) {
        let toks = lex(stmt);
        let Some(Tok::Word(first)) = toks.first() else {
            self.warn(line, format!("cannot read '{stmt}'"));
            return;
        };
        let kw = first.to_ascii_lowercase();
        let rest = &toks[1..];
        let ws = words(rest);
        match kw.as_str() {
            "include" | "ahdl_include" => {
                if kw == "ahdl_include" {
                    self.warn(line, "Verilog-A (ahdl_include) is not read; use ngspice with OSDI".into());
                    return;
                }
                let (kv, files) = kv_words(&ws);
                let Some(file) = files.first() else { return self.warn(line, "include without a file".into()) };
                match kv.get("section") {
                    Some(sec) => self.push(line, format!(".lib \"{}\" {}", unquote(file), unquote(sec))),
                    None => self.push(line, format!(".include \"{}\"", unquote(file))),
                }
            }
            "library" | "endlibrary" => {}
            "section" => self.push(line, format!(".lib {}", ws.first().map_or("", |s| s))),
            "endsection" => self.push(line, ".endl".into()),
            "parameters" => {
                let text = convert_numbers(&ws.join(" "));
                match self.subckt.last() {
                    // Parameters of a subcircuit are its overridable defaults.
                    Some(&h) => {
                        self.out[h].text.push(' ');
                        self.out[h].text.push_str(&text);
                    }
                    None => self.push(line, format!(".param {text}")),
                }
            }
            "inline" | "subckt" => {
                let rest = if kw == "inline" { &toks[2.min(toks.len())..] } else { rest };
                let ws = words(rest);
                let Some(name) = ws.first() else { return self.warn(line, "subckt without a name".into()) };
                let ports = ws[1..].join(" ");
                self.subckt.push(self.out.len());
                self.push(line, format!(".subckt {name} {ports}").trim_end().to_string());
            }
            "ends" | "endsubckt" => {
                self.subckt.pop();
                self.push(line, ".ends".into());
            }
            "model" => self.model(line, &ws),
            "global" => self.push(line, format!(".global {}", ws.join(" "))),
            "ic" | "nodeset" => {
                let (kv, _) = kv_words(&ws);
                let ics: Vec<String> = kv.iter().map(|(k, v)| format!("v({k})={}", convert_numbers(v))).collect();
                if !ics.is_empty() {
                    self.push(line, format!(".{kw} {}", ics.join(" ")));
                }
            }
            "save" | "simulator_options" => {}
            "real" | "if" | "else" | "statistics" | "process" | "mismatch" | "vary" | "correlate" | "truncate" => {
                self.warn(line, format!("Spectre '{kw}' blocks are not read"))
            }
            _ => self.named(line, first, rest),
        }
    }

    fn model(&mut self, line: usize, ws: &[String]) {
        let [name, ty, params @ ..] = ws else { return self.warn(line, "model needs a name and a type".into()) };
        let (kv, _) = kv_words(params);
        let ty = ty.to_ascii_lowercase();
        let polarity = kv.get("type").map(|t| unquote(t).to_ascii_lowercase());
        let p_type = polarity.as_deref().is_some_and(|t| t.starts_with('p'));
        let mos = |level: &str| format!("{} level={level}", if p_type { "pmos" } else { "nmos" });
        let head = match ty.as_str() {
            "bsim4" => mos("54"),
            "bsim3v3" | "bsim3" => mos("49"),
            "bsimcmg" => mos("17"),
            "mos1" => mos("1"),
            "mos2" => mos("2"),
            "mos3" => mos("3"),
            "diode" => "d".into(),
            "bjt" | "vbic" | "hicum" => {
                if polarity.as_deref() == Some("pnp") {
                    "pnp".into()
                } else {
                    "npn".into()
                }
            }
            other => {
                self.warn(line, format!("model {name}: Spectre model type '{other}' is not read"));
                return;
            }
        };
        let params: Vec<String> =
            params.iter().filter(|w| !w.to_ascii_lowercase().starts_with("type=")).map(|w| convert_numbers(w)).collect();
        self.push(line, format!(".model {name} {head} {}", params.join(" ")).trim_end().to_string());
    }

    /// `name (nodes) master params`, `name master params` (analyses, options) or
    /// `name n1 n2 master params`.
    fn named(&mut self, line: usize, name: &str, rest: &[Tok]) {
        let (nodes, after): (Vec<String>, &[Tok]) = match rest.first() {
            Some(Tok::Nodes(n)) => (n.clone(), &rest[1..]),
            _ => (vec![], rest),
        };
        let ws = words(after);
        let (kv, plain) = kv_words(&ws);
        // Without parentheses the master is the last word that is not a parameter.
        let (nodes, master) =
            if nodes.is_empty() && !plain.is_empty() && !ANALYSES.contains(&plain[0].to_ascii_lowercase().as_str()) {
                let n = plain.len();
                if plain[0].eq_ignore_ascii_case("options") {
                    (vec![], plain[0].clone())
                } else {
                    (plain[..n - 1].to_vec(), plain[n - 1].clone())
                }
            } else {
                match plain.first() {
                    Some(m) => (nodes, m.clone()),
                    None => return self.warn(line, format!("'{name}' has no master")),
                }
            };
        let m = master.to_ascii_lowercase();
        let num = |k: &str| kv.get(k).map(|v| convert_numbers(v));
        let params = || ws.iter().filter(|w| w.contains('=')).map(|w| convert_numbers(w)).collect::<Vec<_>>().join(" ");
        match m.as_str() {
            "options" => {
                let opts: Vec<String> = kv.iter().map(|(k, v)| format!("{k}={}", convert_numbers(v))).collect();
                if !opts.is_empty() {
                    self.push(line, format!(".option {}", opts.join(" ")));
                }
            }
            "dc" => match kv.get("dev") {
                Some(dev) => {
                    let (start, stop) = (num("start").unwrap_or("0".into()), num("stop").unwrap_or("0".into()));
                    let step = num("step").unwrap_or_else(|| match (num("lin"), kv.get("start"), kv.get("stop")) {
                        (Some(n), _, _) => format!("(({stop})-({start}))/{n}"),
                        _ => format!("(({stop})-({start}))/50"),
                    });
                    self.push(line, format!(".dc {dev} {start} {stop} {{{step}}}"));
                }
                None => self.push(line, ".op".into()),
            },
            "ac" => {
                let dec = num("dec").unwrap_or_else(|| "10".into());
                let (start, stop) = (num("start").unwrap_or("1".into()), num("stop").unwrap_or("1g".into()));
                self.push(line, format!(".ac dec {dec} {start} {stop}"));
            }
            "tran" => {
                let Some(stop) = num("stop") else { return self.warn(line, format!("'{name}': tran without stop")) };
                let step = num("step").or_else(|| num("maxstep")).unwrap_or_else(|| format!("{{({stop})/1000}}"));
                self.push(line, format!(".tran {step} {stop}"));
            }
            "noise" => {
                let out = match nodes.as_slice() {
                    [p] => format!("v({p})"),
                    [p, n] if n == "0" => format!("v({p})"),
                    [p, n] => format!("v({p},{n})"),
                    _ => return self.warn(line, format!("'{name}': noise needs an output node")),
                };
                let src = kv.get("iprobe").cloned().unwrap_or_default();
                let dec = num("dec").unwrap_or_else(|| "10".into());
                let (start, stop) = (num("start").unwrap_or("1".into()), num("stop").unwrap_or("1g".into()));
                self.push(line, format!(".noise {out} {src} dec {dec} {start} {stop}"));
            }
            a if ANALYSES.contains(&a) => self.warn(line, format!("Spectre analysis '{a}' is not read")),
            "resistor" | "capacitor" | "inductor" => {
                let (prefix, key) = match m.as_str() {
                    "resistor" => ('R', "r"),
                    "capacitor" => ('C', "c"),
                    _ => ('L', "l"),
                };
                let Some(v) = num(key) else { return self.warn(line, format!("'{name}': {m} without {key}=")) };
                let v = if v.parse::<f64>().is_ok() || ics_units_ok(&v) { v } else { format!("{{{v}}}") };
                self.push(line, format!("{} {} {v}", element(prefix, name), nodes.join(" ")));
            }
            "vsource" | "isource" => {
                let prefix = if m == "vsource" { 'V' } else { 'I' };
                let spec = source_spec(&kv);
                self.push(line, format!("{} {} {spec}", element(prefix, name), nodes.join(" ")));
            }
            _ => {
                let p = params();
                self.push(line, format!("@{name} {} {master} {p}", nodes.join(" ")).trim_end().to_string());
            }
        }
    }
}

/// A value SPICE reads as written (number with an optional scale suffix).
fn ics_units_ok(v: &str) -> bool {
    crate::units::parse_spice(v).is_some() && v.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.' || c == '-')
}

/// SPICE source specification from Spectre `vsource` / `isource` parameters.
fn source_spec(kv: &BTreeMap<String, String>) -> String {
    let g = |k: &str| kv.get(k).map(|v| convert_numbers(v));
    let z = || "0".to_string();
    let ty = kv.get("type").map(|t| t.to_ascii_lowercase()).unwrap_or_else(|| "dc".into());
    let mut parts = vec![];
    let wave = match ty.as_str() {
        "pulse" => Some(format!(
            "PULSE({} {} {} {} {} {} {})",
            g("val0").unwrap_or_else(z),
            g("val1").unwrap_or_else(z),
            g("delay").unwrap_or_else(z),
            g("rise").unwrap_or_else(z),
            g("fall").unwrap_or_else(z),
            g("width").unwrap_or_else(|| "1".into()),
            g("period").unwrap_or_else(z)
        )),
        "sine" => Some(format!(
            "SIN({} {} {} {} {})",
            g("sinedc").unwrap_or_else(z),
            g("ampl").unwrap_or_else(z),
            g("freq").unwrap_or_else(z),
            g("delay").unwrap_or_else(z),
            g("damp").unwrap_or_else(z)
        )),
        "pwl" => {
            g("wave").map(|w| format!("PWL({})", w.trim_matches(['[', ']']).split_whitespace().collect::<Vec<_>>().join(" ")))
        }
        _ => None,
    };
    let dc = g("dc").or_else(|| match ty.as_str() {
        "pulse" => g("val0"),
        "sine" => g("sinedc"),
        "pwl" => g("wave").and_then(|w| w.trim_matches(['[', ']']).split_whitespace().nth(1).map(str::to_string)),
        _ => None,
    });
    parts.push(format!("DC {}", dc.unwrap_or_else(z)));
    if let Some(mag) = g("mag") {
        parts.push(format!("AC {mag} {}", g("phase").unwrap_or_else(z)));
    }
    parts.extend(wave);
    parts.join(" ")
}

/// Spectre (and embedded SPICE) text as SPICE lines. `spectre_first`: the text starts in the
/// Spectre language (a `.scs` file); otherwise it starts in SPICE until `simulator lang=spectre`.
pub fn to_spice(text: &str, spectre_first: bool) -> Translated {
    let mut r = Reader { out: vec![], warnings: vec![], subckt: vec![] };
    let mut spectre = spectre_first;
    let mut pending: Option<(usize, String)> = None;
    for (n, raw) in text.lines().enumerate() {
        let ln = n + 1;
        let t = raw.trim();
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("simulator") && lower.contains("lang") {
            if let Some((l, s)) = pending.take() {
                r.statement(l, &s);
            }
            spectre = lower.replace(' ', "").contains("lang=spectre");
            continue;
        }
        if spectre {
            // `//` comments (outside quotes), `*` comment lines, `\` continuations.
            let mut body = String::new();
            let mut quote = false;
            let mut chars = raw.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '"' {
                    quote = !quote;
                }
                if !quote && c == '/' && chars.peek() == Some(&'/') {
                    break;
                }
                body.push(c);
            }
            let body = body.trim();
            if pending.is_none() && (body.is_empty() || body.starts_with('*')) {
                continue;
            }
            let (part, cont) = match body.strip_suffix('\\') {
                Some(p) => (p.trim(), true),
                None => (body, false),
            };
            let (l, mut s) = pending.take().unwrap_or((ln, String::new()));
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(part);
            if cont {
                pending = Some((l, s));
            } else if !s.trim().is_empty() {
                r.statement(l, &s);
            }
        } else {
            // SPICE: as the SPICE reader does.
            let mut line = raw.split(';').next().unwrap_or("");
            if let Some(i) = line.find(" $") {
                line = &line[..i];
            }
            let t = line.trim();
            // `//` starts no SPICE statement: a Spectre-style comment ahead of `simulator lang`.
            if t.is_empty() || t.starts_with('*') || t.starts_with('$') || t.starts_with("//") {
                continue;
            }
            if let Some(cont) = t.strip_prefix('+') {
                if let Some(last) = r.out.last_mut() {
                    last.text.push(' ');
                    last.text.push_str(cont.trim());
                }
                continue;
            }
            r.push(ln, t.to_string());
        }
    }
    if let Some((l, s)) = pending {
        r.statement(l, &s);
    }
    Translated { lines: r.out, warnings: r.warnings }
}

/// SPICE value spelling in Spectre, whose scale factors are case sensitive: `meg` becomes `M`,
/// `g` / `t` become `G` / `T` (`m` is milli in both).
fn spectre_value(v: &str) -> String {
    let lower = v.to_ascii_lowercase();
    let num_end = lower.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e'))).unwrap_or(lower.len());
    // `1e3` style exponents end in a digit; a trailing `e` is not a suffix here.
    if num_end == 0 || v[..num_end].parse::<f64>().is_err() {
        return v.to_string();
    }
    let (num, suffix) = v.split_at(num_end);
    match suffix.to_ascii_lowercase().as_str() {
        s if s.starts_with("meg") => format!("{num}M"),
        "g" => format!("{num}G"),
        "t" => format!("{num}T"),
        _ => v.to_string(),
    }
}

fn spectre_params(toks: &[&str]) -> String {
    toks.iter()
        .map(|t| match t.split_once('=') {
            Some((k, v)) => format!("{}={}", k.to_ascii_lowercase(), spectre_value(v)),
            None => spectre_value(t),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A flat SPICE deck (as the netlister writes it) in the Spectre language.
pub fn from_spice(deck: &str) -> String {
    let mut out = String::from("// Generated by IC Studio\nsimulator lang=spectre\nglobal 0\n");
    let mut lines: Vec<String> = vec![];
    for raw in deck.lines() {
        let t = raw.trim();
        if let Some(c) = t.strip_prefix('+') {
            if let Some(last) = lines.last_mut() {
                last.push(' ');
                last.push_str(c.trim());
            }
        } else {
            lines.push(t.to_string());
        }
    }
    let mut analysis = 0;
    for l in lines {
        if l.is_empty() {
            continue;
        }
        if let Some(c) = l.strip_prefix('*') {
            out += &format!("//{c}\n");
            continue;
        }
        let spaced = l.replace('(', " ( ").replace(')', " ) ").replace(',', " ");
        let toks: Vec<&str> = l.split_whitespace().collect();
        let lower = l.to_ascii_lowercase();
        let cmd = toks[0].to_ascii_lowercase();
        let mut name = || {
            analysis += 1;
            analysis
        };
        match cmd.as_str() {
            ".end" => break,
            ".model" if toks.len() >= 3 => {
                let p: Vec<&str> = spaced.split_whitespace().skip(3).filter(|t| *t != "(" && *t != ")").collect();
                let (kv, _) = kv_words(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                let ty = toks[2].to_ascii_lowercase();
                let level = kv.get("level").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0) as i64;
                let master = match ty.as_str() {
                    "nmos" | "pmos" => {
                        let m = match level {
                            14 | 54 => "bsim4",
                            8 | 49 => "bsim3v3",
                            17 => "bsimcmg",
                            2 => "mos2",
                            3 => "mos3",
                            _ => "mos1",
                        };
                        format!("{m} type={}", if ty == "pmos" { "p" } else { "n" })
                    }
                    "d" => "diode".into(),
                    "npn" | "pnp" => format!("bjt type={ty}"),
                    other => other.to_string(),
                };
                let p: Vec<&str> = p.into_iter().filter(|t| !t.to_ascii_lowercase().starts_with("level=")).collect();
                out += &format!("model {} {master} {}\n", toks[1], spectre_params(&p));
            }
            ".lib" if toks.len() >= 3 => {
                out += &format!("include \"{}\" section={}\n", toks[1].trim_matches(['"', '\'']), toks[2])
            }
            ".lib" | ".include" | ".inc" if toks.len() >= 2 => {
                out += &format!("include \"{}\"\n", toks[1].trim_matches(['"', '\'']))
            }
            ".temp" if toks.len() >= 2 => out += &format!("tempOption options temp={}\n", toks[1]),
            ".param" => out += &format!("parameters {}\n", spectre_params(&toks[1..])),
            ".option" | ".options" => out += &format!("simOptions options {}\n", spectre_params(&toks[1..])),
            ".op" => out += &format!("op{} dc\n", name()),
            ".dc" if toks.len() >= 5 => {
                out += &format!("dc{} dc dev={} param=dc start={} stop={} step={}\n", name(), toks[1], toks[2], toks[3], toks[4])
            }
            ".ac" if toks.len() >= 5 => {
                let ty = toks[1].to_ascii_lowercase();
                out += &format!(
                    "ac{} ac start={} stop={} {ty}={}\n",
                    name(),
                    spectre_value(toks[3]),
                    spectre_value(toks[4]),
                    toks[2]
                )
            }
            ".tran" if toks.len() >= 3 => out += &format!("tran{} tran stop={} step={}\n", name(), toks[2], toks[1]),
            ".noise" if toks.len() >= 7 => {
                let node = toks[1].trim_start_matches(['v', 'V']).trim_matches(['(', ')']);
                out += &format!(
                    "noise{} ({node} 0) noise iprobe={} start={} stop={} dec={}\n",
                    name(),
                    toks[2],
                    spectre_value(toks[5]),
                    spectre_value(toks[6]),
                    toks[4]
                );
            }
            c if c.starts_with('.') => out += &format!("// not translated: {l}\n"),
            _ => out += &element_line(&l, &lower, &toks),
        }
    }
    out
}

fn element_line(l: &str, lower: &str, toks: &[&str]) -> String {
    let name = toks[0];
    let first = lower.chars().next().unwrap_or(' ');
    let two = |master: &str, key: &str| {
        if toks.len() < 4 {
            return format!("// not translated: {l}\n");
        }
        format!("{name} ({} {}) {master} {key}={}\n", toks[1], toks[2], spectre_value(toks[3]))
    };
    match first {
        'r' => two("resistor", "r"),
        'c' => two("capacitor", "c"),
        'l' => two("inductor", "l"),
        'v' | 'i' if toks.len() >= 3 => {
            let master = if first == 'v' { "vsource" } else { "isource" };
            let spaced = l.replace('(', " ( ").replace(')', " ) ").replace(',', " ");
            let t: Vec<String> = spaced.split_whitespace().skip(3).map(str::to_string).collect();
            let mut p = vec![];
            let mut i = 0;
            let nums = |i: &mut usize| -> Vec<String> {
                let mut v = vec![];
                while *i < t.len() && !["dc", "ac", "pulse", "sin", "pwl", "("].contains(&t[*i].to_ascii_lowercase().as_str()) {
                    if t[*i] != ")" {
                        v.push(spectre_value(&t[*i]));
                    }
                    *i += 1;
                }
                v
            };
            while i < t.len() {
                let k = t[i].to_ascii_lowercase();
                i += 1;
                match k.as_str() {
                    "dc" => p.push(format!("dc={}", nums(&mut i).first().cloned().unwrap_or("0".into()))),
                    "ac" => {
                        let v = nums(&mut i);
                        p.push(format!("mag={}", v.first().cloned().unwrap_or("1".into())));
                        if let Some(ph) = v.get(1) {
                            p.push(format!("phase={ph}"));
                        }
                    }
                    "pulse" | "sin" | "pwl" => {
                        while i < t.len() && t[i] == "(" {
                            i += 1;
                        }
                        let v = nums(&mut i);
                        let g = |k: usize| v.get(k).cloned().unwrap_or("0".into());
                        match k.as_str() {
                            "pulse" => p.push(format!(
                                "type=pulse val0={} val1={} delay={} rise={} fall={} width={} period={}",
                                g(0),
                                g(1),
                                g(2),
                                g(3),
                                g(4),
                                g(5),
                                g(6)
                            )),
                            "sin" => p.push(format!(
                                "type=sine sinedc={} ampl={} freq={} delay={} damp={}",
                                g(0),
                                g(1),
                                g(2),
                                g(3),
                                g(4)
                            )),
                            _ => p.push(format!("type=pwl wave=[{}]", v.join(" "))),
                        }
                    }
                    "(" | ")" => {}
                    other => p.push(format!("dc={}", spectre_value(other))),
                }
            }
            format!("{name} ({} {}) {master} {}\n", toks[1], toks[2], p.join(" "))
        }
        'd' if toks.len() >= 4 => format!("{name} ({} {}) {} {}\n", toks[1], toks[2], toks[3], spectre_params(&toks[4..])),
        'm' if toks.len() >= 6 => {
            format!("{name} ({} {} {} {}) {} {}\n", toks[1], toks[2], toks[3], toks[4], toks[5], spectre_params(&toks[6..]))
        }
        'x' => {
            let n = toks[1..].iter().take_while(|t| !t.contains('=')).count();
            if n == 0 {
                return format!("// not translated: {l}\n");
            }
            let (nodes, master) = (&toks[1..n], toks[n]);
            format!("{} ({}) {master} {}\n", &name[1..], nodes.join(" "), spectre_params(&toks[n + 1..]))
        }
        _ => format!("// not translated: {l}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(t: &Translated) -> Vec<&str> {
        t.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn numbers_take_spice_suffixes() {
        assert_eq!(
            convert_numbers("r=1M c=10f x=2.5e-3M w=wM a=3a b=4c p=5%"),
            "r=1meg c=10f x=2.5e-3meg w=wM a=3e-18 b=4e-2 p=5e-2"
        );
        assert_eq!(convert_numbers("f=1Meg nch2 m1=3m"), "f=1Meg nch2 m1=3m");
    }

    #[test]
    fn reads_a_spectre_deck() {
        let deck = "// inverter\nsimulator lang=spectre\nglobal 0 vdd!\nparameters vdd=1.8 wn = 1u\n\
            include \"models.scs\" section=tt\n\
            subckt inv (a y vdd vss)\n  parameters wp=2*wn\n  MP (y a vdd vdd) pch w=wp l=0.18u\n  MN (y a vss vss) nch w=wn \\\n  l=0.18u\nends inv\n\
            V1 (vdd 0) vsource dc=vdd\nV2 (in 0) vsource type=pulse val0=0 val1=vdd delay=1n rise=50p fall=50p width=2n period=4n\n\
            R1 (y 0) resistor r=1M\nC1 (y 0) capacitor c=10f\nX1 (in y vdd 0) inv wn=2u\n\
            ic y=0\nopts options reltol=1e-4 temp=50\ndcOp dc\nsweep1 dc dev=V2 start=0 stop=1.8 step=0.1\n\
            tr tran stop=10n\nac1 ac start=1 stop=1G dec=10\nnoi (y 0) noise start=1 stop=1G dec=5 iprobe=V2\nsave y\n";
        let t = to_spice(deck, false);
        assert_eq!(
            texts(&t),
            [
                ".global 0 vdd!",
                ".param vdd=1.8 wn=1u",
                ".lib \"models.scs\" tt",
                ".subckt inv a y vdd vss wp=2*wn",
                "@MP y a vdd vdd pch w=wp l=0.18u",
                "@MN y a vss vss nch w=wn l=0.18u",
                ".ends",
                "V1 vdd 0 DC vdd",
                "V2 in 0 DC 0 PULSE(0 vdd 1n 50p 50p 2n 4n)",
                "R1 y 0 1meg",
                "C1 y 0 10f",
                "@X1 in y vdd 0 inv wn=2u",
                ".ic v(y)=0",
                ".option reltol=1e-4 temp=50",
                ".op",
                ".dc V2 0 1.8 {0.1}",
                ".tran {(10n)/1000} 10n",
                ".ac dec 10 1 1G",
                ".noise v(y) V2 dec 5 1 1G",
            ]
        );
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        assert_eq!(t.lines[5].line, 9, "continued statements keep their first line");
    }

    #[test]
    fn library_sections_models_and_language_switches() {
        let lib = "simulator lang=spectre\nlibrary mylib\nsection tt\nparameters dvt=0\n\
            model nch bsim4 type=n version=4.5 vth0=0.45+dvt toxe=4n\nmodel pch bsim4 type=p vth0=-0.45\n\
            model dd diode is=1e-14\nendsection tt\nendlibrary mylib\n\
            simulator lang=spice\n.model nlegacy nmos level=1 vto=0.5\n+ kp=100u\n";
        let t = to_spice(lib, false);
        assert_eq!(
            texts(&t),
            [
                ".lib tt",
                ".param dvt=0",
                ".model nch nmos level=54 version=4.5 vth0=0.45+dvt toxe=4n",
                ".model pch pmos level=54 vth0=-0.45",
                ".model dd d is=1e-14",
                ".endl",
                ".model nlegacy nmos level=1 vto=0.5 kp=100u",
            ]
        );
        assert!(is_spectre(lib, None) && !is_spectre(".model x nmos\n", None));
        assert!(is_spectre("", Some(Path::new("a/b.SCS"))));
    }

    #[test]
    fn unsupported_constructs_warn() {
        let t = to_spice("ahdl_include \"x.va\"\nstatistics {\nm1 (a b) rpoly r=1\nsp1 sp ports=[p1]\n", true);
        assert_eq!(t.warnings.len(), 3, "{:?}", t.warnings);
        assert_eq!(texts(&t), ["@m1 a b rpoly r=1"]);
    }

    #[test]
    fn writes_spice_as_spectre() {
        let deck = "* t\n* Generated by IC Studio\nV1 in 0 DC 0 AC 1 PULSE(0 1.8 1n 50p 50p 2n 4n)\nR1 in out 1meg\nC1 out 0 10f\n\
            M1 out in 0 0 nmos w=1u l=0.18u\nXM2 out in vdd vdd sky130_fd_pr__pfet_01v8 w=2 l=0.15\nD1 out 0 dmod\n\
            .model nmos nmos (level=1 vto=0.45 kp=280u)\n.model dmod d (is=1e-14)\n.lib \"/pdk/models.lib\" tt\n\
            .temp 27\n.op\n.dc V1 0 1.8 0.1\n.ac dec 10 1 1g\n.tran 10p 10n\n.noise v(out) V1 dec 10 1 1meg\n.end\n";
        let s = from_spice(deck);
        for want in [
            "simulator lang=spectre",
            "V1 (in 0) vsource dc=0 mag=1 type=pulse val0=0 val1=1.8 delay=1n rise=50p fall=50p width=2n period=4n",
            "R1 (in out) resistor r=1M",
            "C1 (out 0) capacitor c=10f",
            "M1 (out in 0 0) nmos w=1u l=0.18u",
            "M2 (out in vdd vdd) sky130_fd_pr__pfet_01v8 w=2 l=0.15",
            "D1 (out 0) dmod",
            "model nmos mos1 type=n vto=0.45 kp=280u",
            "model dmod diode is=1e-14",
            "include \"/pdk/models.lib\" section=tt",
            "tempOption options temp=27",
            "op1 dc",
            "dc2 dc dev=V1 param=dc start=0 stop=1.8 step=0.1",
            "ac3 ac start=1 stop=1G dec=10",
            "tran4 tran stop=10n step=10p",
            "noise5 (out 0) noise iprobe=V1 start=1 stop=1M dec=10",
        ] {
            assert!(s.contains(want), "missing '{want}' in\n{s}");
        }
        // The Spectre text reads back as the same circuit.
        let back = to_spice(&s, true);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        let t = texts(&back);
        assert!(t.contains(&"R1 in out 1meg") && t.contains(&"@M1 out in 0 0 nmos w=1u l=0.18u"), "{t:#?}");
    }
}
