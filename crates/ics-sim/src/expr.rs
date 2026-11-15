//! Parameter expressions as written in SPICE decks and PDK model libraries
//! (`{w*2}`, `'vdd/2'`, `.param l=0.15u*lmult`).
//!
//! Numbers take SPICE scale suffixes (`1u`, `10k`, `2meg`). Operators, from lowest to highest
//! precedence: `?:`, `||`, `&&`, comparisons, `+ -`, `* / %`, unary `- + !`, and `^` / `**`
//! (right-associative). Names are case-insensitive. The statistical functions (`agauss`,
//! `gauss`, `aunif`, `unif`) evaluate to their nominal value.

use std::collections::HashMap;

use ics_core::units::parse_spice;

/// Parameter values by lower-case name.
pub type Params = HashMap<String, f64>;

#[derive(Debug, Clone, PartialEq)]
pub enum ExprError {
    /// The expression names a parameter that is not defined (yet).
    Unknown(String),
    Syntax(String),
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExprError::Unknown(n) => write!(f, "unknown parameter '{n}'"),
            ExprError::Syntax(m) => f.write_str(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(&'static str),
}

const OPS: &[&str] =
    &["**", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "^", "(", ")", ",", "?", ":", "<", ">", "!"];

fn lex(s: &str) -> Result<Vec<Tok>, ExprError> {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            // Exponent: `e` followed by a digit or a signed digit.
            if i < b.len() && (b[i] | 0x20) == b'e' {
                let sign = matches!(b.get(i + 1), Some(b'+' | b'-'));
                let digit = b.get(i + 1 + sign as usize).is_some_and(u8::is_ascii_digit);
                if digit {
                    i += 1 + sign as usize;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            // Scale suffix and unit letters (`1u`, `2meg`, `5pF`).
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            let t = &s[start..i];
            out.push(Tok::Num(parse_spice(t).ok_or_else(|| ExprError::Syntax(format!("bad number '{t}'")))?));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            out.push(Tok::Ident(s[start..i].to_ascii_lowercase()));
        } else if let Some(op) = OPS.iter().find(|op| s[i..].starts_with(**op)) {
            out.push(Tok::Op(op));
            i += op.len();
        } else {
            return Err(ExprError::Syntax(format!("unexpected '{}' in '{s}'", s[i..].chars().next().unwrap_or(' '))));
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    params: &'a Params,
}

impl Parser<'_> {
    fn peek_op(&self, op: &str) -> bool {
        matches!(self.toks.get(self.pos), Some(Tok::Op(o)) if *o == op)
    }

    fn eat(&mut self, op: &str) -> bool {
        let hit = self.peek_op(op);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect(&mut self, op: &str) -> Result<(), ExprError> {
        if self.eat(op) { Ok(()) } else { Err(ExprError::Syntax(format!("expected '{op}'"))) }
    }

    fn ternary(&mut self) -> Result<f64, ExprError> {
        let c = self.or()?;
        if self.eat("?") {
            let a = self.ternary()?;
            self.expect(":")?;
            let b = self.ternary()?;
            return Ok(if c != 0.0 { a } else { b });
        }
        Ok(c)
    }

    fn or(&mut self) -> Result<f64, ExprError> {
        let mut v = self.and()?;
        while self.eat("||") {
            let r = self.and()?;
            v = ((v != 0.0) || (r != 0.0)) as u8 as f64;
        }
        Ok(v)
    }

    fn and(&mut self) -> Result<f64, ExprError> {
        let mut v = self.cmp()?;
        while self.eat("&&") {
            let r = self.cmp()?;
            v = ((v != 0.0) && (r != 0.0)) as u8 as f64;
        }
        Ok(v)
    }

    fn cmp(&mut self) -> Result<f64, ExprError> {
        let mut v = self.add()?;
        loop {
            let op = ["==", "!=", "<=", ">=", "<", ">"].into_iter().find(|o| self.peek_op(o));
            let Some(op) = op else { return Ok(v) };
            self.pos += 1;
            let r = self.add()?;
            let t = match op {
                "==" => v == r,
                "!=" => v != r,
                "<=" => v <= r,
                ">=" => v >= r,
                "<" => v < r,
                _ => v > r,
            };
            v = t as u8 as f64;
        }
    }

    fn add(&mut self) -> Result<f64, ExprError> {
        let mut v = self.mul()?;
        loop {
            if self.eat("+") {
                v += self.mul()?;
            } else if self.eat("-") {
                v -= self.mul()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn mul(&mut self) -> Result<f64, ExprError> {
        let mut v = self.unary()?;
        loop {
            if self.eat("*") {
                v *= self.unary()?;
            } else if self.eat("/") {
                v /= self.unary()?;
            } else if self.eat("%") {
                v %= self.unary()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn unary(&mut self) -> Result<f64, ExprError> {
        if self.eat("-") {
            return Ok(-self.unary()?);
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok((self.unary()? == 0.0) as u8 as f64);
        }
        self.pow()
    }

    fn pow(&mut self) -> Result<f64, ExprError> {
        let base = self.primary()?;
        if self.eat("^") || self.eat("**") {
            let e = self.unary()?;
            return Ok(base.powf(e));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<f64, ExprError> {
        match self.toks.get(self.pos).cloned() {
            Some(Tok::Num(v)) => {
                self.pos += 1;
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                if self.eat("(") {
                    let mut args = vec![];
                    if !self.eat(")") {
                        loop {
                            args.push(self.ternary()?);
                            if self.eat(")") {
                                break;
                            }
                            self.expect(",")?;
                        }
                    }
                    return call(&name, &args);
                }
                if let Some(v) = self.params.get(&name) {
                    return Ok(*v);
                }
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    _ => Err(ExprError::Unknown(name)),
                }
            }
            Some(Tok::Op("(")) => {
                self.pos += 1;
                let v = self.ternary()?;
                self.expect(")")?;
                Ok(v)
            }
            Some(Tok::Op(o)) => Err(ExprError::Syntax(format!("unexpected '{o}'"))),
            None => Err(ExprError::Syntax("unexpected end of expression".into())),
        }
    }
}

fn call(name: &str, a: &[f64]) -> Result<f64, ExprError> {
    let arity = |n: usize| -> Result<(), ExprError> {
        if a.len() < n { Err(ExprError::Syntax(format!("{name}() needs {n} argument(s)"))) } else { Ok(()) }
    };
    let one = |f: fn(f64) -> f64| -> Result<f64, ExprError> {
        arity(1)?;
        Ok(f(a[0]))
    };
    match name {
        "sqrt" => one(f64::sqrt),
        "exp" => one(f64::exp),
        "log" | "ln" => one(f64::ln),
        "log10" => one(f64::log10),
        "abs" => one(f64::abs),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "sinh" => one(f64::sinh),
        "cosh" => one(f64::cosh),
        "tanh" => one(f64::tanh),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "int" => one(f64::trunc),
        "nint" | "round" => one(f64::round),
        "sgn" | "sign" if a.len() == 1 => one(|x| {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }),
        "sign" => {
            arity(2)?;
            Ok(a[0].abs().copysign(a[1]))
        }
        "atan2" => {
            arity(2)?;
            Ok(a[0].atan2(a[1]))
        }
        "pow" | "pwr" => {
            arity(2)?;
            Ok(if name == "pwr" { a[0].abs().powf(a[1]).copysign(a[0]) } else { a[0].powf(a[1]) })
        }
        "min" | "max" => {
            arity(1)?;
            let f = if name == "min" { f64::min } else { f64::max };
            Ok(a[1..].iter().fold(a[0], |m, &x| f(m, x)))
        }
        "limit" => {
            arity(3)?;
            Ok(a[0].clamp(a[1].min(a[2]), a[1].max(a[2])))
        }
        "if" | "ternary_fcn" => {
            arity(3)?;
            Ok(if a[0] != 0.0 { a[1] } else { a[2] })
        }
        // Statistical distributions: the nominal value.
        "agauss" | "gauss" | "aunif" | "unif" => {
            arity(1)?;
            Ok(a[0])
        }
        _ => Err(ExprError::Syntax(format!("unknown function '{name}()'"))),
    }
}

/// Evaluate `s` with `params` in scope.
pub fn eval(s: &str, params: &Params) -> Result<f64, ExprError> {
    let toks = lex(s)?;
    let mut p = Parser { toks, pos: 0, params };
    let v = p.ternary()?;
    if p.pos != p.toks.len() {
        return Err(ExprError::Syntax(format!("unexpected text in '{s}'")));
    }
    Ok(v)
}

/// A number as written into a flattened deck: exact and readable by [`parse_spice`].
pub fn fmt_num(v: f64) -> String {
    // 12 significant digits hide rounding residue (`10u` read as 9.999999999999999e-6).
    let v: f64 = format!("{v:.11e}").parse().unwrap_or(v);
    if v == 0.0 || (v.abs() >= 1e-3 && v.abs() < 1e6) { format!("{v}") } else { format!("{v:e}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        let p: Params = [("wn".to_string(), 2e-6), ("vdd".to_string(), 1.8)].into();
        eval(s, &p).unwrap()
    }

    #[test]
    fn arithmetic_suffixes_and_precedence() {
        assert_eq!(ev("1+2*3"), 7.0);
        assert_eq!(ev("(1+2)*3"), 9.0);
        assert_eq!(ev("2^3^2"), 512.0);
        assert_eq!(ev("2**3"), 8.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert!((ev("wn*2") - 4e-6).abs() < 1e-18);
        assert!((ev("1u + 10n") - 1.01e-6).abs() < 1e-18);
        assert_eq!(ev("2meg"), 2e6);
        assert_eq!(ev("1e-3*1k"), 1.0);
        assert_eq!(ev("1.5e+2"), 150.0);
        assert_eq!(ev("VDD/2"), 0.9);
    }

    #[test]
    fn functions_and_logic() {
        assert_eq!(ev("max(1, 3, 2)"), 3.0);
        assert_eq!(ev("min(4,2)"), 2.0);
        assert_eq!(ev("sqrt(16)"), 4.0);
        assert_eq!(ev("vdd > 1 ? 5 : 6"), 5.0);
        assert_eq!(ev("if(vdd < 1, 5, 6)"), 6.0);
        assert_eq!(ev("1 && 0 || 1"), 1.0);
        assert_eq!(ev("agauss(0.4, 0.01, 3)"), 0.4);
        assert_eq!(ev("abs(-pi) == pi"), 1.0);
    }

    #[test]
    fn errors_name_the_problem() {
        let p = Params::new();
        assert_eq!(eval("x+1", &p), Err(ExprError::Unknown("x".into())));
        assert!(matches!(eval("1+", &p), Err(ExprError::Syntax(_))));
        assert!(matches!(eval("foo(1)", &p), Err(ExprError::Syntax(_))));
        assert!(matches!(eval("1 2", &p), Err(ExprError::Syntax(_))));
    }

    #[test]
    fn formatted_numbers_round_trip() {
        assert_eq!(fmt_num(parse_spice("10u").unwrap()), "1e-5");
        for v in [0.0, 1.0, -1.8, 1e-6, 2.5e-15, 1.234e9, 0.15e-6, 1e-3] {
            assert_eq!(parse_spice(&fmt_num(v)), Some(v), "{v} -> {}", fmt_num(v));
        }
    }
}
