//! Waveform calculator: evaluates expressions such as `db(v(out)/v(in))` or
//! `cross(v(out), 0.5, 1, rise)` over a simulation [`Dataset`].

use ics_core::units::parse_spice;
use num_complex::Complex64 as C;

use crate::results::{Dataset, VecKind, Vector};

#[derive(Debug, Clone)]
pub enum CalcValue {
    Scalar(f64),
    /// Same length as the dataset scale; `im` is `Some` for complex data.
    Wave(Vector),
}

/// (signature, one-line description) of every function, for the UI's function list.
pub fn functions() -> &'static [(&'static str, &'static str)] {
    &[
        ("v(node)", "Node voltage; v(a,b) is v(a) - v(b)"),
        ("i(src)", "Branch current of a voltage source or inductor"),
        ("xval()", "The scale (time, frequency or sweep) as a waveform"),
        ("abs(w)", "Absolute value / magnitude"),
        ("mag(w)", "Magnitude"),
        ("db(w)", "20 * log10(|w|)"),
        ("db10(w)", "10 * log10(|w|)"),
        ("phase(w)", "Unwrapped phase in degrees"),
        ("real(w)", "Real part"),
        ("imag(w)", "Imaginary part"),
        ("conj(w)", "Complex conjugate"),
        ("sqrt(w)", "Square root"),
        ("exp(w)", "Exponential"),
        ("ln(w)", "Natural logarithm"),
        ("log10(w)", "Base-10 logarithm"),
        ("sin(w)", "Sine (radians)"),
        ("cos(w)", "Cosine (radians)"),
        ("deriv(w)", "Derivative with respect to the scale"),
        ("integ(w)", "Cumulative trapezoidal integral over the scale"),
        ("min(w)", "Minimum value"),
        ("max(w)", "Maximum value"),
        ("xmin(w)", "Scale value at the minimum"),
        ("xmax(w)", "Scale value at the maximum"),
        ("pp(w)", "Peak-to-peak value"),
        ("avg(w)", "Average over the scale"),
        ("rms(w)", "RMS value over the scale"),
        ("value(w, x)", "Value at scale point x (linear interpolation)"),
        ("cross(w, level [, n [, edge]])", "Scale value of the n-th crossing (n < 0 counts from the end; edge rise/fall/either)"),
        ("delay(w1, l1, w2, l2 [, edge1 [, edge2]])", "Scale distance between the first crossings of w1 and w2"),
        ("risetime(w [, lo%, hi%])", "Rise time between lo% and hi% of min..max (default 10/90)"),
        ("falltime(w [, hi%, lo%])", "Fall time between hi% and lo% of min..max (default 90/10)"),
        ("overshoot(w)", "Overshoot in percent of the initial-to-final step"),
        ("settling(w [, pct])", "Scale value after which w stays within pct% of the step around its final value (default 2)"),
        ("bandwidth(w [, db])", "Frequency where |w| drops db (default 3) dB below its first point"),
        ("ugf(w)", "Unity-gain frequency (|w| crosses 0 dB)"),
        ("phasemargin(w)", "180 + phase(w) at the unity-gain frequency, in degrees"),
        ("gainmargin(w)", "-db(w) at the frequency where the phase crosses -180 degrees"),
    ]
}

/// Evaluate `expr` over `ds`.
pub fn eval(expr: &str, ds: &Dataset) -> Result<CalcValue, String> {
    let toks = lex(expr)?;
    if toks.is_empty() {
        return Err("empty expression".into());
    }
    let n = match ds.scale_vector() {
        Some(s) => s.re.len(),
        None => ds.vectors.first().map_or(0, |v| v.re.len()),
    };
    let (x, xkind) = match ds.scale_vector() {
        Some(s) => (s.re.clone(), s.kind),
        None => ((0..n).map(|i| i as f64).collect(), VecKind::Other),
    };
    let mut p = Parser { toks, pos: 0, ctx: Ctx { ds, n, x, xkind } };
    let v = p.expr()?;
    if let Some(t) = p.toks.get(p.pos) {
        return Err(format!("unexpected {}", t.describe()));
    }
    match v {
        Val::Num(x) => Ok(CalcValue::Scalar(x)),
        Val::Wave(w) => Ok(CalcValue::Wave(Vector { name: expr.trim().to_string(), kind: w.kind, re: w.re, im: w.im })),
        Val::Name(s) => Err(unknown(&s)),
    }
}

fn unknown(s: &str) -> String {
    format!("unknown signal '{s}'")
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    /// Identifier or quoted name (`true` when quoted).
    Name(String, bool),
    /// `v(...)` or `i(...)` with its raw comma-separated arguments.
    Signal(char, Vec<String>),
    Op(char),
    LParen,
    RParen,
    Comma,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Num(x) => format!("number {x}"),
            Tok::Name(s, _) => format!("'{s}'"),
            Tok::Signal(c, a) => format!("'{c}({})'", a.join(",")),
            Tok::Op(c) => format!("'{c}'"),
            Tok::LParen => "'('".into(),
            Tok::RParen => "')'".into(),
            Tok::Comma => "','".into(),
        }
    }
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = s.chars().collect();
    let len = cs.len();
    let mut out = vec![];
    let mut i = 0;
    while i < len {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let st = i;
            while i < len && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            if i < len && (cs[i] == 'e' || cs[i] == 'E') {
                let j = if cs.get(i + 1).is_some_and(|c| *c == '+' || *c == '-') { i + 2 } else { i + 1 };
                if cs.get(j).is_some_and(|c| c.is_ascii_digit()) {
                    i = j;
                    while i < len && cs[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            while i < len && cs[i].is_alphabetic() {
                i += 1;
            }
            let t: String = cs[st..i].iter().collect();
            out.push(Tok::Num(parse_spice(&t).ok_or_else(|| format!("invalid number '{t}'"))?));
        } else if c.is_alphabetic() || c == '_' {
            let st = i;
            while i < len && (cs[i].is_alphanumeric() || matches!(cs[i], '_' | '.' | '#' | ':' | '$')) {
                i += 1;
            }
            let name: String = cs[st..i].iter().collect();
            let lname = name.to_ascii_lowercase();
            let mut j = i;
            while j < len && cs[j].is_whitespace() {
                j += 1;
            }
            if (lname == "v" || lname == "i") && cs.get(j) == Some(&'(') {
                let mut k = j + 1;
                let mut depth = 1;
                while k < len {
                    match cs[k] {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if k >= len {
                    return Err(format!("missing ')' after '{name}('"));
                }
                let inner: String = cs[j + 1..k].iter().collect();
                let args: Vec<String> = inner.split(',').map(|a| a.trim().to_string()).collect();
                if args.iter().any(String::is_empty) || args.len() > 2 {
                    return Err(format!("{lname}() takes one or two node names, got '{lname}({inner})'"));
                }
                out.push(Tok::Signal(if lname == "v" { 'v' } else { 'i' }, args));
                i = k + 1;
            } else {
                out.push(Tok::Name(name, false));
            }
        } else if c == '"' || c == '\'' {
            let end = cs[i + 1..].iter().position(|d| *d == c).ok_or_else(|| format!("unterminated {c}"))?;
            out.push(Tok::Name(cs[i + 1..i + 1 + end].iter().collect(), true));
            i += end + 2;
        } else {
            out.push(match c {
                '+' | '-' | '*' | '/' | '^' => Tok::Op(c),
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                ',' => Tok::Comma,
                _ => return Err(format!("unexpected character '{c}'")),
            });
            i += 1;
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct W {
    re: Vec<f64>,
    im: Option<Vec<f64>>,
    kind: VecKind,
}

impl W {
    fn real(re: Vec<f64>, kind: VecKind) -> Self {
        Self { re, im: None, kind }
    }

    fn at(&self, i: usize) -> C {
        C::new(self.re[i], self.im.as_ref().map_or(0.0, |v| v[i]))
    }

    fn from_c(cs: Vec<C>, kind: VecKind) -> Self {
        Self { re: cs.iter().map(|c| c.re).collect(), im: Some(cs.iter().map(|c| c.im).collect()), kind }
    }

    /// Real samples for measurements: magnitude for complex data.
    fn ys(&self) -> Vec<f64> {
        match &self.im {
            Some(im) => self.re.iter().zip(im).map(|(r, i)| r.hypot(*i)).collect(),
            None => self.re.clone(),
        }
    }

    fn phase(&self) -> Vec<f64> {
        let raw: Vec<f64> = match &self.im {
            Some(im) => self.re.iter().zip(im).map(|(r, i)| i.atan2(*r).to_degrees()).collect(),
            None => self.re.iter().map(|r| if *r < 0.0 { 180.0 } else { 0.0 }).collect(),
        };
        let mut out = raw.clone();
        let mut off = 0.0;
        for i in 1..raw.len() {
            let d = raw[i] - raw[i - 1];
            if d > 180.0 {
                off -= 360.0;
            } else if d < -180.0 {
                off += 360.0;
            }
            out[i] = raw[i] + off;
        }
        out
    }
}

#[derive(Debug, Clone)]
enum Val {
    Num(f64),
    Wave(W),
    /// An identifier that is not a signal (e.g. `rise`); only valid as a function argument.
    Name(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Edge {
    Rise,
    Fall,
    Either,
}

struct Ctx<'a> {
    ds: &'a Dataset,
    n: usize,
    x: Vec<f64>,
    xkind: VecKind,
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    ctx: Ctx<'a>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Result<Tok, String> {
        let t = self.toks.get(self.pos).cloned().ok_or("unexpected end of expression")?;
        self.pos += 1;
        Ok(t)
    }

    fn expect(&mut self, t: Tok) -> Result<(), String> {
        match self.next() {
            Ok(got) if got == t => Ok(()),
            Ok(got) => Err(format!("expected {}, found {}", t.describe(), got.describe())),
            Err(_) => Err(format!("expected {}", t.describe())),
        }
    }

    fn expr(&mut self) -> Result<Val, String> {
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = self.ctx.binop(v, r, op)?;
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<Val, String> {
        let mut v = self.unary()?;
        while let Some(Tok::Op(op @ ('*' | '/'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.unary()?;
            v = self.ctx.binop(v, r, op)?;
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<Val, String> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                let v = self.unary()?;
                self.ctx.neg(v)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                let v = self.unary()?;
                self.ctx.operand(v)
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Val, String> {
        let b = self.primary()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let e = self.unary()?;
            return self.ctx.binop(b, e, '^');
        }
        Ok(b)
    }

    fn primary(&mut self) -> Result<Val, String> {
        match self.next()? {
            Tok::Num(x) => Ok(Val::Num(x)),
            Tok::Signal(c, args) => self.ctx.signal(c, &args),
            Tok::Name(name, quoted) => {
                if !quoted && self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = vec![];
                    if self.peek() == Some(&Tok::RParen) {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.expr()?);
                            match self.next() {
                                Ok(Tok::Comma) => {}
                                Ok(Tok::RParen) => break,
                                Ok(t) => return Err(format!("expected ',' or ')' in {name}(), found {}", t.describe())),
                                Err(_) => return Err(format!("missing ')' after {name}(")),
                            }
                        }
                    }
                    self.ctx.call(&name, args)
                } else {
                    Ok(self.ctx.name(&name, quoted))
                }
            }
            Tok::LParen => {
                let v = self.expr()?;
                self.expect(Tok::RParen)?;
                Ok(v)
            }
            t => Err(format!("unexpected {}", t.describe())),
        }
    }
}

fn arity(f: &str, args: &[Val], min: usize, max: usize) -> Result<(), String> {
    let n = args.len();
    if (min..=max).contains(&n) {
        return Ok(());
    }
    let s = |k: usize| if k == 1 { "" } else { "s" };
    Err(if min == max {
        format!("{f}() takes {min} argument{}, got {n}", s(min))
    } else {
        format!("{f}() takes {min} to {max} arguments, got {n}")
    })
}

fn num(f: &str, v: &Val, what: &str) -> Result<f64, String> {
    match v {
        Val::Num(x) => Ok(*x),
        Val::Wave(_) => Err(format!("{f}(): {what} must be a scalar, not a waveform")),
        Val::Name(s) => Err(unknown(s)),
    }
}

fn edge(f: &str, v: &Val) -> Result<Edge, String> {
    match v {
        Val::Name(s) => match s.to_ascii_lowercase().as_str() {
            "rise" | "rising" => Ok(Edge::Rise),
            "fall" | "falling" => Ok(Edge::Fall),
            "either" | "any" => Ok(Edge::Either),
            _ => Err(format!("{f}(): edge must be rise, fall or either, not '{s}'")),
        },
        _ => Err(format!("{f}(): edge must be rise, fall or either")),
    }
}

fn interp_x(x0: f64, x1: f64, t: f64, logx: bool) -> f64 {
    if logx && x0 > 0.0 && x1 > 0.0 { 10f64.powf(x0.log10() + t * (x1.log10() - x0.log10())) } else { x0 + t * (x1 - x0) }
}

fn crossings(x: &[f64], y: &[f64], level: f64, edge: Edge, logx: bool) -> Vec<f64> {
    let mut out = vec![];
    for i in 0..y.len().saturating_sub(1) {
        let (a, b) = (y[i] - level, y[i + 1] - level);
        let rise = a < 0.0 && b >= 0.0;
        let fall = a > 0.0 && b <= 0.0;
        if (rise && edge != Edge::Fall) || (fall && edge != Edge::Rise) {
            out.push(interp_x(x[i], x[i + 1], a / (a - b), logx));
        }
    }
    out
}

fn interp_at(x: &[f64], y: &[f64], xq: f64, logx: bool) -> Option<f64> {
    if y.len() == 1 {
        return Some(y[0]);
    }
    for i in 0..y.len().saturating_sub(1) {
        let (x0, x1) = (x[i], x[i + 1]);
        if xq >= x0.min(x1) && xq <= x0.max(x1) {
            if x0 == x1 {
                return Some(y[i]);
            }
            let t = if logx && x0 > 0.0 && x1 > 0.0 && xq > 0.0 {
                (xq.log10() - x0.log10()) / (x1.log10() - x0.log10())
            } else {
                (xq - x0) / (x1 - x0)
            };
            return Some(y[i] + t * (y[i + 1] - y[i]));
        }
    }
    None
}

fn trapz(x: &[f64], y: &[f64]) -> f64 {
    (0..y.len().saturating_sub(1)).map(|i| 0.5 * (y[i] + y[i + 1]) * (x[i + 1] - x[i])).sum()
}

fn db(y: &[f64]) -> Vec<f64> {
    y.iter().map(|m| 20.0 * m.abs().max(1e-300).log10()).collect()
}

type Fr = fn(f64) -> f64;
type Fc = fn(C) -> C;

/// Element-wise functions: (name, real fn, complex fn, result is real).
const ELEMENTWISE: &[(&str, Fr, Fc, bool)] = &[
    ("abs", f64::abs, |c| C::from(c.norm()), true),
    ("mag", f64::abs, |c| C::from(c.norm()), true),
    ("db", |x| 20.0 * x.abs().max(1e-300).log10(), |c| C::from(20.0 * c.norm().max(1e-300).log10()), true),
    ("db10", |x| 10.0 * x.abs().max(1e-300).log10(), |c| C::from(10.0 * c.norm().max(1e-300).log10()), true),
    ("real", |x| x, |c| C::from(c.re), true),
    ("imag", |_| 0.0, |c| C::from(c.im), true),
    ("conj", |x| x, |c| c.conj(), false),
    ("sqrt", f64::sqrt, |c| c.sqrt(), false),
    ("exp", f64::exp, |c| c.exp(), false),
    ("ln", f64::ln, |c| c.ln(), false),
    ("log10", f64::log10, |c| c.log10(), false),
    ("sin", f64::sin, |c| c.sin(), false),
    ("cos", f64::cos, |c| c.cos(), false),
];

impl Ctx<'_> {
    fn vector(&self, v: &Vector) -> Result<W, String> {
        if v.re.len() != self.n {
            return Err(format!("signal '{}' has {} points, the scale has {}", v.name, v.re.len(), self.n));
        }
        Ok(W { re: v.re.clone(), im: v.im.clone().filter(|im| im.len() == v.re.len()), kind: v.kind })
    }

    fn node(&self, node: &str) -> Result<W, String> {
        if let Some(v) = self.ds.get(&format!("v({node})")) {
            return self.vector(v);
        }
        if node == "0" || node.eq_ignore_ascii_case("gnd") {
            return Ok(W::real(vec![0.0; self.n], VecKind::Voltage));
        }
        Err(unknown(&format!("v({node})")))
    }

    fn signal(&self, c: char, args: &[String]) -> Result<Val, String> {
        if c == 'i' {
            if args.len() != 1 {
                return Err("i() takes one source name".into());
            }
            let name = format!("i({})", args[0]);
            return self.ds.get(&name).ok_or_else(|| unknown(&name)).and_then(|v| self.vector(v)).map(Val::Wave);
        }
        let a = self.node(&args[0])?;
        match args.get(1) {
            None => Ok(Val::Wave(a)),
            Some(b) => {
                let b = self.node(b)?;
                self.binop(Val::Wave(a), Val::Wave(b), '-')
            }
        }
    }

    fn scale(&self) -> W {
        W::real(self.x.clone(), self.xkind)
    }

    fn name(&self, s: &str, quoted: bool) -> Val {
        if let Some(v) = self.ds.get(s)
            && let Ok(w) = self.vector(v)
        {
            return Val::Wave(w);
        }
        if !quoted {
            match s.to_ascii_lowercase().as_str() {
                "time" if self.xkind == VecKind::Time => return Val::Wave(self.scale()),
                "freq" | "frequency" if self.xkind == VecKind::Frequency => return Val::Wave(self.scale()),
                "pi" => return Val::Num(std::f64::consts::PI),
                _ => {}
            }
        }
        Val::Name(s.to_string())
    }

    fn operand(&self, v: Val) -> Result<Val, String> {
        match v {
            Val::Name(s) => Err(unknown(&s)),
            v => Ok(v),
        }
    }

    fn wave(&self, f: &str, v: &Val) -> Result<W, String> {
        match v {
            Val::Num(x) => Ok(W::real(vec![*x; self.n], VecKind::Other)),
            Val::Wave(w) => Ok(w.clone()),
            Val::Name(s) => Err(format!("{f}(): {}", unknown(s))),
        }
    }

    /// Waveform argument for a measurement, as real samples (magnitude for complex data).
    fn samples(&self, f: &str, v: &Val) -> Result<Vec<f64>, String> {
        let y = self.wave(f, v)?.ys();
        if y.is_empty() {
            return Err(format!("{f}(): empty waveform"));
        }
        Ok(y)
    }

    fn neg(&self, v: Val) -> Result<Val, String> {
        match self.operand(v)? {
            Val::Num(x) => Ok(Val::Num(-x)),
            Val::Wave(mut w) => {
                w.re.iter_mut().for_each(|r| *r = -*r);
                if let Some(im) = &mut w.im {
                    im.iter_mut().for_each(|r| *r = -*r);
                }
                Ok(Val::Wave(w))
            }
            Val::Name(_) => unreachable!(),
        }
    }

    fn binop(&self, a: Val, b: Val, op: char) -> Result<Val, String> {
        let (a, b) = (self.operand(a)?, self.operand(b)?);
        let real = |x: f64, y: f64| match op {
            '+' => x + y,
            '-' => x - y,
            '*' => x * y,
            '/' => x / y,
            _ => x.powf(y),
        };
        if let (Val::Num(x), Val::Num(y)) = (&a, &b) {
            return Ok(Val::Num(real(*x, *y)));
        }
        let kind = match (&a, &b) {
            (Val::Wave(w), Val::Num(_)) | (Val::Num(_), Val::Wave(w)) if op != '^' => w.kind,
            (Val::Wave(x), Val::Wave(y)) if x.kind == y.kind && (op == '+' || op == '-') => x.kind,
            _ => VecKind::Other,
        };
        let (wa, wb) = (self.wave("", &a)?, self.wave("", &b)?);
        if wa.re.len() != wb.re.len() {
            return Err(format!("waveform lengths differ ({} vs {})", wa.re.len(), wb.re.len()));
        }
        if wa.im.is_none() && wb.im.is_none() {
            return Ok(Val::Wave(W::real(wa.re.iter().zip(&wb.re).map(|(x, y)| real(*x, *y)).collect(), kind)));
        }
        let cs = (0..wa.re.len())
            .map(|i| {
                let (x, y) = (wa.at(i), wb.at(i));
                match op {
                    '+' => x + y,
                    '-' => x - y,
                    '*' => x * y,
                    '/' => x / y,
                    _ if y.im == 0.0 && y.re.fract() == 0.0 && y.re.abs() < i32::MAX as f64 => x.powi(y.re as i32),
                    _ => x.powc(y),
                }
            })
            .collect();
        Ok(Val::Wave(W::from_c(cs, kind)))
    }

    fn call(&self, name: &str, args: Vec<Val>) -> Result<Val, String> {
        let f = name.to_ascii_lowercase();
        let f = f.as_str();
        if let Some((_, fr, fc, real_out)) = ELEMENTWISE.iter().find(|e| e.0 == f) {
            arity(f, &args, 1, 1)?;
            return match self.operand(args[0].clone())? {
                Val::Num(x) => Ok(Val::Num(fr(x))),
                Val::Wave(w) => Ok(Val::Wave(match &w.im {
                    None => W::real(w.re.iter().map(|x| fr(*x)).collect(), VecKind::Other),
                    Some(_) => {
                        let cs: Vec<C> = (0..w.re.len()).map(|i| fc(w.at(i))).collect();
                        if *real_out {
                            W::real(cs.iter().map(|c| c.re).collect(), VecKind::Other)
                        } else {
                            W::from_c(cs, VecKind::Other)
                        }
                    }
                })),
                Val::Name(_) => unreachable!(),
            };
        }
        let x = &self.x;
        let logx = self.xkind == VecKind::Frequency;
        let scalar = |v: f64| Ok(Val::Num(v));
        match f {
            "xval" => {
                arity(f, &args, 0, 1)?;
                Ok(Val::Wave(self.scale()))
            }
            "phase" => {
                arity(f, &args, 1, 1)?;
                match self.operand(args[0].clone())? {
                    Val::Num(v) => scalar(if v < 0.0 { 180.0 } else { 0.0 }),
                    v => Ok(Val::Wave(W::real(self.wave(f, &v)?.phase(), VecKind::Other))),
                }
            }
            "deriv" | "integ" => {
                arity(f, &args, 1, 1)?;
                let w = self.wave(f, &args[0])?;
                let n = w.re.len();
                if f == "deriv" && n < 2 {
                    return Err("deriv(): needs at least 2 points".into());
                }
                let op = |y: &[f64]| -> Vec<f64> {
                    if f == "deriv" {
                        (0..n)
                            .map(|i| {
                                let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
                                (y[b] - y[a]) / (x[b] - x[a])
                            })
                            .collect()
                    } else {
                        let mut acc = 0.0;
                        (0..n)
                            .map(|i| {
                                if i > 0 {
                                    acc += 0.5 * (y[i] + y[i - 1]) * (x[i] - x[i - 1]);
                                }
                                acc
                            })
                            .collect()
                    }
                };
                Ok(Val::Wave(W { re: op(&w.re), im: w.im.as_deref().map(op), kind: VecKind::Other }))
            }
            "min" | "max" | "xmin" | "xmax" | "pp" => {
                arity(f, &args, 1, 1)?;
                let y = self.samples(f, &args[0])?;
                let (mut imin, mut imax) = (0, 0);
                for (i, v) in y.iter().enumerate() {
                    if *v < y[imin] || y[imin].is_nan() {
                        imin = i;
                    }
                    if *v > y[imax] || y[imax].is_nan() {
                        imax = i;
                    }
                }
                scalar(match f {
                    "min" => y[imin],
                    "max" => y[imax],
                    "xmin" => x[imin],
                    "xmax" => x[imax],
                    _ => y[imax] - y[imin],
                })
            }
            "avg" | "rms" => {
                arity(f, &args, 1, 1)?;
                let mut y = self.samples(f, &args[0])?;
                if f == "rms" {
                    y.iter_mut().for_each(|v| *v *= *v);
                }
                let span = x[y.len() - 1] - x[0];
                let m = if span == 0.0 { y.iter().sum::<f64>() / y.len() as f64 } else { trapz(x, &y) / span };
                scalar(if f == "rms" { m.sqrt() } else { m })
            }
            "value" => {
                arity(f, &args, 2, 2)?;
                let y = self.samples(f, &args[0])?;
                let xq = num(f, &args[1], "x")?;
                interp_at(x, &y, xq, false).map(Val::Num).ok_or_else(|| format!("value(): x = {xq} is outside the scale range"))
            }
            "cross" => {
                arity(f, &args, 2, 4)?;
                let y = self.samples(f, &args[0])?;
                let level = num(f, &args[1], "level")?;
                let nth = args.get(2).map(|v| num(f, v, "n")).transpose()?.unwrap_or(1.0);
                if nth.fract() != 0.0 || nth == 0.0 {
                    return Err(format!("cross(): n must be a non-zero integer, got {nth}"));
                }
                let e = args.get(3).map(|v| edge(f, v)).transpose()?.unwrap_or(Edge::Either);
                let xs = crossings(x, &y, level, e, false);
                if xs.is_empty() {
                    return Err(format!("cross(): no crossing of level {level} found"));
                }
                let idx = if nth > 0.0 { nth as i64 - 1 } else { xs.len() as i64 + nth as i64 };
                usize::try_from(idx)
                    .ok()
                    .and_then(|i| xs.get(i))
                    .map(|v| Val::Num(*v))
                    .ok_or_else(|| format!("cross(): only {} crossing(s) of level {level} found", xs.len()))
            }
            "delay" => {
                arity(f, &args, 4, 6)?;
                let y1 = self.samples(f, &args[0])?;
                let l1 = num(f, &args[1], "level1")?;
                let y2 = self.samples(f, &args[2])?;
                let l2 = num(f, &args[3], "level2")?;
                let e1 = args.get(4).map(|v| edge(f, v)).transpose()?.unwrap_or(Edge::Either);
                let e2 = args.get(5).map(|v| edge(f, v)).transpose()?.unwrap_or(Edge::Either);
                let t1 = crossings(x, &y1, l1, e1, false).first().copied();
                let t2 = crossings(x, &y2, l2, e2, false).first().copied();
                match (t1, t2) {
                    (Some(a), Some(b)) => scalar(b - a),
                    (None, _) => Err(format!("delay(): first waveform never crosses {l1}")),
                    (_, None) => Err(format!("delay(): second waveform never crosses {l2}")),
                }
            }
            "risetime" | "falltime" => {
                arity(f, &args, 1, 3)?;
                let y = self.samples(f, &args[0])?;
                let rise = f == "risetime";
                let (d1, d2) = if rise { (10.0, 90.0) } else { (90.0, 10.0) };
                let p1 = args.get(1).map(|v| num(f, v, "first percentage")).transpose()?.unwrap_or(d1);
                let p2 = args.get(2).map(|v| num(f, v, "second percentage")).transpose()?.unwrap_or(d2);
                let lo = y.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = y.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                if hi <= lo {
                    return Err(format!("{f}(): flat waveform"));
                }
                let e = if rise { Edge::Rise } else { Edge::Fall };
                let l1 = lo + p1 / 100.0 * (hi - lo);
                let l2 = lo + p2 / 100.0 * (hi - lo);
                let edge_name = if rise { "rising" } else { "falling" };
                let t1 =
                    crossings(x, &y, l1, e, false).first().copied().ok_or_else(|| format!("{f}(): no {edge_name} edge found"))?;
                let t2 = crossings(x, &y, l2, e, false)
                    .into_iter()
                    .find(|t| *t >= t1)
                    .ok_or_else(|| format!("{f}(): {edge_name} edge does not complete"))?;
                scalar(t2 - t1)
            }
            "overshoot" | "settling" => {
                arity(f, &args, 1, if f == "settling" { 2 } else { 1 })?;
                let y = self.samples(f, &args[0])?;
                let (y0, yf) = (y[0], y[y.len() - 1]);
                let step = yf - y0;
                if step == 0.0 || !step.is_finite() {
                    return Err(format!("{f}(): initial and final values are equal"));
                }
                if f == "overshoot" {
                    let peak = if step > 0.0 {
                        y.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                    } else {
                        y.iter().copied().fold(f64::INFINITY, f64::min)
                    };
                    return scalar((peak - yf) / step * 100.0);
                }
                let pct = args.get(1).map(|v| num(f, v, "pct")).transpose()?.unwrap_or(2.0);
                let tol = pct.abs() / 100.0 * step.abs();
                match (0..y.len()).rev().find(|i| (y[*i] - yf).abs() > tol) {
                    None => scalar(x[0]),
                    Some(i) => {
                        let level = if y[i] > yf { yf + tol } else { yf - tol };
                        let t = (y[i] - level) / (y[i] - y[i + 1]);
                        scalar(interp_x(x[i], x[i + 1], t, false))
                    }
                }
            }
            "bandwidth" | "ugf" | "phasemargin" | "gainmargin" => {
                arity(f, &args, 1, if f == "bandwidth" { 2 } else { 1 })?;
                let w = self.wave(f, &args[0])?;
                let g = db(&self.samples(f, &args[0])?);
                let ugf = || {
                    crossings(x, &g, 0.0, Edge::Either, logx)
                        .first()
                        .copied()
                        .ok_or_else(|| format!("{f}(): gain never crosses 0 dB"))
                };
                match f {
                    "bandwidth" => {
                        let drop = args.get(1).map(|v| num(f, v, "db")).transpose()?.unwrap_or(3.0);
                        let target = g[0] - drop;
                        crossings(x, &g, target, Edge::Fall, logx)
                            .first()
                            .map(|v| Val::Num(*v))
                            .ok_or_else(|| format!("bandwidth(): response never drops {drop} dB below its first point"))
                    }
                    "ugf" => ugf().map(Val::Num),
                    "phasemargin" => {
                        let fu = ugf()?;
                        let ph = interp_at(x, &w.phase(), fu, logx).ok_or("phasemargin(): phase not available at the UGF")?;
                        scalar(180.0 + ph)
                    }
                    _ => {
                        let ph = w.phase();
                        let f180 = crossings(x, &ph, -180.0, Edge::Either, logx)
                            .first()
                            .copied()
                            .ok_or("gainmargin(): phase never crosses -180 degrees")?;
                        let gm = interp_at(x, &g, f180, logx).ok_or("gainmargin(): gain not available")?;
                        scalar(-gm)
                    }
                }
            }
            _ => Err(format!("unknown function '{name}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(analysis: &str, scale: Option<(&str, VecKind, Vec<f64>)>, sigs: Vec<Vector>) -> Dataset {
        let mut vectors = vec![];
        let has_scale = scale.is_some();
        if let Some((n, k, x)) = scale {
            vectors.push(Vector::real(n, k, x));
        }
        vectors.extend(sigs);
        Dataset { analysis: analysis.into(), title: String::new(), scale: has_scale.then_some(0), vectors, devices: vec![] }
    }

    fn tran(t: Vec<f64>, sigs: Vec<(&str, Vec<f64>)>) -> Dataset {
        ds(
            "tran",
            Some(("time", VecKind::Time, t)),
            sigs.into_iter().map(|(n, y)| Vector::real(n, VecKind::Voltage, y)).collect(),
        )
    }

    fn lin(a: f64, b: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| a + (b - a) * i as f64 / (n - 1) as f64).collect()
    }

    /// AC dataset with v(in) = 1 and v(out) = h(f).
    fn ac(h: impl Fn(f64) -> C) -> Dataset {
        let f: Vec<f64> = (0..=180).map(|i| 10f64.powf(i as f64 / 20.0)).collect();
        let out: Vec<C> = f.iter().map(|f| h(*f)).collect();
        let n = f.len();
        let cv = |name: &str, cs: Vec<C>| Vector {
            name: name.into(),
            kind: VecKind::Voltage,
            re: cs.iter().map(|c| c.re).collect(),
            im: Some(cs.iter().map(|c| c.im).collect()),
        };
        ds(
            "ac",
            Some(("frequency", VecKind::Frequency, f)),
            vec![cv("v(in)", vec![C::new(2.0, 0.0); n]), cv("v(out)", out.into_iter().map(|c| c * 2.0).collect())],
        )
    }

    fn s(expr: &str, d: &Dataset) -> f64 {
        match eval(expr, d) {
            Ok(CalcValue::Scalar(x)) => x,
            other => panic!("{expr}: expected scalar, got {other:?}"),
        }
    }

    fn w(expr: &str, d: &Dataset) -> Vector {
        match eval(expr, d) {
            Ok(CalcValue::Wave(v)) => v,
            other => panic!("{expr}: expected wave, got {other:?}"),
        }
    }

    fn err(expr: &str, d: &Dataset) -> String {
        match eval(expr, d) {
            Err(e) => e,
            Ok(v) => panic!("{expr}: expected error, got {v:?}"),
        }
    }

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
    }

    fn empty() -> Dataset {
        ds("op", None, vec![])
    }

    #[test]
    fn arithmetic_and_precedence() {
        let d = empty();
        close(s("1+2*3", &d), 7.0, 0.0);
        close(s("(1+2)*3", &d), 9.0, 0.0);
        close(s("2^3^2", &d), 512.0, 0.0);
        close(s("-2^2", &d), -4.0, 0.0);
        close(s("2*-3", &d), -6.0, 0.0);
        close(s("10-4-3", &d), 3.0, 0.0);
        close(s("8/4/2", &d), 1.0, 0.0);
        close(s("10k/2meg", &d), 0.005, 1e-15);
        close(s("1n*1e9 + 1.5e+3m", &d), 2.5, 1e-12);
        close(s("2*pi", &d), std::f64::consts::TAU, 1e-12);
        close(s("db(10)", &d), 20.0, 1e-12);
        close(s("sqrt(16) + abs(-1)", &d), 5.0, 0.0);
    }

    #[test]
    fn signals_and_broadcasting() {
        let d = tran(
            vec![0.0, 1.0, 2.0],
            vec![("v(a)", vec![1.0, 2.0, 3.0]), ("v(b)", vec![0.5, 0.5, 0.5]), ("i(vdd)", vec![1e-3; 3])],
        );
        let v = w("v(a)*2 + 1", &d);
        assert_eq!(v.re, vec![3.0, 5.0, 7.0]);
        assert_eq!(v.name, "v(a)*2 + 1");
        assert_eq!(v.kind, VecKind::Voltage);
        assert_eq!(w("v(a,b)", &d).re, vec![0.5, 1.5, 2.5]);
        assert_eq!(w("v(a, 0)", &d).re, vec![1.0, 2.0, 3.0]);
        assert_eq!(w("V(A)", &d).re, vec![1.0, 2.0, 3.0]);
        assert_eq!(w("i(vdd)*1k", &d).re, vec![1.0; 3]);
        assert_eq!(w("time", &d).re, vec![0.0, 1.0, 2.0]);
        assert_eq!(w("xval()", &d).kind, VecKind::Time);
        assert_eq!(w("\"v(b)\"", &d).re, vec![0.5; 3]);
        close(s("max(v(a)) - min(v(a))", &d), 2.0, 0.0);
        close(s("value(v(a), 1.5)", &d), 2.5, 1e-12);
        close(s("xmax(v(a))", &d), 2.0, 0.0);
    }

    #[test]
    fn complex_divide_on_ac() {
        let fp = 1e3;
        let d = ac(|f| C::new(1.0, 0.0) / C::new(1.0, f / fp));
        let h = w("v(out)/v(in)", &d);
        assert!(h.im.is_some());
        let i = 60; // f = 1 kHz
        close(h.re[i].hypot(h.im.as_ref().unwrap()[i]), 1.0 / 2f64.sqrt(), 1e-12);
        close(w("db(v(out)/v(in))", &d).re[i], -3.0103, 1e-4);
        close(w("phase(v(out)/v(in))", &d).re[i], -45.0, 1e-9);
        assert!(w("mag(v(out))", &d).im.is_none());
        close(w("imag(conj(v(out)))", &d).re[i], 1.0, 1e-12);
        close(s("value(mag(v(out)/v(in)), 1k)", &d), 1.0 / 2f64.sqrt(), 1e-12);
    }

    #[test]
    fn deriv_and_integ_of_ramp() {
        let t = lin(0.0, 1.0, 11);
        let d = tran(t.clone(), vec![("v(a)", t.iter().map(|x| 2.0 * x).collect())]);
        for v in w("deriv(v(a))", &d).re {
            close(v, 2.0, 1e-9);
        }
        for (v, x) in w("integ(v(a))", &d).re.iter().zip(&t) {
            close(*v, x * x, 1e-12);
        }
        close(s("avg(v(a))", &d), 1.0, 1e-12);
    }

    #[test]
    fn rms_of_sine() {
        let t = lin(0.0, 1e-3, 1001);
        let y = t.iter().map(|t| (std::f64::consts::TAU * 1e3 * t).sin()).collect();
        let d = tran(t, vec![("v(s)", y)]);
        close(s("rms(v(s))", &d), 1.0 / 2f64.sqrt(), 1e-4);
        close(s("avg(v(s))", &d), 0.0, 1e-9);
        close(s("pp(v(s))", &d), 2.0, 1e-4);
    }

    #[test]
    fn crossings_delay_and_edges() {
        let t = lin(0.0, 1.0, 11);
        let up = t.clone();
        let late: Vec<f64> = t.iter().map(|x| (x - 0.2).max(0.0)).collect();
        let tri: Vec<f64> = t.iter().map(|x| 1.0 - (2.0 * x - 1.0).abs()).collect();
        let d = tran(t, vec![("v(a)", up), ("v(b)", late), ("v(tri)", tri)]);
        close(s("cross(v(a), 0.55)", &d), 0.55, 1e-12);
        close(s("cross(v(tri), 0.5, 2)", &d), 0.75, 1e-12);
        close(s("cross(v(tri), 0.5, 1, fall)", &d), 0.75, 1e-12);
        close(s("cross(v(tri), 0.5, -1)", &d), 0.75, 1e-12);
        close(s("cross(v(tri), 0.5, 1, \"rise\")", &d), 0.25, 1e-12);
        close(s("delay(v(a), 0.3, v(b), 0.3)", &d), 0.2, 1e-12);
        close(s("delay(v(tri), 0.5, v(tri), 0.5, rise, fall)", &d), 0.5, 1e-12);
        close(s("risetime(v(a))", &d), 0.8, 1e-12);
        close(s("risetime(v(a), 20, 80)", &d), 0.6, 1e-12);
        close(s("falltime(v(tri))", &d), 0.4, 1e-12);
        assert!(err("cross(v(a), 0.5, 1, fall)", &d).contains("no crossing"));
        assert!(err("cross(v(tri), 0.5, 3)", &d).contains("only 2"));
        assert!(err("falltime(v(a))", &d).contains("no falling edge"));
        assert!(err("cross(v(a), 0.5, 1, sideways)", &d).contains("edge must be"));
    }

    #[test]
    fn step_overshoot_and_settling() {
        let t = lin(0.0, 6.0, 7);
        let y = vec![0.0, 0.0, 1.2, 0.9, 1.05, 1.0, 1.0];
        let d = tran(t, vec![("v(o)", y)]);
        close(s("overshoot(v(o))", &d), 20.0, 1e-9);
        close(s("settling(v(o), 10)", &d), 2.0 + 1.0 / 3.0, 1e-9);
        close(s("settling(v(o), 1)", &d), 4.0 + 0.04 / 0.05, 1e-9);
    }

    #[test]
    fn single_pole_bandwidth_ugf_phase_margin() {
        let (a, fp) = (1000.0, 1e3);
        let d = ac(|f| C::new(a, 0.0) / C::new(1.0, f / fp));
        let h = "v(out)/v(in)";
        let bw = s(&format!("bandwidth({h})"), &d);
        close(bw / (fp * (10f64.powf(0.3) - 1.0).sqrt()), 1.0, 0.01);
        close(s(&format!("ugf({h})"), &d) / (fp * (a * a - 1.0f64).sqrt()), 1.0, 0.01);
        let pm = s(&format!("phasemargin({h})"), &d);
        close(pm, 180.0 - (a * a - 1.0f64).sqrt().atan().to_degrees(), 0.1);
        assert!(err(&format!("gainmargin({h})"), &d).contains("-180"));
    }

    #[test]
    fn three_pole_gain_margin() {
        let d = ac(|f| C::new(4.0, 0.0) / C::new(1.0, f / 1e3).powi(3));
        close(s("gainmargin(v(out)/v(in))", &d), 20.0 * 2f64.log10(), 0.05);
        assert!(s("phasemargin(v(out)/v(in))", &d) > 0.0);
    }

    #[test]
    fn op_dataset_and_empty_dataset() {
        let op = ds("op", None, vec![Vector::real("v(a)", VecKind::Voltage, vec![1.8])]);
        close(s("max(v(a))", &op), 1.8, 0.0);
        close(s("value(v(a), 0)", &op), 1.8, 0.0);
        assert_eq!(w("v(a)/2", &op).re, vec![0.9]);
        assert!(err("deriv(v(a))", &op).contains("2 points"));
        assert!(err("cross(v(a), 1)", &op).contains("no crossing"));
        let e = empty();
        assert!(err("max(v(a))", &e).contains("unknown signal 'v(a)'"));
        assert!(err("max(1)", &e).contains("empty waveform"));
        assert!(err("rms(xval())", &e).contains("empty waveform"));
        let blank = tran(vec![], vec![("v(a)", vec![])]);
        assert!(err("bandwidth(v(a))", &blank).contains("empty"));
        assert!(err("overshoot(v(a))", &blank).contains("empty"));
    }

    #[test]
    fn errors() {
        let d = tran(vec![0.0, 1.0], vec![("v(a)", vec![0.0, 1.0])]);
        assert_eq!(err("v(x)", &d), "unknown signal 'v(x)'");
        assert_eq!(err("i(vx) + 1", &d), "unknown signal 'i(vx)'");
        assert_eq!(err("foo + 1", &d), "unknown signal 'foo'");
        assert_eq!(err("rise", &d), "unknown signal 'rise'");
        assert!(err("abs(v(a), 2)", &d).contains("takes 1 argument, got 2"));
        assert!(err("cross(v(a))", &d).contains("takes 2 to 4 arguments"));
        assert!(err("bogus(1)", &d).contains("unknown function 'bogus'"));
        assert!(err("cross(v(a), v(a))", &d).contains("must be a scalar"));
        assert!(err("risetime(v(a) * 0)", &d).contains("flat"));
        assert!(err("overshoot(v(a)*0)", &d).contains("equal"));
        assert!(err("ugf(v(a)*0.5)", &d).contains("0 dB"));
        assert!(err("value(v(a), 5)", &d).contains("outside"));
        assert!(err("frequency", &d).contains("unknown signal"));
        for bad in [
            "",
            " ",
            "(",
            ")",
            "1+",
            "*2",
            "v(",
            "v()",
            "v(a,b,c)",
            "i(a,b)",
            "abs(",
            "abs(1",
            "abs(1 2)",
            "(1",
            "1)",
            "..",
            "\"a",
            "1 2",
            ",",
            "^^",
            "cross(,)",
            "@",
            "v(a) v(a)",
        ] {
            assert!(eval(bad, &d).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn every_listed_function_is_known() {
        let d = tran(lin(0.0, 1.0, 5), vec![("v(a)", lin(0.0, 1.0, 5))]);
        for (sig, desc) in functions() {
            assert!(!desc.is_empty());
            let name = &sig[..sig.find('(').unwrap()];
            if name == "v" || name == "i" {
                continue;
            }
            let e = eval(&format!("{name}()"), &d).err().unwrap_or_default();
            assert!(!e.contains("unknown function"), "{name}: {e}");
        }
    }
}
