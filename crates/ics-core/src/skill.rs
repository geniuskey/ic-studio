//! Cadence SKILL (`.il`) interpreter for layout scripting.
//!
//! The language subset covers what layout generator scripts use:
//! - syntax: C style calls `f(a b)`, Lisp forms `(f a b)`, infix operators (`+ - * / ** == != < > <= >=
//!   && || ! = ++ --`), points `x:y`, `obj~>prop` / `obj->prop`, `tbl[key]`, quoting `'sym` and
//!   `'(1 2 3)`, `?key value` arguments, `{ ... }` blocks, `;` and `/* */` comments, numbers with
//!   scale factors (`1.5u`);
//! - control: `if` (with or without `then` / `else`), `when`, `unless`, `cond`, `case`, `caseq`,
//!   `while`, `for`, `foreach` (also `foreach(mapcar ...)` and tables), `let`, `prog` / `return`,
//!   `progn`, `and`, `or`, `setof`, `exists`, `forall`, `errset`, `error`;
//! - definitions: `procedure` and `defun` with `@optional`, `@key` and `@rest`, `lambda`, `funcall`,
//!   `apply`, `mapcar`, `mapc`; variables are dynamically scoped as in SKILL (`.il`);
//! - data: numbers, strings, symbols, lists, disembodied property lists, `makeTable` tables;
//! - layout database (`db*`): `dbOpenCellViewByType` (`"r"`, `"w"` clears, `"a"` appends; libraries
//!   and cells are created on demand), `dbCreateRect`, `dbCreatePolygon`, `dbCreatePath`,
//!   `dbCreateLabel`, `dbCreateInst`, `dbCreateInstByMasterName`, `dbCreateSimpleMosaic`,
//!   `dbDeleteObject`, `dbMoveFig`, `dbCopyFig`, `dbFindAnyInstByName`, `dbSave` / `dbClose`
//!   (no-ops: the caller saves the project) and `geGetEditCellView` for the cell open in the editor.
//!
//! Coordinates are user units (µm), converted to database units with the technology's
//! `dbu_per_um`. A layer-purpose pair is a layer name, `list("metal1" "drawing")` or a layer index;
//! a purpose other than `drawing` first looks for a layer named `metal1.pin` / `metal1_pin`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::geom::IRect;
use crate::layout::{ArraySpec, CellInst, Geometry, Layout, Trans};
use crate::project::{Cell, CellRef, Project};

/// Result of a SKILL run.
#[derive(Debug, Clone, Default)]
pub struct RunResult {
    /// Everything the script printed (`printf`, `println`, `print`, ...).
    pub output: String,
    /// The value of the last top-level form, printed SKILL style.
    pub value: String,
    /// Cells whose layout the script created or changed, in first-touched order.
    pub modified: Vec<CellRef>,
}

/// A SKILL error with the position it happened at.
#[derive(Debug, Clone)]
pub struct SkillError {
    pub message: String,
    /// File the error happened in (`None` for inline code).
    pub file: Option<String>,
    /// 1-based line.
    pub line: usize,
    /// Output printed before the error.
    pub output: String,
    /// Cells changed before the error (their edits are kept).
    pub modified: Vec<CellRef>,
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.file {
            Some(file) => write!(f, "{file}:{}: {}", self.line, self.message),
            None => write!(f, "line {}: {}", self.line, self.message),
        }
    }
}

impl std::error::Error for SkillError {}

/// Run SKILL source against `project`. `file` names the source for errors and is the base
/// directory of relative `load()` paths.
pub fn run(project: &mut Project, src: &str, file: Option<&Path>) -> Result<RunResult, SkillError> {
    run_with(project, src, file, None)
}

/// [`run`] with `current` as the cell open in the editor (`geGetEditCellView()`).
pub fn run_with(
    project: &mut Project,
    src: &str,
    file: Option<&Path>,
    current: Option<&CellRef>,
) -> Result<RunResult, SkillError> {
    Session::new().eval(project, src, file, current)
}

/// An interpreter whose global variables, procedures and open cellviews persist between
/// evaluations, like Virtuoso's CIW.
#[derive(Default)]
pub struct Session {
    globals: HashMap<Name, Value>,
    funcs: HashMap<Name, Rc<Func>>,
    cvs: Vec<CvState>,
    files: Vec<Option<PathBuf>>,
}

impl Session {
    pub fn new() -> Self {
        let globals = ["poport", "errport", "woport", "piport"].into_iter().map(|p| (p.into(), Value::Port)).collect();
        Session { globals, ..Default::default() }
    }

    /// Evaluate `src`; `file` names it for errors and is the base directory of relative `load()`
    /// paths, `current` is the cell `geGetEditCellView()` returns.
    pub fn eval(
        &mut self,
        project: &mut Project,
        src: &str,
        file: Option<&Path>,
        current: Option<&CellRef>,
    ) -> Result<RunResult, SkillError> {
        let mut it = Interp::new(project, current.cloned(), std::mem::take(self));
        let r = it.run_source(src, file.map(Path::to_path_buf));
        let modified = std::mem::take(&mut it.modified);
        let output = std::mem::take(&mut it.out);
        let res = match r {
            Ok(v) | Err(Flow::Return(v)) => Ok(RunResult { output, value: it.repr(&v), modified }),
            Err(Flow::Err(e)) => Err(SkillError {
                message: e.msg,
                file: e.pos.and_then(|p| it.files.get(p.file as usize).cloned().flatten()).map(|p| p.display().to_string()),
                line: e.pos.map_or(0, |p| p.line as usize),
                output,
                modified,
            }),
        };
        *self = it.into_session();
        res
    }

    /// Read and evaluate a `.il` file.
    pub fn load(&mut self, project: &mut Project, path: &Path, current: Option<&CellRef>) -> Result<RunResult, SkillError> {
        let src = std::fs::read_to_string(path).map_err(|e| SkillError {
            message: e.to_string(),
            file: Some(path.display().to_string()),
            line: 0,
            output: String::new(),
            modified: vec![],
        })?;
        self.eval(project, &src, Some(path), current)
    }

    /// Names of the procedures defined so far, sorted.
    pub fn procedures(&self) -> Vec<String> {
        let mut v: Vec<String> = self.funcs.keys().map(|k| k.to_string()).collect();
        v.sort();
        v
    }
}

/// Whether `src` is a complete input: no unclosed parenthesis, brace, bracket, string or comment.
/// Other syntax errors count as complete, so evaluating reports them.
pub fn is_complete(src: &str) -> bool {
    match lex(src, 0) {
        Ok(toks) => {
            let mut depth = 0i64;
            for (t, _) in &toks {
                match t {
                    Tok::LParen | Tok::CallStart(_) | Tok::LBrace | Tok::LBracket => depth += 1,
                    Tok::RParen | Tok::RBrace | Tok::RBracket => depth -= 1,
                    _ => {}
                }
            }
            depth <= 0
        }
        Err(Flow::Err(e)) => !e.msg.starts_with("unterminated"),
        Err(_) => true,
    }
}

/// Read and run a `.il` file.
pub fn run_file(project: &mut Project, path: &Path) -> Result<RunResult, SkillError> {
    run_file_with(project, path, None)
}

/// [`run_file`] with `current` as the cell open in the editor.
pub fn run_file_with(project: &mut Project, path: &Path, current: Option<&CellRef>) -> Result<RunResult, SkillError> {
    let src = std::fs::read_to_string(path).map_err(|e| SkillError {
        message: e.to_string(),
        file: Some(path.display().to_string()),
        line: 0,
        output: String::new(),
        modified: vec![],
    })?;
    run_with(project, &src, Some(path), current)
}

// ----------------------------------------------------------------------------------------------
// Values

type Name = Rc<str>;

#[derive(Debug, Clone)]
enum Value {
    Nil,
    T,
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Sym(Name),
    /// Never empty: the empty list is `Nil`.
    List(Rc<Vec<Value>>),
    Func(Rc<Func>),
    Obj(Obj),
    Table(Rc<RefCell<Table>>),
    Port,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Obj {
    Cv(usize),
    Shape(usize, u64),
    Inst(usize, u64),
}

#[derive(Debug, Default)]
struct Table {
    name: String,
    default: Option<Value>,
    entries: Vec<(Value, Value)>,
}

#[derive(Debug)]
struct Func {
    name: Name,
    params: Params,
    body: Vec<Node>,
}

#[derive(Debug, Default)]
struct Params {
    req: Vec<Name>,
    opt: Vec<(Name, Option<Node>)>,
    key: Vec<(Name, Option<Node>)>,
    rest: Option<Name>,
}

fn list(v: Vec<Value>) -> Value {
    if v.is_empty() { Value::Nil } else { Value::List(Rc::new(v)) }
}

fn sym(s: &str) -> Value {
    match s {
        "nil" => Value::Nil,
        "t" => Value::T,
        _ => Value::Sym(s.into()),
    }
}

fn string(s: impl Into<Rc<str>>) -> Value {
    Value::Str(s.into())
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil)
}

fn boolv(b: bool) -> Value {
    if b { Value::T } else { Value::Nil }
}

fn items(v: &Value) -> &[Value] {
    match v {
        Value::List(l) => l,
        _ => &[],
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) | (Value::T, Value::T) | (Value::Port, Value::Port) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => as_f64(a) == as_f64(b),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Sym(x), Value::Sym(y)) => x == y,
        (Value::List(x), Value::List(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| equal(a, b)),
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
        (Value::Obj(x), Value::Obj(y)) => x == y,
        (Value::Table(x), Value::Table(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::List(x), Value::List(y)) => Rc::ptr_eq(x, y),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y) || x == y,
        _ => equal(a, b),
    }
}

fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        _ => f64::NAN,
    }
}

fn fmt_float(f: f64) -> String {
    if !f.is_finite() {
        format!("{f}")
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else if f != 0.0 && (f.abs() < 1e-4 || f.abs() >= 1e15) {
        c_exponent(&format!("{f:e}"))
    } else {
        format!("{f}")
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil | Value::List(_) => "list",
        Value::T | Value::Sym(_) => "symbol",
        Value::Int(_) => "fixnum",
        Value::Float(_) => "flonum",
        Value::Str(_) => "string",
        Value::Func(_) => "funobj",
        Value::Obj(_) => "dbobject",
        Value::Table(_) => "assocTable",
        Value::Port => "port",
    }
}

// ----------------------------------------------------------------------------------------------
// Errors and control flow

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pos {
    file: u32,
    line: u32,
}

#[derive(Debug)]
struct ErrInfo {
    msg: String,
    pos: Option<Pos>,
}

#[derive(Debug)]
enum Flow {
    Err(ErrInfo),
    Return(Value),
}

type R<T> = Result<T, Flow>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(Flow::Err(ErrInfo { msg: msg.into(), pos: None }))
}

fn at(pos: Pos) -> impl Fn(Flow) -> Flow {
    move |f| match f {
        Flow::Err(ErrInfo { msg, pos: None }) => Flow::Err(ErrInfo { msg, pos: Some(pos) }),
        f => f,
    }
}

// ----------------------------------------------------------------------------------------------
// Lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64, bool),
    Str(String),
    Ident(String),
    /// Identifier directly followed by `(`.
    CallStart(String),
    Key(String),
    At(String),
    Op(&'static str),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Quote,
    Eof,
}

const OPS: [&str; 26] = [
    "~>", "->", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "**", "<<", ">>", "+", "-", "*", "/", "<", ">", "=", "!", ":",
    "&", "|", "^", "~",
];

fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '?'
}

fn lex(src: &str, file: u32) -> R<Vec<(Tok, Pos)>> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = vec![];
    let (mut i, mut line) = (0, 1u32);
    let pos = |line| Pos { file, line };
    let err = |line: u32, msg: String| Flow::Err(ErrInfo { msg, pos: Some(Pos { file, line }) });
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == ';' {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && cs.get(i + 1) == Some(&'*') {
            let start = line;
            i += 2;
            loop {
                match cs.get(i) {
                    None => return Err(err(start, "unterminated /* comment".into())),
                    Some('*') if cs.get(i + 1) == Some(&'/') => {
                        i += 2;
                        break;
                    }
                    Some('\n') => line += 1,
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        let p = pos(line);
        if c == '"' {
            let start = line;
            let mut s = String::new();
            i += 1;
            loop {
                match cs.get(i) {
                    None => return Err(err(start, "unterminated string".into())),
                    Some('"') => break,
                    Some('\\') => {
                        i += 1;
                        match cs.get(i) {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('r') => s.push('\r'),
                            Some('\n') => line += 1,
                            Some(&c) => s.push(c),
                            None => return Err(err(start, "unterminated string".into())),
                        }
                    }
                    Some(&c) => {
                        if c == '\n' {
                            line += 1;
                        }
                        s.push(c);
                    }
                }
                i += 1;
            }
            i += 1;
            out.push((Tok::Str(s), p));
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            if c == '0' && matches!(cs.get(i + 1), Some('x' | 'X')) {
                i += 2;
                while i < cs.len() && cs[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let s: String = cs[start + 2..i].iter().collect();
                let v = i64::from_str_radix(&s, 16).map_err(|_| err(line, format!("bad number 0x{s}")))?;
                out.push((Tok::Num(v as f64, true), p));
                continue;
            }
            let mut float = false;
            while i < cs.len() && cs[i].is_ascii_digit() {
                i += 1;
            }
            if cs.get(i) == Some(&'.') && cs.get(i + 1).is_none_or(|c| !ident_start(*c) || matches!(c, 'e' | 'E')) {
                float = true;
                i += 1;
                while i < cs.len() && cs[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if matches!(cs.get(i), Some('e' | 'E'))
                && (cs.get(i + 1).is_some_and(char::is_ascii_digit)
                    || (matches!(cs.get(i + 1), Some('+' | '-')) && cs.get(i + 2).is_some_and(char::is_ascii_digit)))
            {
                float = true;
                i += 2;
                while i < cs.len() && cs[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s: String = cs[start..i].iter().collect();
            let mut v: f64 = s.parse().map_err(|_| err(line, format!("bad number {s}")))?;
            let scale = match cs.get(i) {
                Some('T') => 1e12,
                Some('G') => 1e9,
                Some('M') => 1e6,
                Some('K' | 'k') => 1e3,
                Some('m') => 1e-3,
                Some('u') => 1e-6,
                Some('n') => 1e-9,
                Some('p') => 1e-12,
                Some('f') => 1e-15,
                Some('a') => 1e-18,
                _ => 0.0,
            };
            if scale != 0.0 && cs.get(i + 1).is_none_or(|c| !ident_char(*c)) {
                v *= scale;
                float = true;
                i += 1;
            }
            out.push((Tok::Num(v, !float), p));
            continue;
        }
        if ident_start(c) || (c == '\\' && cs.get(i + 1).is_some()) {
            let mut s = String::new();
            while i < cs.len() && (ident_char(cs[i]) || cs[i] == '\\') {
                if cs[i] == '\\' {
                    i += 1;
                    if let Some(&c) = cs.get(i) {
                        s.push(c);
                    }
                } else {
                    s.push(cs[i]);
                }
                i += 1;
            }
            if cs.get(i) == Some(&'(') {
                i += 1;
                out.push((Tok::CallStart(s), p));
            } else {
                out.push((Tok::Ident(s), p));
            }
            continue;
        }
        if (c == '?' || c == '@') && cs.get(i + 1).is_some_and(|c| ident_start(*c)) {
            i += 1;
            let start = i;
            while i < cs.len() && ident_char(cs[i]) {
                i += 1;
            }
            let s: String = cs[start..i].iter().collect();
            out.push((if c == '?' { Tok::Key(s) } else { Tok::At(s) }, p));
            continue;
        }
        let single = match c {
            '(' => Some(Tok::LParen),
            ')' => Some(Tok::RParen),
            '{' => Some(Tok::LBrace),
            '}' => Some(Tok::RBrace),
            '[' => Some(Tok::LBracket),
            ']' => Some(Tok::RBracket),
            '\'' => Some(Tok::Quote),
            _ => None,
        };
        if let Some(t) = single {
            out.push((t, p));
            i += 1;
            continue;
        }
        let rest: String = cs[i..(i + 2).min(cs.len())].iter().collect();
        match OPS.iter().find(|op| rest.starts_with(**op)) {
            Some(op) => {
                i += op.len();
                out.push((Tok::Op(op), p));
            }
            None => return Err(err(line, format!("unexpected character '{c}'"))),
        }
    }
    out.push((Tok::Eof, pos(line)));
    Ok(out)
}

// ----------------------------------------------------------------------------------------------
// Parser

#[derive(Debug, Clone)]
enum Node {
    Lit(Value),
    Var(Name),
    /// Named call: C style `f(a b)`, operators and built-in forms.
    Call(Name, Vec<Node>, Pos),
    /// Parenthesised Lisp form `(f a b)`; `(expr)` groups.
    Form(Vec<Node>, Pos),
    Key(Name),
    At(Name),
    Get(Box<Node>, Name, Pos),
    Index(Box<Node>, Box<Node>, Pos),
}

struct Parser {
    toks: Vec<(Tok, Pos)>,
    i: usize,
}

fn binary(op: &str) -> Option<(u8, &'static str)> {
    Some(match op {
        "=" => (1, "setq"),
        ":" => (2, "range"),
        "||" => (3, "or"),
        "&&" => (4, "and"),
        "|" => (5, "bitor"),
        "^" => (6, "bitxor"),
        "&" => (7, "bitand"),
        "==" => (8, "equal"),
        "!=" => (8, "nequal"),
        "<" => (9, "lessp"),
        ">" => (9, "greaterp"),
        "<=" => (9, "leqp"),
        ">=" => (9, "geqp"),
        "<<" => (10, "leftshift"),
        ">>" => (10, "rightshift"),
        "+" => (11, "plus"),
        "-" => (11, "difference"),
        "*" => (12, "times"),
        "/" => (12, "quotient"),
        "**" => (13, "expt"),
        _ => return None,
    })
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].0
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].1
    }

    fn next(&mut self) -> (Tok, Pos) {
        let t = self.toks[self.i].clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(Flow::Err(ErrInfo { msg: msg.into(), pos: Some(self.pos()) }))
    }

    fn program(&mut self) -> R<Vec<Node>> {
        let mut out = vec![];
        while *self.peek() != Tok::Eof {
            if matches!(self.peek(), Tok::RParen | Tok::RBrace | Tok::RBracket) {
                return self.err(format!("unbalanced {:?}", self.peek()));
            }
            out.push(self.expr(0)?);
        }
        Ok(out)
    }

    fn seq(&mut self, close: Tok) -> R<Vec<Node>> {
        let mut out = vec![];
        loop {
            let t = self.peek().clone();
            if t == close {
                self.next();
                return Ok(out);
            }
            match t {
                Tok::Eof => return self.err("unexpected end of input (missing closing parenthesis?)"),
                Tok::RParen | Tok::RBrace | Tok::RBracket => return self.err(format!("unexpected {t:?}")),
                _ => out.push(self.expr(0)?),
            }
        }
    }

    fn expr(&mut self, min: u8) -> R<Node> {
        let mut lhs = self.unary()?;
        while let Tok::Op(op) = *self.peek() {
            let Some((prec, name)) = binary(op) else { break };
            if prec < min {
                break;
            }
            let pos = self.pos();
            self.next();
            let right_assoc = matches!(op, "=" | "**");
            let rhs = self.expr(if right_assoc { prec } else { prec + 1 })?;
            lhs = Node::Call(name.into(), vec![lhs, rhs], pos);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> R<Node> {
        let pos = self.pos();
        match *self.peek() {
            Tok::Op("-") => {
                self.next();
                let e = self.unary()?;
                Ok(match e {
                    Node::Lit(Value::Int(i)) => Node::Lit(Value::Int(-i)),
                    Node::Lit(Value::Float(f)) => Node::Lit(Value::Float(-f)),
                    e => Node::Call("minus".into(), vec![e], pos),
                })
            }
            Tok::Op("+") => {
                self.next();
                self.unary()
            }
            Tok::Op("!") => {
                self.next();
                Ok(Node::Call("null".into(), vec![self.unary()?], pos))
            }
            Tok::Op("~") => {
                self.next();
                Ok(Node::Call("bitnot".into(), vec![self.unary()?], pos))
            }
            Tok::Op(op @ ("++" | "--")) => {
                self.next();
                let f = if op == "++" { "preincrement" } else { "predecrement" };
                Ok(Node::Call(f.into(), vec![self.unary()?], pos))
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> R<Node> {
        let mut e = self.primary()?;
        loop {
            let pos = self.pos();
            match self.peek().clone() {
                Tok::Op("~>") | Tok::Op("->") => {
                    self.next();
                    match self.next().0 {
                        Tok::Ident(p) => e = Node::Get(Box::new(e), p.into(), pos),
                        Tok::Key(p) if p.is_empty() => e = Node::Get(Box::new(e), "?".into(), pos),
                        t => {
                            return Err(Flow::Err(ErrInfo {
                                msg: format!("property name expected after ~>, got {t:?}"),
                                pos: Some(pos),
                            }));
                        }
                    }
                }
                Tok::LBracket => {
                    self.next();
                    let idx = self.expr(0)?;
                    if self.next().0 != Tok::RBracket {
                        return self.err("']' expected");
                    }
                    e = Node::Index(Box::new(e), Box::new(idx), pos);
                }
                Tok::Op(op @ ("++" | "--")) if matches!(e, Node::Var(_)) => {
                    self.next();
                    let f = if op == "++" { "postincrement" } else { "postdecrement" };
                    e = Node::Call(f.into(), vec![e], pos);
                }
                _ => return Ok(e),
            }
        }
    }

    fn primary(&mut self) -> R<Node> {
        let (t, pos) = self.next();
        Ok(match t {
            Tok::Num(v, true) => Node::Lit(Value::Int(v as i64)),
            Tok::Num(v, false) => Node::Lit(Value::Float(v)),
            Tok::Str(s) => Node::Lit(string(s)),
            Tok::Ident(s) => match s.as_str() {
                "nil" => Node::Lit(Value::Nil),
                "t" => Node::Lit(Value::T),
                _ => Node::Var(s.into()),
            },
            Tok::CallStart(f) => Node::Call(f.into(), self.seq(Tok::RParen)?, pos),
            Tok::LParen => Node::Form(self.seq(Tok::RParen)?, pos),
            Tok::LBrace => Node::Call("progn".into(), self.seq(Tok::RBrace)?, pos),
            Tok::Key(k) => Node::Key(k.into()),
            Tok::At(a) => Node::At(a.into()),
            Tok::Quote => Node::Lit(self.datum()?),
            t => return Err(Flow::Err(ErrInfo { msg: format!("unexpected {t:?}"), pos: Some(pos) })),
        })
    }

    fn datum(&mut self) -> R<Value> {
        let (t, pos) = self.next();
        Ok(match t {
            Tok::Num(v, true) => Value::Int(v as i64),
            Tok::Num(v, false) => Value::Float(v),
            Tok::Str(s) => string(s),
            Tok::Ident(s) => sym(&s),
            Tok::Key(s) => sym(&format!("?{s}")),
            Tok::At(s) => sym(&format!("@{s}")),
            Tok::Op("-") if matches!(self.peek(), Tok::Num(..)) => match self.next().0 {
                Tok::Num(v, true) => Value::Int(-(v as i64)),
                Tok::Num(v, _) => Value::Float(-v),
                _ => unreachable!(),
            },
            Tok::Op(op) => sym(op),
            Tok::Quote => list(vec![sym("quote"), self.datum()?]),
            Tok::CallStart(f) => {
                let mut v = vec![sym(&f)];
                v.extend(self.datum_list(Tok::RParen)?);
                list(v)
            }
            Tok::LParen => list(self.datum_list(Tok::RParen)?),
            Tok::LBrace => list(self.datum_list(Tok::RBrace)?),
            t => return Err(Flow::Err(ErrInfo { msg: format!("unexpected {t:?} in quoted data"), pos: Some(pos) })),
        })
    }

    fn datum_list(&mut self, close: Tok) -> R<Vec<Value>> {
        let mut out = vec![];
        loop {
            if *self.peek() == close {
                self.next();
                return Ok(out);
            }
            if *self.peek() == Tok::Eof {
                return self.err("unexpected end of input in quoted list");
            }
            out.push(self.datum()?);
        }
    }
}

fn parse(src: &str, file: u32) -> R<Vec<Node>> {
    Parser { toks: lex(src, file)?, i: 0 }.program()
}

/// The elements of a list-like node, for forms such as `let((a (b 1)) ...)`.
fn elems(n: &Node) -> Vec<Node> {
    match n {
        Node::Form(v, _) => v.clone(),
        Node::Call(f, args, _) => {
            let mut v = vec![Node::Var(f.clone())];
            v.extend(args.iter().cloned());
            v
        }
        Node::Lit(Value::Nil) => vec![],
        n => vec![n.clone()],
    }
}

fn node_to_datum(n: &Node) -> Value {
    match n {
        Node::Lit(v) => v.clone(),
        Node::Var(s) => Value::Sym(s.clone()),
        Node::Key(k) => sym(&format!("?{k}")),
        Node::At(a) => sym(&format!("@{a}")),
        n => list(elems(n).iter().map(node_to_datum).collect()),
    }
}

fn datum_to_node(v: &Value) -> Node {
    let pos = Pos { file: 0, line: 0 };
    match v {
        Value::Sym(s) => Node::Var(s.clone()),
        Value::List(l) => match &l[0] {
            Value::Sym(q) if &**q == "quote" && l.len() == 2 => Node::Lit(l[1].clone()),
            Value::Sym(f) => Node::Call(f.clone(), l[1..].iter().map(datum_to_node).collect(), pos),
            _ => Node::Form(l.iter().map(datum_to_node).collect(), pos),
        },
        v => Node::Lit(v.clone()),
    }
}

// ----------------------------------------------------------------------------------------------
// Interpreter

const MAX_DEPTH: usize = 400;
const MAX_STEPS: u64 = 200_000_000;

struct CvState {
    cell: CellRef,
    writable: bool,
}

struct Interp<'p> {
    p: &'p mut Project,
    globals: HashMap<Name, Value>,
    frames: Vec<HashMap<Name, Value>>,
    funcs: HashMap<Name, Rc<Func>>,
    out: String,
    cvs: Vec<CvState>,
    modified: Vec<CellRef>,
    files: Vec<Option<PathBuf>>,
    load_stack: Vec<Option<PathBuf>>,
    depth: usize,
    steps: u64,
    current: Option<CellRef>,
}

struct Args {
    pos: Vec<Value>,
    keys: Vec<(Name, Value)>,
}

impl<'p> Interp<'p> {
    fn new(p: &'p mut Project, current: Option<CellRef>, s: Session) -> Self {
        Interp {
            p,
            globals: s.globals,
            frames: vec![],
            funcs: s.funcs,
            out: String::new(),
            cvs: s.cvs,
            modified: vec![],
            files: s.files,
            load_stack: vec![],
            depth: 0,
            steps: 0,
            current,
        }
    }

    fn into_session(self) -> Session {
        Session { globals: self.globals, funcs: self.funcs, cvs: self.cvs, files: self.files }
    }

    fn run_source(&mut self, src: &str, file: Option<PathBuf>) -> R<Value> {
        if self.files.len() >= u32::MAX as usize {
            return fail("too many loaded files");
        }
        let idx = self.files.len() as u32;
        self.files.push(file.clone());
        let nodes = parse(src, idx)?;
        self.load_stack.push(file);
        let mut last = Value::Nil;
        let mut r = Ok(());
        for n in &nodes {
            match self.eval(n) {
                Ok(v) => last = v,
                Err(e) => {
                    r = Err(e);
                    break;
                }
            }
        }
        self.load_stack.pop();
        r.map(|_| last)
    }

    // ----- variables -----

    fn lookup(&self, name: &str) -> R<Value> {
        for f in self.frames.iter().rev() {
            if let Some(v) = f.get(name) {
                return Ok(v.clone());
            }
        }
        match self.globals.get(name) {
            Some(v) => Ok(v.clone()),
            None => fail(format!("unbound variable {name}")),
        }
    }

    fn assign(&mut self, name: &Name, v: Value) {
        for f in self.frames.iter_mut().rev() {
            if let Some(slot) = f.get_mut(name) {
                *slot = v;
                return;
            }
        }
        self.globals.insert(name.clone(), v);
    }

    fn with_frame<T>(&mut self, frame: HashMap<Name, Value>, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        self.frames.push(frame);
        let r = f(self);
        self.frames.pop();
        r
    }

    fn body(&mut self, nodes: &[Node]) -> R<Value> {
        let mut last = Value::Nil;
        for n in nodes {
            last = self.eval(n)?;
        }
        Ok(last)
    }

    // ----- evaluation -----

    fn eval(&mut self, n: &Node) -> R<Value> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return fail("evaluation step limit exceeded (infinite loop?)");
        }
        match n {
            Node::Lit(v) => Ok(v.clone()),
            Node::Var(s) => self.lookup(s),
            Node::Call(f, args, pos) => self.call_named(f, args).map_err(at(*pos)),
            Node::Form(items, pos) => match items.as_slice() {
                [] => Ok(Value::Nil),
                [Node::Var(f), args @ ..] => self.call_named(f, args).map_err(at(*pos)),
                [single] => self.eval(single),
                [head, args @ ..] => {
                    let f = self.eval(head).map_err(at(*pos))?;
                    let a = self.eval_args(args).map_err(at(*pos))?;
                    self.call_value(&f, a).map_err(at(*pos))
                }
            },
            Node::Key(k) => fail(format!("unexpected keyword argument ?{k}")),
            Node::At(a) => fail(format!("unexpected @{a}")),
            Node::Get(obj, prop, pos) => {
                let o = self.eval(obj).map_err(at(*pos))?;
                self.get_prop(&o, prop).map_err(at(*pos))
            }
            Node::Index(obj, idx, pos) => {
                let o = self.eval(obj).map_err(at(*pos))?;
                let k = self.eval(idx).map_err(at(*pos))?;
                self.index(&o, &k).map_err(at(*pos))
            }
        }
    }

    fn eval_args(&mut self, args: &[Node]) -> R<Args> {
        let mut a = Args { pos: Vec::with_capacity(args.len()), keys: vec![] };
        let mut i = 0;
        while i < args.len() {
            if let Node::Key(k) = &args[i] {
                let Some(v) = args.get(i + 1) else { return fail(format!("missing value for ?{k}")) };
                let v = self.eval(v)?;
                a.keys.push((k.clone(), v));
                i += 2;
            } else {
                a.pos.push(self.eval(&args[i])?);
                i += 1;
            }
        }
        Ok(a)
    }

    fn call_named(&mut self, f: &Name, args: &[Node]) -> R<Value> {
        if let Some(r) = self.special(f, args) {
            return r;
        }
        if let Some(func) = self.funcs.get(f).cloned() {
            let a = self.eval_args(args)?;
            return self.apply(&func, a);
        }
        let a = self.eval_args(args)?;
        self.builtin(f, a)
    }

    fn call_value(&mut self, f: &Value, a: Args) -> R<Value> {
        match f {
            Value::Func(func) => self.apply(func, a),
            Value::Sym(name) => match self.funcs.get(name).cloned() {
                Some(func) => self.apply(&func, a),
                None => self.builtin(name, a),
            },
            Value::Str(name) => self.call_value(&Value::Sym(name.clone()), a),
            v => fail(format!("{} is not a function", self.repr(v))),
        }
    }

    fn call1(&mut self, f: &Value, args: Vec<Value>) -> R<Value> {
        self.call_value(f, Args { pos: args, keys: vec![] })
    }

    fn apply(&mut self, func: &Rc<Func>, a: Args) -> R<Value> {
        let ps = &func.params;
        if a.pos.len() < ps.req.len() {
            return fail(format!("{}: too few arguments ({} given, {} required)", func.name, a.pos.len(), ps.req.len()));
        }
        if ps.rest.is_none() && a.pos.len() > ps.req.len() + ps.opt.len() {
            return fail(format!("{}: too many arguments ({} given)", func.name, a.pos.len()));
        }
        if self.depth >= MAX_DEPTH {
            return fail(format!("{}: call depth limit exceeded (infinite recursion?)", func.name));
        }
        let mut frame = HashMap::new();
        let mut it = a.pos.into_iter();
        for p in &ps.req {
            frame.insert(p.clone(), it.next().unwrap_or(Value::Nil));
        }
        let mut defaults = vec![];
        for (p, d) in &ps.opt {
            match it.next() {
                Some(v) => {
                    frame.insert(p.clone(), v);
                }
                None => defaults.push((p.clone(), d.clone())),
            }
        }
        if let Some(r) = &ps.rest {
            frame.insert(r.clone(), list(it.collect()));
        }
        for (k, v) in a.keys {
            if !ps.key.iter().any(|(p, _)| *p == k) {
                return fail(format!("{}: unknown keyword argument ?{k}", func.name));
            }
            frame.insert(k, v);
        }
        for (p, d) in &ps.key {
            if !frame.contains_key(p) {
                defaults.push((p.clone(), d.clone()));
            }
        }
        self.depth += 1;
        let r = self.with_frame(frame, |s| {
            for (p, d) in defaults {
                let v = match d {
                    Some(n) => s.eval(&n)?,
                    None => Value::Nil,
                };
                s.frames.last_mut().unwrap().insert(p, v);
            }
            s.body(&func.body)
        });
        self.depth -= 1;
        match r {
            Err(Flow::Return(v)) => Ok(v),
            r => r,
        }
    }

    fn params(&self, nodes: &[Node]) -> R<Params> {
        let mut ps = Params::default();
        let mut mode = "";
        for n in nodes {
            match n {
                Node::At(m) => match &**m {
                    "optional" | "key" | "rest" | "aux" => mode = if &**m == "aux" { "optional" } else { m },
                    _ => return fail(format!("unknown parameter marker @{m}")),
                },
                Node::Var(p) => match mode {
                    "" => ps.req.push(p.clone()),
                    "optional" => ps.opt.push((p.clone(), None)),
                    "key" => ps.key.push((p.clone(), None)),
                    _ => ps.rest = Some(p.clone()),
                },
                n => {
                    let e = elems(n);
                    let (Some(Node::Var(p)), d) = (e.first(), e.get(1).cloned()) else {
                        return fail("bad parameter");
                    };
                    match mode {
                        "key" => ps.key.push((p.clone(), d)),
                        "optional" => ps.opt.push((p.clone(), d)),
                        _ => return fail(format!("parameter {p} with a default needs @optional or @key")),
                    }
                }
            }
        }
        Ok(ps)
    }

    fn define(&mut self, sig: &Node, body: &[Node]) -> R<Value> {
        let e = elems(sig);
        let Some(Node::Var(name)) = e.first() else { return fail("procedure name expected") };
        let params = self.params(&e[1..])?;
        let f = Rc::new(Func { name: name.clone(), params, body: body.to_vec() });
        self.funcs.insert(name.clone(), f);
        Ok(Value::Sym(name.clone()))
    }

    fn bind_list(&mut self, n: &Node) -> R<HashMap<Name, Value>> {
        let mut frame = HashMap::new();
        for b in elems(n) {
            match b {
                Node::Var(v) => {
                    frame.insert(v, Value::Nil);
                }
                b => {
                    let e = elems(&b);
                    match (e.first(), e.get(1)) {
                        (Some(Node::Var(v)), init) => {
                            let x = match init {
                                Some(i) => self.eval(i)?,
                                None => Value::Nil,
                            };
                            frame.insert(v.clone(), x);
                        }
                        _ => return fail("bad variable binding"),
                    }
                }
            }
        }
        Ok(frame)
    }

    fn set_target(&mut self, target: &Node, v: Value) -> R<Value> {
        match target {
            Node::Var(name) => self.assign(name, v.clone()),
            Node::Get(obj, prop, _) => {
                let o = self.eval(obj)?;
                match o {
                    Value::Nil | Value::List(_) if items(&o).first().is_none_or(|x| matches!(x, Value::Nil)) => {
                        let mut l: Vec<Value> = if o.is_nil() { vec![Value::Nil] } else { items(&o).to_vec() };
                        match l.iter().skip(1).step_by(2).position(|k| matches!(k, Value::Sym(s) if s == prop)) {
                            Some(i) => l[2 + 2 * i] = v.clone(),
                            None => l.extend([Value::Sym(prop.clone()), v.clone()]),
                        }
                        return match &**obj {
                            Node::Var(_) => {
                                self.set_target(obj, list(l))?;
                                Ok(v)
                            }
                            _ => fail("cannot set a property of a temporary property list"),
                        };
                    }
                    Value::Table(t) => set_table(&t, Value::Sym(prop.clone()), v.clone()),
                    o => self.set_prop(&o, prop, &v)?,
                }
            }
            Node::Index(obj, idx, _) => {
                let o = self.eval(obj)?;
                let k = self.eval(idx)?;
                match o {
                    Value::Table(t) => set_table(&t, k, v.clone()),
                    o => return fail(format!("cannot index {}", type_name(&o))),
                }
            }
            n => return fail(format!("cannot assign to {}", self.repr(&node_to_datum(n)))),
        }
        Ok(v)
    }

    fn iterate(&mut self, v: &Value) -> R<Vec<Value>> {
        match v {
            Value::Nil => Ok(vec![]),
            Value::List(l) => Ok(l.to_vec()),
            Value::Table(t) => Ok(t.borrow().entries.iter().map(|(k, _)| k.clone()).collect()),
            v => fail(format!("cannot iterate over {}", type_name(v))),
        }
    }

    fn special(&mut self, f: &str, args: &[Node]) -> Option<R<Value>> {
        let argc = |n: usize| if args.len() < n { fail(format!("{f}: too few arguments")) } else { Ok(()) };
        Some((|| -> R<Value> {
            match f {
                "quote" => {
                    argc(1)?;
                    Ok(node_to_datum(&args[0]))
                }
                "setq" => {
                    argc(2)?;
                    let v = self.eval(&args[1])?;
                    self.set_target(&args[0], v)
                }
                "preincrement" | "predecrement" | "postincrement" | "postdecrement" => {
                    argc(1)?;
                    let old = self.eval(&args[0])?;
                    let d = Value::Int(if f.contains("inc") { 1 } else { -1 });
                    let new = arith("plus", &old, &d)?;
                    self.set_target(&args[0], new.clone())?;
                    Ok(if f.starts_with("pre") { new } else { old })
                }
                "progn" => self.body(args),
                "if" => {
                    argc(1)?;
                    let c = truthy(&self.eval(&args[0])?);
                    let rest = &args[1..];
                    let is = |n: &Node, w: &str| matches!(n, Node::Var(s) if &**s == w);
                    if rest.first().is_some_and(|n| is(n, "then")) {
                        let e = rest.iter().position(|n| is(n, "else"));
                        let (then, els) = match e {
                            Some(e) => (&rest[1..e], &rest[e + 1..]),
                            None => (&rest[1..], &rest[..0]),
                        };
                        self.body(if c { then } else { els })
                    } else if rest.len() > 2 {
                        fail("if: use 'then' and 'else' for more than one expression per branch")
                    } else if c {
                        rest.first().map_or(Ok(Value::Nil), |n| self.eval(n))
                    } else {
                        rest.get(1).map_or(Ok(Value::Nil), |n| self.eval(n))
                    }
                }
                "when" | "unless" => {
                    argc(1)?;
                    let c = truthy(&self.eval(&args[0])?);
                    if c == (f == "when") { self.body(&args[1..]) } else { Ok(Value::Nil) }
                }
                "cond" => {
                    for clause in args {
                        let e = elems(clause);
                        let Some(test) = e.first() else { continue };
                        let v = self.eval(test)?;
                        if truthy(&v) {
                            return if e.len() > 1 { self.body(&e[1..]) } else { Ok(v) };
                        }
                    }
                    Ok(Value::Nil)
                }
                "case" | "caseq" => {
                    argc(1)?;
                    let v = self.eval(&args[0])?;
                    for clause in &args[1..] {
                        let e = elems(clause);
                        let Some(head) = e.first() else { continue };
                        let hit = match head {
                            Node::Lit(Value::T) => true,
                            Node::Form(..) | Node::Call(..) => elems(head).iter().any(|k| equal(&node_to_datum(k), &v)),
                            k => equal(&node_to_datum(k), &v),
                        };
                        if hit {
                            return self.body(&e[1..]);
                        }
                    }
                    Ok(Value::Nil)
                }
                "and" => {
                    let mut last = Value::T;
                    for a in args {
                        last = self.eval(a)?;
                        if !truthy(&last) {
                            return Ok(Value::Nil);
                        }
                    }
                    Ok(last)
                }
                "or" => {
                    for a in args {
                        let v = self.eval(a)?;
                        if truthy(&v) {
                            return Ok(v);
                        }
                    }
                    Ok(Value::Nil)
                }
                "while" => {
                    argc(1)?;
                    while truthy(&self.eval(&args[0])?) {
                        self.body(&args[1..])?;
                    }
                    Ok(Value::T)
                }
                "for" => {
                    argc(3)?;
                    let Node::Var(var) = &args[0] else { return fail("for: loop variable expected") };
                    let from = self.eval(&args[1])?;
                    let to = self.eval(&args[2])?;
                    let (Value::Int(from), Value::Int(to)) = (from, to) else { return fail("for: integer bounds expected") };
                    let frame = HashMap::from([(var.clone(), Value::Int(from))]);
                    self.with_frame(frame, |s| {
                        let mut i = from;
                        while i <= to {
                            s.frames.last_mut().unwrap().insert(var.clone(), Value::Int(i));
                            s.body(&args[3..])?;
                            i += 1;
                        }
                        Ok(Value::T)
                    })
                }
                "foreach" => {
                    argc(2)?;
                    let (mapper, rest) = match (&args[0], args.get(1)) {
                        (Node::Var(m), Some(_)) if matches!(&**m, "mapcar" | "mapc" | "mapcan") && args.len() >= 3 => {
                            (Some(m.clone()), &args[1..])
                        }
                        _ => (None, args),
                    };
                    let vars: Vec<Name> = match &rest[0] {
                        Node::Var(v) => vec![v.clone()],
                        n => elems(n)
                            .into_iter()
                            .map(|n| match n {
                                Node::Var(v) => Ok(v),
                                _ => fail("foreach: variable expected"),
                            })
                            .collect::<R<_>>()?,
                    };
                    if rest.len() < vars.len() + 1 {
                        return fail("foreach: too few arguments");
                    }
                    let mut lists = vec![];
                    let mut first = Value::Nil;
                    for (k, n) in rest[1..=vars.len()].iter().enumerate() {
                        let v = self.eval(n)?;
                        if k == 0 {
                            first = v.clone();
                        }
                        lists.push(self.iterate(&v)?);
                    }
                    let body = &rest[vars.len() + 1..];
                    let n = lists.iter().map(Vec::len).min().unwrap_or(0);
                    let mut results = vec![];
                    self.with_frame(HashMap::new(), |s| {
                        for i in 0..n {
                            for (v, l) in vars.iter().zip(&lists) {
                                s.frames.last_mut().unwrap().insert(v.clone(), l[i].clone());
                            }
                            let r = s.body(body)?;
                            if mapper.is_some() {
                                results.push(r);
                            }
                        }
                        Ok(())
                    })?;
                    Ok(match mapper.as_deref() {
                        Some("mapcar") => list(results),
                        Some("mapcan") => list(results.iter().flat_map(|r| items(r).to_vec()).collect()),
                        _ => first,
                    })
                }
                "setof" | "exists" | "forall" => {
                    argc(3)?;
                    let Node::Var(var) = &args[0] else { return fail(format!("{f}: variable expected")) };
                    let l = self.eval(&args[1])?;
                    let l = self.iterate(&l)?;
                    self.with_frame(HashMap::new(), |s| {
                        let mut kept = vec![];
                        for (i, x) in l.iter().enumerate() {
                            s.frames.last_mut().unwrap().insert(var.clone(), x.clone());
                            let ok = truthy(&s.body(&args[2..])?);
                            match f {
                                "setof" if ok => kept.push(x.clone()),
                                "exists" if ok => return Ok(list(l[i..].to_vec())),
                                "forall" if !ok => return Ok(Value::Nil),
                                _ => {}
                            }
                        }
                        Ok(match f {
                            "setof" => list(kept),
                            "exists" => Value::Nil,
                            _ => Value::T,
                        })
                    })
                }
                "let" | "prog" | "letseq" | "let*" => {
                    argc(1)?;
                    let frame = if f == "let" || f == "prog" {
                        self.bind_list(&args[0])?
                    } else {
                        let mut frame = HashMap::new();
                        self.frames.push(HashMap::new());
                        let r = (|| {
                            for b in elems(&args[0]) {
                                let one = self.bind_list(&Node::Form(vec![b], Pos { file: 0, line: 0 }))?;
                                self.frames.last_mut().unwrap().extend(one.clone());
                                frame.extend(one);
                            }
                            Ok(())
                        })();
                        self.frames.pop();
                        r?;
                        frame
                    };
                    let r = self.with_frame(frame, |s| s.body(&args[1..]));
                    match (f, r) {
                        ("prog", Err(Flow::Return(v))) => Ok(v),
                        ("prog", Ok(_)) => Ok(Value::Nil),
                        (_, r) => r,
                    }
                }
                "return" => {
                    let v = match args.first() {
                        Some(n) => self.eval(n)?,
                        None => Value::Nil,
                    };
                    Err(Flow::Return(v))
                }
                "procedure" | "nprocedure" | "globalProc" => {
                    argc(1)?;
                    self.define(&args[0], &args[1..])
                }
                "defun" => {
                    argc(2)?;
                    let mut sig = vec![args[0].clone()];
                    sig.extend(elems(&args[1]));
                    self.define(&Node::Form(sig, Pos { file: 0, line: 0 }), &args[2..])
                }
                "lambda" => {
                    argc(1)?;
                    let params = self.params(&elems(&args[0]))?;
                    Ok(Value::Func(Rc::new(Func { name: "lambda".into(), params, body: args[1..].to_vec() })))
                }
                "errset" => {
                    argc(1)?;
                    let verbose = match args.get(1) {
                        Some(n) => truthy(&self.eval(n)?),
                        None => false,
                    };
                    let (frames, depth) = (self.frames.len(), self.depth);
                    match self.eval(&args[0]) {
                        Ok(v) => Ok(list(vec![v])),
                        Err(Flow::Err(e)) => {
                            self.frames.truncate(frames);
                            self.depth = depth;
                            if verbose {
                                self.out.push_str(&format!("*Error* {}\n", e.msg));
                            }
                            self.globals.insert("errset".into(), list(vec![string(e.msg)]));
                            Ok(Value::Nil)
                        }
                        Err(r) => Err(r),
                    }
                }
                "sprintf" => {
                    argc(2)?;
                    let a = self.eval_args(&args[1..])?;
                    let Value::Str(fmt) = &a.pos[0] else { return fail("sprintf: format string expected") };
                    let s = string(self.format(fmt, &a.pos[1..])?);
                    if let Node::Var(_) = &args[0] {
                        self.set_target(&args[0], s.clone())?;
                    }
                    Ok(s)
                }
                "boundp" if matches!(args.first(), Some(Node::Var(_))) => {
                    let Some(Node::Var(v)) = args.first() else { unreachable!() };
                    Ok(boolv(self.lookup(v).is_ok()))
                }
                _ => Err(Flow::Return(Value::Nil)),
            }
        })())
        .filter(|r| !matches!(r, Err(Flow::Return(Value::Nil))) || f == "return")
    }

    // ----- printing -----

    fn repr(&self, v: &Value) -> String {
        let mut s = String::new();
        self.write_repr(v, &mut s);
        s
    }

    fn write_repr(&self, v: &Value, s: &mut String) {
        match v {
            Value::Nil => s.push_str("nil"),
            Value::T => s.push('t'),
            Value::Int(i) => s.push_str(&i.to_string()),
            Value::Float(f) => s.push_str(&fmt_float(*f)),
            Value::Str(x) => {
                s.push('"');
                for c in x.chars() {
                    match c {
                        '"' => s.push_str("\\\""),
                        '\\' => s.push_str("\\\\"),
                        '\n' => s.push_str("\\n"),
                        '\t' => s.push_str("\\t"),
                        c => s.push(c),
                    }
                }
                s.push('"');
            }
            Value::Sym(x) => s.push_str(x),
            Value::List(l) => {
                s.push('(');
                for (i, x) in l.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    self.write_repr(x, s);
                }
                s.push(')');
            }
            Value::Func(f) => s.push_str(&format!("funobj:{}", f.name)),
            Value::Obj(o) => {
                let (cv, id) = match o {
                    Obj::Cv(c) => (*c, 0),
                    Obj::Shape(c, id) | Obj::Inst(c, id) => (*c, *id),
                };
                s.push_str(&format!("db:0x{:08x}", ((cv as u64 + 1) << 24) | id));
            }
            Value::Table(t) => s.push_str(&format!("table:{}", t.borrow().name)),
            Value::Port => s.push_str("port:\"*stdout*\""),
        }
    }

    fn display(&self, v: &Value) -> String {
        match v {
            Value::Str(s) => s.to_string(),
            v => self.repr(v),
        }
    }

    fn format(&self, fmt: &str, args: &[Value]) -> R<String> {
        let mut out = String::new();
        let cs: Vec<char> = fmt.chars().collect();
        let mut i = 0;
        let mut next = args.iter();
        while i < cs.len() {
            if cs[i] != '%' {
                out.push(cs[i]);
                i += 1;
                continue;
            }
            i += 1;
            if cs.get(i) == Some(&'%') {
                out.push('%');
                i += 1;
                continue;
            }
            let mut flags = String::new();
            while let Some(&c) = cs.get(i).filter(|c| "-+ 0#".contains(**c)) {
                flags.push(c);
                i += 1;
            }
            let mut width = String::new();
            while let Some(&c) = cs.get(i).filter(|c| c.is_ascii_digit()) {
                width.push(c);
                i += 1;
            }
            let mut prec: Option<usize> = None;
            if cs.get(i) == Some(&'.') {
                i += 1;
                let mut p = String::new();
                while let Some(&c) = cs.get(i).filter(|c| c.is_ascii_digit()) {
                    p.push(c);
                    i += 1;
                }
                prec = Some(p.parse().unwrap_or(0));
            }
            let Some(&conv) = cs.get(i) else { return fail("format: incomplete % directive") };
            i += 1;
            let Some(arg) = next.next() else { return fail(format!("format: missing argument for %{conv}")) };
            let num = |v: &Value| match v {
                Value::Int(_) | Value::Float(_) => Ok(as_f64(v)),
                v => fail(format!("format: %{conv} expects a number, got {}", self.repr(v))),
            };
            let mut body = match conv {
                'd' | 'i' => match arg {
                    Value::Int(n) => n.to_string(),
                    v => (num(v)?.trunc() as i64).to_string(),
                },
                'x' | 'X' | 'o' | 'b' => {
                    let n = num(arg)? as i64;
                    match conv {
                        'x' => format!("{n:x}"),
                        'X' => format!("{n:X}"),
                        'o' => format!("{n:o}"),
                        _ => format!("{n:b}"),
                    }
                }
                'f' => format!("{:.*}", prec.unwrap_or(6), num(arg)?),
                'e' | 'E' => {
                    let s = format!("{:.*e}", prec.unwrap_or(6), num(arg)?);
                    let s = c_exponent(&s);
                    if conv == 'E' { s.to_uppercase() } else { s }
                }
                'g' | 'G' => format_g(num(arg)?, prec.unwrap_or(6)),
                's' => {
                    let s = self.display(arg);
                    match prec {
                        Some(p) => s.chars().take(p).collect(),
                        None => s,
                    }
                }
                'c' => match arg {
                    Value::Str(s) => s.chars().next().map(String::from).unwrap_or_default(),
                    Value::Int(n) => char::from_u32(*n as u32).map(String::from).unwrap_or_default(),
                    v => self.display(v),
                },
                'L' | 'A' | 'P' | 'B' => self.repr(arg),
                'n' | 'N' => match arg {
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => format_g(*f, prec.unwrap_or(6)),
                    v => return fail(format!("format: %n expects a number, got {}", self.repr(v))),
                },
                c => return fail(format!("format: unknown directive %{c}")),
            };
            if flags.contains('+') && matches!(conv, 'd' | 'i' | 'f' | 'e' | 'g' | 'n') && !body.starts_with('-') {
                body.insert(0, '+');
            }
            let w: usize = width.parse().unwrap_or(0);
            let len = body.chars().count();
            if len < w {
                let pad = w - len;
                if flags.contains('-') {
                    body.push_str(&" ".repeat(pad));
                } else if flags.contains('0') && !matches!(conv, 's' | 'L' | 'c') {
                    let sign = body.starts_with(['-', '+']);
                    body.insert_str(usize::from(sign), &"0".repeat(pad));
                } else {
                    body.insert_str(0, &" ".repeat(pad));
                }
            }
            out.push_str(&body);
        }
        Ok(out)
    }

    // ----- tables, property lists and indexing -----

    fn index(&mut self, o: &Value, k: &Value) -> R<Value> {
        match o {
            Value::Table(t) => {
                let t = t.borrow();
                Ok(t.entries
                    .iter()
                    .find(|(x, _)| equal(x, k))
                    .map(|(_, v)| v.clone())
                    .or(t.default.clone())
                    .unwrap_or(Value::Nil))
            }
            Value::List(l) => match k {
                Value::Int(i) if *i >= 0 => Ok(l.get(*i as usize).cloned().unwrap_or(Value::Nil)),
                _ => fail("list index must be a non-negative integer"),
            },
            o => fail(format!("cannot index {}", type_name(o))),
        }
    }

    fn get_prop(&mut self, o: &Value, prop: &Name) -> R<Value> {
        match o {
            Value::Obj(obj) => self.db_prop(*obj, prop),
            Value::Table(t) => {
                if &**prop == "?" {
                    return Ok(list(t.borrow().entries.iter().map(|(k, _)| k.clone()).collect()));
                }
                let key = Value::Sym(prop.clone());
                self.index(o, &key)
            }
            Value::Nil => Ok(Value::Nil),
            Value::List(l) => {
                if matches!(l[0], Value::Nil) {
                    if &**prop == "?" {
                        return Ok(list(l.iter().skip(1).step_by(2).cloned().collect()));
                    }
                    let mut it = l[1..].chunks(2);
                    return Ok(it
                        .find(|c| matches!(&c[0], Value::Sym(s) if s == prop))
                        .and_then(|c| c.get(1).cloned())
                        .unwrap_or(Value::Nil));
                }
                let mut out = vec![];
                for x in l.iter() {
                    out.push(self.get_prop(x, prop)?);
                }
                Ok(list(out))
            }
            o => fail(format!("cannot get property {prop} of {}", self.repr(o))),
        }
    }

    // ----- built-in functions -----

    fn builtin(&mut self, f: &str, a: Args) -> R<Value> {
        let v = a.pos;
        let n = v.len();
        let need = |k: usize| if n < k { fail(format!("{f}: too few arguments ({n} given, {k} required)")) } else { Ok(()) };
        let arg = |k: usize| v.get(k).cloned().unwrap_or(Value::Nil);
        let s = |k: usize| match v.get(k) {
            Some(Value::Str(s)) => Ok(s.clone()),
            Some(Value::Sym(s)) => Ok(s.clone()),
            Some(x) => fail(format!("{f}: argument {} must be a string, got {}", k + 1, self.repr(x))),
            None => fail(format!("{f}: missing argument {}", k + 1)),
        };
        let int = |k: usize| match v.get(k) {
            Some(Value::Int(i)) => Ok(*i),
            Some(Value::Float(x)) if x.fract() == 0.0 => Ok(*x as i64),
            Some(x) => fail(format!("{f}: argument {} must be an integer, got {}", k + 1, self.repr(x))),
            None => fail(format!("{f}: missing argument {}", k + 1)),
        };
        let num = |k: usize| match v.get(k) {
            Some(x @ (Value::Int(_) | Value::Float(_))) => Ok(as_f64(x)),
            Some(x) => fail(format!("{f}: argument {} must be a number, got {}", k + 1, self.repr(x))),
            None => fail(format!("{f}: missing argument {}", k + 1)),
        };
        let lst = |k: usize| match v.get(k) {
            Some(Value::Nil) | None => Ok(vec![]),
            Some(Value::List(l)) => Ok(l.to_vec()),
            Some(x) => fail(format!("{f}: argument {} must be a list, got {}", k + 1, self.repr(x))),
        };
        let float = |x: f64| Value::Float(x);
        match f {
            // arithmetic
            "plus" | "difference" | "times" | "quotient" => {
                need(1)?;
                let mut acc = v[0].clone();
                if n == 1 && f == "difference" {
                    return arith("difference", &Value::Int(0), &acc);
                }
                for x in &v[1..] {
                    acc = arith(f, &acc, x)?;
                }
                if n == 1 && !matches!(acc, Value::Int(_) | Value::Float(_)) {
                    return fail(format!("{f}: number expected"));
                }
                Ok(acc)
            }
            "minus" => {
                need(1)?;
                arith("difference", &Value::Int(0), &v[0])
            }
            "add1" | "sub1" => {
                need(1)?;
                arith(if f == "add1" { "plus" } else { "difference" }, &v[0], &Value::Int(1))
            }
            "expt" => {
                need(2)?;
                match (&v[0], &v[1]) {
                    (Value::Int(b), Value::Int(e)) if *e >= 0 => {
                        b.checked_pow(*e as u32).map(Value::Int).map_or_else(|| fail("expt: overflow"), Ok)
                    }
                    _ => Ok(float(num(0)?.powf(num(1)?))),
                }
            }
            "abs" => match arg(0) {
                Value::Int(i) => Ok(Value::Int(i.abs())),
                _ => Ok(float(num(0)?.abs())),
            },
            "min" | "max" => {
                need(1)?;
                let mut best = v[0].clone();
                for (k, cand) in v.iter().enumerate().skip(1) {
                    let (b, x) = (as_f64(&best), num(k)?);
                    if (f == "min" && x < b) || (f == "max" && x > b) {
                        best = cand.clone();
                    }
                }
                num(0)?;
                Ok(best)
            }
            "round" => Ok(Value::Int(num(0)?.round() as i64)),
            "fix" | "truncate" => Ok(Value::Int(num(0)?.trunc() as i64)),
            "floor" => Ok(Value::Int(num(0)?.floor() as i64)),
            "ceiling" => Ok(Value::Int(num(0)?.ceil() as i64)),
            "float" => Ok(float(num(0)?)),
            "integerp" | "fixp" => Ok(boolv(matches!(arg(0), Value::Int(_)))),
            "floatp" | "flonump" => Ok(boolv(matches!(arg(0), Value::Float(_)))),
            "numberp" => Ok(boolv(matches!(arg(0), Value::Int(_) | Value::Float(_)))),
            "zerop" => Ok(boolv(num(0)? == 0.0)),
            "plusp" => Ok(boolv(num(0)? > 0.0)),
            "minusp" | "negativep" => Ok(boolv(num(0)? < 0.0)),
            "evenp" => Ok(boolv(int(0)? % 2 == 0)),
            "oddp" => Ok(boolv(int(0)? % 2 != 0)),
            "onep" => Ok(boolv(num(0)? == 1.0)),
            "sqrt" => Ok(float(num(0)?.sqrt())),
            "exp" => Ok(float(num(0)?.exp())),
            "log" => Ok(float(num(0)?.ln())),
            "log10" => Ok(float(num(0)?.log10())),
            "sin" => Ok(float(num(0)?.sin())),
            "cos" => Ok(float(num(0)?.cos())),
            "tan" => Ok(float(num(0)?.tan())),
            "asin" => Ok(float(num(0)?.asin())),
            "acos" => Ok(float(num(0)?.acos())),
            "atan" => Ok(float(num(0)?.atan())),
            "atan2" => Ok(float(num(0)?.atan2(num(1)?))),
            "mod" | "remainder" | "rem" | "modulo" => {
                let (x, y) = (int(0)?, int(1)?);
                if y == 0 {
                    return fail(format!("{f}: division by zero"));
                }
                Ok(Value::Int(if f == "modulo" { x.rem_euclid(y) } else { x % y }))
            }
            "random" => {
                let k = if n > 0 { int(0)?.max(1) } else { i64::MAX };
                self.steps = self.steps.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                Ok(Value::Int(((self.steps >> 17) as i64).rem_euclid(k)))
            }
            "bitand" | "bitor" | "bitxor" | "leftshift" | "rightshift" => {
                let (x, y) = (int(0)?, int(1)?);
                Ok(Value::Int(match f {
                    "bitand" => x & y,
                    "bitor" => x | y,
                    "bitxor" => x ^ y,
                    "leftshift" => x << (y & 63),
                    _ => x >> (y & 63),
                }))
            }
            "bitnot" => Ok(Value::Int(!int(0)?)),
            "lessp" | "greaterp" | "leqp" | "geqp" => {
                need(2)?;
                let (x, y) = match (&v[0], &v[1]) {
                    (Value::Str(a), Value::Str(b)) => (a.cmp(b) as i32 as f64, 0.0),
                    _ => (num(0)?, num(1)?),
                };
                Ok(boolv(match f {
                    "lessp" => x < y,
                    "greaterp" => x > y,
                    "leqp" => x <= y,
                    _ => x >= y,
                }))
            }
            "equal" | "nequal" => {
                need(2)?;
                Ok(boolv(equal(&v[0], &v[1]) == (f == "equal")))
            }
            "eq" | "eqv" | "neq" => {
                need(2)?;
                Ok(boolv(eq(&v[0], &v[1]) == (f != "neq")))
            }
            "null" | "not" => Ok(boolv(!truthy(&arg(0)))),

            // lists
            "list" => Ok(list(v)),
            "range" => Ok(list(v)),
            "cons" => {
                need(2)?;
                let mut l = vec![v[0].clone()];
                l.extend(lst(1)?);
                Ok(list(l))
            }
            "xcons" => {
                need(2)?;
                let mut l = vec![v[1].clone()];
                l.extend(lst(0)?);
                Ok(list(l))
            }
            "car" | "xCoord" | "lowerLeft" => Ok(lst(0)?.first().cloned().unwrap_or(Value::Nil)),
            "cdr" => Ok(list(lst(0)?.into_iter().skip(1).collect())),
            "cadr" | "yCoord" | "upperRight" | "second" => Ok(lst(0)?.get(1).cloned().unwrap_or(Value::Nil)),
            "caddr" | "third" => Ok(lst(0)?.get(2).cloned().unwrap_or(Value::Nil)),
            "cadddr" => Ok(lst(0)?.get(3).cloned().unwrap_or(Value::Nil)),
            "cddr" => Ok(list(lst(0)?.into_iter().skip(2).collect())),
            "caar" | "cdar" | "cadar" | "caadr" => {
                let outer = lst(0)?;
                let pick = |l: &[Value], i: usize| l.get(i).cloned().unwrap_or(Value::Nil);
                Ok(match f {
                    "caar" => pick(items(&pick(&outer, 0)), 0),
                    "cdar" => list(items(&pick(&outer, 0)).iter().skip(1).cloned().collect()),
                    "cadar" => pick(items(&pick(&outer, 0)), 1),
                    _ => pick(items(&pick(&outer, 1)), 0),
                })
            }
            "first" => Ok(lst(0)?.first().cloned().unwrap_or(Value::Nil)),
            "rest" => Ok(list(lst(0)?.into_iter().skip(1).collect())),
            "leftEdge" | "bottomEdge" | "rightEdge" | "topEdge" => {
                let b = lst(0)?;
                let corner = if f == "leftEdge" || f == "bottomEdge" { 0 } else { 1 };
                let p = b.get(corner).map(|p| items(p).to_vec()).unwrap_or_default();
                Ok(p.get(usize::from(f == "bottomEdge" || f == "topEdge")).cloned().unwrap_or(Value::Nil))
            }
            "centerBox" => {
                let (x0, y0, x1, y1) = box_coords(&arg(0)).map_or_else(|| fail("centerBox: bounding box expected"), Ok)?;
                Ok(list(vec![float((x0 + x1) / 2.0), float((y0 + y1) / 2.0)]))
            }
            "length" => match arg(0) {
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                Value::Table(t) => Ok(Value::Int(t.borrow().entries.len() as i64)),
                _ => Ok(Value::Int(lst(0)?.len() as i64)),
            },
            "nth" => Ok(lst(1)?.get(int(0)?.max(0) as usize).cloned().unwrap_or(Value::Nil)),
            "nthelem" => Ok(lst(1)?.get((int(0)? - 1).max(0) as usize).cloned().unwrap_or(Value::Nil)),
            "nthcdr" => Ok(list(lst(1)?.into_iter().skip(int(0)?.max(0) as usize).collect())),
            "last" => Ok(lst(0)?.last().map(|x| list(vec![x.clone()])).unwrap_or(Value::Nil)),
            "reverse" => Ok(list(lst(0)?.into_iter().rev().collect())),
            "append" | "nconc" => {
                if let (Some(Value::Table(dst)), Some(Value::Table(src))) = (v.first(), v.get(1)) {
                    let entries = src.borrow().entries.clone();
                    for (k, x) in entries {
                        set_table(dst, k, x);
                    }
                    return Ok(v[0].clone());
                }
                let mut out = vec![];
                for k in 0..n {
                    out.extend(lst(k)?);
                }
                Ok(list(out))
            }
            "append1" => {
                need(2)?;
                let mut l = lst(0)?;
                l.push(v[1].clone());
                Ok(list(l))
            }
            "member" | "memq" | "memv" => {
                need(2)?;
                let l = lst(1)?;
                let cmp: fn(&Value, &Value) -> bool = if f == "member" { equal } else { eq };
                Ok(l.iter().position(|x| cmp(x, &v[0])).map_or(Value::Nil, |i| list(l[i..].to_vec())))
            }
            "assoc" | "assq" | "assv" => {
                need(2)?;
                Ok(lst(1)?.into_iter().find(|e| items(e).first().is_some_and(|k| equal(k, &v[0]))).unwrap_or(Value::Nil))
            }
            "remove" | "remq" | "remd" => {
                need(2)?;
                if let Value::Table(t) = &v[1] {
                    let mut t = t.borrow_mut();
                    let before = t.entries.len();
                    t.entries.retain(|(k, _)| !equal(k, &v[0]));
                    return Ok(boolv(t.entries.len() != before));
                }
                Ok(list(lst(1)?.into_iter().filter(|x| !equal(x, &v[0])).collect()))
            }
            "copy" => Ok(arg(0)),
            "subst" => {
                need(3)?;
                fn rec(new: &Value, old: &Value, x: &Value) -> Value {
                    if equal(x, old) {
                        return new.clone();
                    }
                    match x {
                        Value::List(l) => list(l.iter().map(|e| rec(new, old, e)).collect()),
                        x => x.clone(),
                    }
                }
                Ok(rec(&v[0], &v[1], &v[2]))
            }
            "listp" => Ok(boolv(matches!(arg(0), Value::Nil | Value::List(_)))),
            "pairp" | "dtpr" => Ok(boolv(matches!(arg(0), Value::List(_)))),
            "atom" => Ok(boolv(!matches!(arg(0), Value::List(_)))),
            "stringp" => Ok(boolv(matches!(arg(0), Value::Str(_)))),
            "symbolp" => Ok(boolv(matches!(arg(0), Value::Sym(_) | Value::T | Value::Nil))),
            "tablep" => Ok(boolv(matches!(arg(0), Value::Table(_)))),
            "dbobjectp" | "dbIsId" => Ok(boolv(matches!(arg(0), Value::Obj(_)))),
            "isCallable" | "fboundp" => Ok(boolv(match arg(0) {
                Value::Sym(s) => self.funcs.contains_key(&s),
                Value::Func(_) => true,
                _ => false,
            })),
            "type" | "typep" => Ok(sym(type_name(&arg(0)))),
            "mapcar" | "mapc" | "mapcan" => {
                need(2)?;
                let lists: Vec<Vec<Value>> = (1..n).map(lst).collect::<R<_>>()?;
                let len = lists.iter().map(Vec::len).min().unwrap_or(0);
                let mut out = vec![];
                for i in 0..len {
                    let r = self.call1(&v[0], lists.iter().map(|l| l[i].clone()).collect())?;
                    out.push(r);
                }
                Ok(match f {
                    "mapcar" => list(out),
                    "mapcan" => list(out.iter().flat_map(|r| items(r).to_vec()).collect()),
                    _ => v[1].clone(),
                })
            }
            "funcall" => {
                need(1)?;
                self.call_value(&v[0], Args { pos: v[1..].to_vec(), keys: a.keys })
            }
            "apply" => {
                need(2)?;
                let mut args = v[1..n - 1].to_vec();
                args.extend(lst(n - 1)?);
                self.call_value(&v[0], Args { pos: args, keys: a.keys })
            }
            "sort" | "sortcar" => {
                need(1)?;
                let l = lst(0)?;
                let cmp = arg(1);
                self.sort(l, &cmp, f == "sortcar").map(list)
            }
            "eval" => {
                need(1)?;
                let node = datum_to_node(&v[0]);
                self.eval(&node)
            }
            "evalstring" => {
                let src = s(0)?;
                self.run_source(&src, None)
            }
            "load" | "loadi" => {
                let path = PathBuf::from(&*s(0)?);
                let path = match self.load_stack.last().cloned().flatten().and_then(|p| p.parent().map(Path::to_path_buf)) {
                    Some(dir) if path.is_relative() => dir.join(path),
                    _ => path,
                };
                let src = match std::fs::read_to_string(&path) {
                    Ok(src) => src,
                    Err(e) => return fail(format!("load: cannot read {}: {e}", path.display())),
                };
                let r = self.run_source(&src, Some(path));
                if f == "loadi" {
                    if let Err(Flow::Err(e)) = r {
                        self.out.push_str(&format!("*Error* {}\n", e.msg));
                    } else {
                        r?;
                    }
                    return Ok(Value::T);
                }
                r.map(|_| Value::T)
            }
            "getd" => Ok(match arg(0) {
                Value::Sym(s) => self.funcs.get(&s).cloned().map_or(Value::Nil, Value::Func),
                _ => Value::Nil,
            }),
            "set" => {
                need(2)?;
                let name = s(0)?;
                self.assign(&name, v[1].clone());
                Ok(v[1].clone())
            }
            "symeval" => self.lookup(&s(0)?),
            "makeTable" => Ok(Value::Table(Rc::new(RefCell::new(Table {
                name: s(0).map(|s| s.to_string()).unwrap_or_default(),
                default: v.get(1).cloned(),
                entries: vec![],
            })))),
            "putprop" => {
                need(3)?;
                match &v[0] {
                    Value::Table(t) => set_table(t, sym(&s(2)?), v[1].clone()),
                    o => {
                        let p = s(2)?;
                        self.set_prop(o, &p, &v[1])?;
                    }
                }
                Ok(v[1].clone())
            }
            "get" => {
                need(2)?;
                let p = s(1)?;
                self.get_prop(&v[0], &p)
            }

            // strings and symbols
            "strcat" => {
                let mut out = String::new();
                for x in &v {
                    match x {
                        Value::Str(s) | Value::Sym(s) => out.push_str(s),
                        Value::Nil => {}
                        x => return fail(format!("strcat: string expected, got {}", self.repr(x))),
                    }
                }
                Ok(string(out))
            }
            "strlen" => Ok(Value::Int(s(0)?.chars().count() as i64)),
            "substring" => {
                let st = s(0)?;
                let chars: Vec<char> = st.chars().collect();
                let start = int(1)?;
                let start = if start < 0 { chars.len() as i64 + start } else { start - 1 }.clamp(0, chars.len() as i64) as usize;
                let len = if n > 2 { int(2)?.max(0) as usize } else { chars.len() };
                Ok(string(chars[start..].iter().take(len).collect::<String>()))
            }
            "upperCase" | "lowerCase" => {
                let st = s(0)?;
                Ok(string(if f == "upperCase" { st.to_uppercase() } else { st.to_lowercase() }))
            }
            "strcmp" => Ok(Value::Int(s(0)?.cmp(&s(1)?) as i64)),
            "strncmp" => {
                let k = int(2)?.max(0) as usize;
                let (x, y): (String, String) = (s(0)?.chars().take(k).collect(), s(1)?.chars().take(k).collect());
                Ok(Value::Int(x.cmp(&y) as i64))
            }
            "index" | "rindex" => {
                let (st, pat) = (s(0)?, s(1)?);
                let found = if f == "index" { st.find(&*pat) } else { st.rfind(&*pat) };
                Ok(found.map_or(Value::Nil, |i| string(&st[i..])))
            }
            "nindex" => {
                let (st, pat) = (s(0)?, s(1)?);
                Ok(st.find(&*pat).map_or(Value::Nil, |i| Value::Int(st[..i].chars().count() as i64 + 1)))
            }
            "parseString" => {
                let st = s(0)?;
                let delims: Vec<char> = if n > 1 { s(1)?.chars().collect() } else { vec![' ', '\t', '\n'] };
                Ok(list(st.split(|c| delims.contains(&c)).filter(|p| !p.is_empty()).map(string).collect()))
            }
            "buildString" => {
                let sep = if n > 1 { s(1)?.to_string() } else { " ".into() };
                let parts: Vec<String> = lst(0)?.iter().map(|x| self.display(x)).collect();
                Ok(string(parts.join(&sep)))
            }
            "concat" => {
                let mut out = String::new();
                for x in &v {
                    out.push_str(&self.display(x));
                }
                Ok(sym(&out))
            }
            "symbolToString" | "get_pname" => Ok(string(s(0)?)),
            "stringToSymbol" | "intern" => Ok(sym(&s(0)?)),
            "atoi" => Ok(s(0)?.trim().parse::<i64>().map_or(Value::Nil, Value::Int)),
            "atof" => Ok(s(0)?.trim().parse::<f64>().map_or(Value::Nil, float)),
            "evalNumber" | "aelNumber" => Ok(match lex(&s(0)?, 0).ok().as_deref() {
                Some([(Tok::Num(x, true), _), (Tok::Eof, _)]) => Value::Int(*x as i64),
                Some([(Tok::Num(x, false), _), (Tok::Eof, _)]) => float(*x),
                _ => Value::Nil,
            }),
            "sprintf" => {
                need(2)?;
                let fmt = s(1)?;
                Ok(string(self.format(&fmt, &v[2..])?))
            }
            "lsprintf" => {
                let fmt = s(0)?;
                Ok(string(self.format(&fmt, &v[1..])?))
            }
            "rexMatchp" | "pcreMatchp" => {
                let (pat, st) = (s(0)?, s(1)?);
                Ok(boolv(simple_match(&pat, &st)))
            }

            // output
            "printf" | "info" => {
                let fmt = s(0)?;
                let text = self.format(&fmt, &v[1..])?;
                self.out.push_str(&text);
                Ok(Value::T)
            }
            "warn" => {
                let fmt = s(0)?;
                let text = self.format(&fmt, &v[1..])?;
                self.out.push_str(&format!("*WARNING* {text}"));
                if !text.ends_with('\n') {
                    self.out.push('\n');
                }
                Ok(Value::Nil)
            }
            "fprintf" => {
                let fmt = s(1)?;
                let text = self.format(&fmt, &v[2..])?;
                self.out.push_str(&text);
                Ok(Value::T)
            }
            "print" | "println" | "pprint" => {
                need(1)?;
                let text = self.repr(&v[0]);
                self.out.push_str(&text);
                if f != "print" {
                    self.out.push('\n');
                }
                Ok(Value::Nil)
            }
            "printlev" => {
                let text = self.repr(&arg(0));
                self.out.push_str(&text);
                self.out.push('\n');
                Ok(Value::Nil)
            }
            "newline" => {
                self.out.push('\n');
                Ok(Value::Nil)
            }
            "drain" | "close" | "gc" | "sstatus" | "status" | "hiRegTimer" => Ok(Value::T),
            "error" => {
                let text = match v.first() {
                    Some(Value::Str(fmt)) => self.format(fmt, &v[1..])?,
                    Some(x) => self.display(x),
                    None => "error".into(),
                };
                fail(text.trim_end().to_string())
            }
            "getCurrentTime" => Ok(string("Jan  1 00:00:00 2026")),
            "getVersion" => Ok(string("IC Studio SKILL")),
            "getShellEnvVar" => Ok(std::env::var(&*s(0)?).map_or(Value::Nil, string)),

            // layout database
            f if f.starts_with("db") || f.starts_with("ge") || f.starts_with("de") || f.starts_with("le") => self.db_call(f, &v),
            _ => fail(format!("undefined function {f}")),
        }
    }

    fn sort(&mut self, l: Vec<Value>, cmp: &Value, by_car: bool) -> R<Vec<Value>> {
        if l.len() <= 1 {
            return Ok(l);
        }
        let mut right = l;
        let left = right.drain(..right.len() / 2).collect();
        let a = self.sort(left, cmp, by_car)?;
        let b = self.sort(right, cmp, by_car)?;
        let mut out = Vec::with_capacity(a.len() + b.len());
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            let key = |v: &Value| if by_car { items(v).first().cloned().unwrap_or(Value::Nil) } else { v.clone() };
            let (x, y) = (key(&b[j]), key(&a[i]));
            let b_first = match cmp {
                Value::Nil => match (&x, &y) {
                    (Value::Str(p), Value::Str(q)) => p < q,
                    _ => as_f64(&x) < as_f64(&y),
                },
                f => truthy(&self.call1(f, vec![x, y])?),
            };
            if b_first {
                out.push(b[j].clone());
                j += 1;
            } else {
                out.push(a[i].clone());
                i += 1;
            }
        }
        out.extend_from_slice(&a[i..]);
        out.extend_from_slice(&b[j..]);
        Ok(out)
    }

    // ----- layout database -----

    fn dbu(&self) -> f64 {
        self.p.tech.dbu_per_um.max(1) as f64
    }

    fn to_dbu(&self, x: f64) -> i64 {
        (x * self.dbu()).round() as i64
    }

    fn uu(&self, x: i64) -> Value {
        Value::Float(x as f64 / self.dbu())
    }

    fn pt_val(&self, p: (i64, i64)) -> Value {
        list(vec![self.uu(p.0), self.uu(p.1)])
    }

    fn box_val(&self, r: IRect) -> Value {
        list(vec![self.pt_val((r.x0, r.y0)), self.pt_val((r.x1, r.y1))])
    }

    fn point(&self, v: &Value) -> R<(i64, i64)> {
        match items(v) {
            [x @ (Value::Int(_) | Value::Float(_)), y @ (Value::Int(_) | Value::Float(_))] => {
                Ok((self.to_dbu(as_f64(x)), self.to_dbu(as_f64(y))))
            }
            _ => fail(format!("point x:y expected, got {}", self.repr(v))),
        }
    }

    fn points(&self, v: &Value) -> R<Vec<(i64, i64)>> {
        items(v).iter().map(|p| self.point(p)).collect()
    }

    fn bbox(&self, v: &Value) -> R<IRect> {
        match items(v) {
            [a, b] => {
                let (a, b) = (self.point(a)?, self.point(b)?);
                Ok(IRect::new(a.0, a.1, b.0, b.1))
            }
            _ => fail(format!("bounding box list(x0:y0 x1:y1) expected, got {}", self.repr(v))),
        }
    }

    fn layer(&self, v: &Value) -> R<usize> {
        let tech = &self.p.tech;
        let (name, purpose) = match v {
            Value::Int(i) if (*i as usize) < tech.layers.len() && *i >= 0 => return Ok(*i as usize),
            Value::Str(s) | Value::Sym(s) => (s.to_string(), String::new()),
            Value::List(l) => match l.as_slice() {
                [Value::Str(a) | Value::Sym(a)] => (a.to_string(), String::new()),
                [Value::Str(a) | Value::Sym(a), Value::Str(b) | Value::Sym(b)] => (a.to_string(), b.to_string()),
                _ => return fail(format!("layer-purpose pair expected, got {}", self.repr(v))),
            },
            _ => return fail(format!("layer expected, got {}", self.repr(v))),
        };
        let mut tries = vec![];
        if !purpose.is_empty() && purpose != "drawing" {
            tries.push(format!("{name}.{purpose}"));
            tries.push(format!("{name}_{purpose}"));
        }
        tries.push(name.clone());
        for t in &tries {
            if let Some(i) = tech.layer_index(t) {
                return Ok(i);
            }
        }
        let names: Vec<&str> = tech.layers.iter().map(|l| l.name.as_str()).collect();
        fail(format!("unknown layer '{name}' in technology '{}' (layers: {})", tech.name, names.join(" ")))
    }

    fn cv(&self, v: &Value) -> R<usize> {
        match v {
            Value::Obj(Obj::Cv(c)) => Ok(*c),
            v => fail(format!("cellview expected, got {}", self.repr(v))),
        }
    }

    fn open_cv(&mut self, cell: CellRef, writable: bool) -> Value {
        if let Some(i) = self.cvs.iter().position(|c| c.cell == cell) {
            self.cvs[i].writable |= writable;
            return Value::Obj(Obj::Cv(i));
        }
        self.cvs.push(CvState { cell, writable });
        Value::Obj(Obj::Cv(self.cvs.len() - 1))
    }

    fn layout(&self, cv: usize) -> Option<&Layout> {
        self.p.cell(&self.cvs[cv].cell)?.layout.as_ref()
    }

    fn layout_mut(&mut self, cv: usize) -> R<&mut Layout> {
        let st = &self.cvs[cv];
        if !st.writable {
            return fail(format!(
                "cellview {}/{} is open read-only (open it with mode \"a\" or \"w\")",
                st.cell.lib, st.cell.cell
            ));
        }
        let cell = st.cell.clone();
        if !self.modified.contains(&cell) {
            self.modified.push(cell.clone());
        }
        match self.p.cell_mut(&cell) {
            Some(c) => Ok(c.layout.get_or_insert_with(Layout::default)),
            None => fail(format!("cell {}/{} no longer exists", cell.lib, cell.cell)),
        }
    }

    fn cell_bbox(&self, cell: &CellRef) -> Option<IRect> {
        let l = self.p.cell(cell)?.layout.as_ref()?;
        crate::hier::flatten(l, &*self.p, 32).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b))
    }

    fn inst_bbox(&self, inst: &CellInst) -> IRect {
        match self.cell_bbox(&inst.cell) {
            Some(b) => inst.bbox(b),
            None => IRect::new(inst.trans.x, inst.trans.y, inst.trans.x, inst.trans.y),
        }
    }

    fn master(&self, cv: usize, lib: &str, cell: &str) -> R<Option<CellRef>> {
        let r = CellRef::new(lib, cell);
        if self.p.cell(&r).and_then(|c| c.layout.as_ref()).is_none() {
            return Ok(None);
        }
        let parent = &self.cvs[cv].cell;
        if r == *parent || crate::hier::would_cycle(&*self.p, parent, &r) {
            return fail(format!("placing {lib}/{cell} in {}/{} would create a cycle", parent.lib, parent.cell));
        }
        Ok(Some(r))
    }

    fn place(
        &mut self,
        cv: usize,
        cell: CellRef,
        name: &Value,
        origin: &Value,
        orient: &Value,
        array: Option<ArraySpec>,
    ) -> R<Value> {
        let (x, y) = self.point(origin)?;
        let (rot, mirror) = parse_orient(&self.display(orient))?;
        let name = match name {
            Value::Str(s) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        };
        let l = self.layout_mut(cv)?;
        let id = l.add_instance(cell, Trans::with(x, y, rot, mirror), array);
        if let Some(i) = l.instance_mut(id) {
            i.name = name;
        }
        Ok(Value::Obj(Obj::Inst(cv, id)))
    }

    fn transform(&self, v: &Value) -> R<Trans> {
        let l = items(v);
        let (dx, dy) = match l.first() {
            Some(p) => self.point(p)?,
            None => (0, 0),
        };
        let (rot, mirror) = match l.get(1) {
            Some(o) => parse_orient(&self.display(o))?,
            None => (0, false),
        };
        Ok(Trans::with(dx, dy, rot, mirror))
    }

    fn fig(&self, v: &Value) -> R<(usize, u64)> {
        match v {
            Value::Obj(Obj::Shape(c, id) | Obj::Inst(c, id)) => Ok((*c, *id)),
            v => fail(format!("shape or instance expected, got {}", self.repr(v))),
        }
    }

    fn db_call(&mut self, f: &str, v: &[Value]) -> R<Value> {
        let n = v.len();
        let need = |k: usize| if n < k { fail(format!("{f}: too few arguments ({n} given, {k} required)")) } else { Ok(()) };
        let arg = |k: usize| v.get(k).cloned().unwrap_or(Value::Nil);
        let text = |k: usize| match v.get(k) {
            Some(Value::Str(s) | Value::Sym(s)) => Ok(s.to_string()),
            x => fail(format!("{f}: argument {} must be a string, got {}", k + 1, x.map_or("nothing".into(), |x| self.repr(x)))),
        };
        match f {
            "dbOpenCellViewByType" | "dbOpenCellView" => {
                need(3)?;
                let (lib, cell, view) = (text(0)?, text(1)?, text(2)?);
                if !matches!(view.as_str(), "layout" | "maskLayout") {
                    return fail(format!("{f}: only layout views are supported, got view '{view}'"));
                }
                let mode = match v.get(4) {
                    Some(Value::Str(m)) => m.to_string(),
                    _ => "r".into(),
                };
                let r = CellRef::new(&lib, &cell);
                match mode.chars().next() {
                    Some('r') => Ok(if self.p.cell(&r).and_then(|c| c.layout.as_ref()).is_some() {
                        self.open_cv(r, false)
                    } else {
                        Value::Nil
                    }),
                    Some(m @ ('w' | 'a')) => {
                        if self.p.libraries.iter().all(|l| l.name != lib) {
                            self.p.add_library(&lib).or_else(fail)?;
                        }
                        if self.p.cell(&r).is_none() {
                            self.p.add_cell(&lib, Cell::new(&cell)).or_else(fail)?;
                        }
                        let cv = self.open_cv(r, true);
                        let idx = self.cv(&cv)?;
                        let l = self.layout_mut(idx)?;
                        if m == 'w' {
                            *l = Layout::default();
                        }
                        Ok(cv)
                    }
                    _ => fail(format!("{f}: unknown mode '{mode}' (use \"r\", \"w\" or \"a\")")),
                }
            }
            "dbFindOpenCellView" => {
                need(3)?;
                let r = CellRef::new(text(0)?, text(1)?);
                Ok(self.cvs.iter().position(|c| c.cell == r).map_or(Value::Nil, |i| Value::Obj(Obj::Cv(i))))
            }
            "geGetEditCellView" | "geGetWindowCellView" | "deGetCellView" | "geGetTopLevelCellView" | "leGetEditCellView" => {
                Ok(match self.current.clone() {
                    Some(c) if self.p.cell(&c).is_some() => self.open_cv(c, true),
                    _ => Value::Nil,
                })
            }
            "dbSave" | "dbClose" | "dbPurge" | "dbReopen" | "dbRefreshCellView" | "geRefresh" | "hiRedraw" => Ok(Value::T),
            "dbCreateRect" => {
                need(3)?;
                let cv = self.cv(&v[0])?;
                let layer = self.layer(&v[1])?;
                let r = self.bbox(&v[2])?;
                if r.width() == 0 || r.height() == 0 {
                    return Ok(Value::Nil);
                }
                let id = self.layout_mut(cv)?.add(layer, Geometry::Rect(r));
                Ok(Value::Obj(Obj::Shape(cv, id)))
            }
            "dbCreatePolygon" => {
                need(3)?;
                let cv = self.cv(&v[0])?;
                let layer = self.layer(&v[1])?;
                let mut points = self.points(&v[2])?;
                points.dedup();
                if points.len() > 1 && points.first() == points.last() {
                    points.pop();
                }
                if points.len() < 3 {
                    return fail(format!("{f}: a polygon needs at least 3 distinct points"));
                }
                let id = self.layout_mut(cv)?.add(layer, Geometry::Polygon { points });
                Ok(Value::Obj(Obj::Shape(cv, id)))
            }
            "dbCreatePath" => {
                need(4)?;
                let cv = self.cv(&v[0])?;
                let layer = self.layer(&v[1])?;
                let points = self.points(&v[2])?;
                if points.len() < 2 {
                    return fail(format!("{f}: a path needs at least 2 points"));
                }
                let w = match &v[3] {
                    x @ (Value::Int(_) | Value::Float(_)) => self.to_dbu(as_f64(x)),
                    x => return fail(format!("{f}: width must be a number, got {}", self.repr(x))),
                };
                let id = self.layout_mut(cv)?.add(layer, Geometry::Path { points, width: w });
                Ok(Value::Obj(Obj::Shape(cv, id)))
            }
            "dbCreateLabel" | "dbCreateTextDisplay" => {
                need(4)?;
                let cv = self.cv(&v[0])?;
                let layer = self.layer(&v[1])?;
                let at = self.point(&v[2])?;
                let label = self.display(&v[3]);
                let size = match v.get(7) {
                    Some(x @ (Value::Int(_) | Value::Float(_))) => self.to_dbu(as_f64(x)).max(1),
                    _ => self.to_dbu(0.1).max(1),
                };
                let id = self.layout_mut(cv)?.add(layer, Geometry::Label { text: label, at, size });
                Ok(Value::Obj(Obj::Shape(cv, id)))
            }
            "dbCreateInst" => {
                need(5)?;
                let cv = self.cv(&v[0])?;
                let mcv = self.cv(&v[1])?;
                let r = self.cvs[mcv].cell.clone();
                let Some(cell) = self.master(cv, &r.lib, &r.cell)? else { return Ok(Value::Nil) };
                self.place(cv, cell, &v[2], &v[3], &v[4], None)
            }
            "dbCreateInstByMasterName" => {
                need(7)?;
                let cv = self.cv(&v[0])?;
                let (lib, cell) = (text(1)?, text(2)?);
                match self.master(cv, &lib, &cell)? {
                    Some(r) => self.place(cv, r, &v[4], &v[5], &v[6], None),
                    None => {
                        self.out.push_str(&format!("*WARNING* {f}: cannot open master {lib}/{cell} layout\n"));
                        Ok(Value::Nil)
                    }
                }
            }
            "dbCreateSimpleMosaic" | "dbCreateSimpleMosaicByMasterName" => {
                let by_name = f.ends_with("MasterName");
                need(if by_name { 11 } else { 9 })?;
                let cv = self.cv(&v[0])?;
                let (cell, k) = if by_name {
                    (self.master(cv, &text(1)?, &text(2)?)?, 4)
                } else {
                    let mcv = self.cv(&v[1])?;
                    let r = self.cvs[mcv].cell.clone();
                    (self.master(cv, &r.lib, &r.cell)?, 2)
                };
                let Some(cell) = cell else { return Ok(Value::Nil) };
                let rows = self.int_arg(f, &v[k + 3])?;
                let cols = self.int_arg(f, &v[k + 4])?;
                let dy = self.to_dbu(as_f64(&v[k + 5]));
                let dx = self.to_dbu(as_f64(&v[k + 6]));
                if rows < 1 || cols < 1 {
                    return fail(format!("{f}: rows and columns must be at least 1"));
                }
                let spec = ArraySpec { cols: cols as u32, rows: rows as u32, col: (dx, 0), row: (0, dy) };
                self.place(cv, cell, &v[k], &v[k + 1], &v[k + 2], Some(spec))
            }
            "dbDeleteObject" => {
                need(1)?;
                if let Value::List(l) = &v[0] {
                    for x in l.iter() {
                        self.db_call(f, std::slice::from_ref(x))?;
                    }
                    return Ok(Value::T);
                }
                let (cv, id) = self.fig(&v[0])?;
                let l = self.layout_mut(cv)?;
                let found = l.shape(id).is_some() || l.instance(id).is_some();
                l.remove(id);
                Ok(boolv(found))
            }
            "dbMoveFig" | "dbCopyFig" => {
                need(1)?;
                let (cv, id) = self.fig(&v[0])?;
                let dst = match v.get(1) {
                    Some(c @ Value::Obj(Obj::Cv(_))) => self.cv(c)?,
                    _ => cv,
                };
                let t = self.transform(&arg(2))?;
                let ids = std::iter::once(id).collect();
                if f == "dbMoveFig" && dst == cv {
                    self.layout_mut(cv)?.transform_items(&ids, &t);
                    return Ok(v[0].clone());
                }
                let Some(mut piece) = self.layout(cv).cloned() else {
                    return fail(format!("{f}: object no longer exists"));
                };
                piece.shapes.retain(|s| s.id == id);
                piece.instances.retain(|i| i.id == id);
                if piece.is_empty() {
                    return fail(format!("{f}: object no longer exists"));
                }
                piece.transform_items(&ids, &t);
                if f == "dbMoveFig" {
                    self.layout_mut(cv)?.remove(id);
                }
                let l = self.layout_mut(dst)?;
                let new = if let Some(s) = piece.shapes.pop() {
                    Obj::Shape(dst, l.add(s.layer, s.geom))
                } else {
                    let i = piece.instances.pop().unwrap();
                    let nid = l.add_instance(i.cell, i.trans, i.array);
                    if let Some(x) = l.instance_mut(nid) {
                        x.name = i.name;
                    }
                    Obj::Inst(dst, nid)
                };
                Ok(Value::Obj(new))
            }
            "dbFindAnyInstByName" | "dbFindInstByName" => {
                need(2)?;
                let cv = self.cv(&v[0])?;
                let name = text(1)?;
                Ok(self
                    .layout(cv)
                    .and_then(|l| l.instances.iter().find(|i| i.name.as_deref() == Some(&name)))
                    .map_or(Value::Nil, |i| Value::Obj(Obj::Inst(cv, i.id))))
            }
            "dbGet" | "dbGetq" => {
                need(2)?;
                let p = text(1)?;
                self.get_prop(&v[0], &p.into())
            }
            "dbTransformPoint" => {
                need(2)?;
                let t = self.transform(&v[1])?;
                let p = self.point(&v[0])?;
                Ok(self.pt_val(t.apply(p)))
            }
            "dbTransformBBox" => {
                need(2)?;
                let t = self.transform(&v[1])?;
                let b = self.bbox(&v[0])?;
                Ok(self.box_val(t.apply_rect(b)))
            }
            "dbGetTrueOverlaps" | "dbGetOverlaps" => {
                need(2)?;
                let cv = self.cv(&v[0])?;
                let area = self.bbox(&v[1])?;
                let layer = match v.get(2) {
                    Some(Value::Nil) | None => None,
                    Some(x) => Some(self.layer(x)?),
                };
                let Some(l) = self.layout(cv) else { return Ok(Value::Nil) };
                let mut out = vec![];
                for s in &l.shapes {
                    if layer.is_none_or(|k| k == s.layer) && s.bbox().intersects(&area) {
                        out.push(Value::Obj(Obj::Shape(cv, s.id)));
                    }
                }
                if layer.is_none() {
                    for i in &l.instances {
                        if self.inst_bbox(i).intersects(&area) {
                            out.push(Value::Obj(Obj::Inst(cv, i.id)));
                        }
                    }
                }
                Ok(list(out))
            }
            _ => fail(format!("undefined function {f}")),
        }
    }

    fn int_arg(&self, f: &str, v: &Value) -> R<i64> {
        match v {
            Value::Int(i) => Ok(*i),
            Value::Float(x) if x.fract() == 0.0 => Ok(*x as i64),
            v => fail(format!("{f}: integer expected, got {}", self.repr(v))),
        }
    }

    fn db_prop(&mut self, o: Obj, prop: &str) -> R<Value> {
        let lname = |s: &Self, layer: usize| s.p.tech.layer(layer).map(|l| l.name.clone()).unwrap_or_else(|| format!("L{layer}"));
        match o {
            Obj::Cv(cv) => {
                let cell = self.cvs[cv].cell.clone();
                Ok(match prop {
                    "objType" => string("cellView"),
                    "libName" => string(cell.lib.as_str()),
                    "cellName" => string(cell.cell.as_str()),
                    "viewName" => string("layout"),
                    "cellViewType" => string("maskLayout"),
                    "mode" => string(if self.cvs[cv].writable { "a" } else { "r" }),
                    "DBUPerUU" => Value::Float(self.dbu()),
                    "userUnits" => string("micron"),
                    "shapes" => list(
                        self.layout(cv)
                            .map(|l| l.shapes.iter().map(|s| Value::Obj(Obj::Shape(cv, s.id))).collect())
                            .unwrap_or_default(),
                    ),
                    "instances" => list(
                        self.layout(cv)
                            .map(|l| l.instances.iter().map(|i| Value::Obj(Obj::Inst(cv, i.id))).collect())
                            .unwrap_or_default(),
                    ),
                    "bBox" => self.cell_bbox(&cell).map_or(Value::Nil, |b| self.box_val(b)),
                    "isParamCell" => Value::Nil,
                    "layerPurposePairs" | "lpps" => {
                        let mut layers: Vec<usize> =
                            self.layout(cv).map(|l| l.shapes.iter().map(|s| s.layer).collect()).unwrap_or_default();
                        layers.sort_unstable();
                        layers.dedup();
                        list(layers.into_iter().map(|k| list(vec![string(lname(self, k)), string("drawing")])).collect())
                    }
                    _ => return fail(format!("unknown cellview property {prop}")),
                })
            }
            Obj::Shape(cv, id) => {
                let Some(s) = self.layout(cv).and_then(|l| l.shape(id)).cloned() else {
                    return fail("the shape was deleted");
                };
                Ok(match prop {
                    "objType" => string(match s.geom {
                        Geometry::Rect(_) => "rect",
                        Geometry::Polygon { .. } => "polygon",
                        Geometry::Path { .. } => "path",
                        Geometry::Label { .. } => "label",
                    }),
                    "layerName" => string(lname(self, s.layer)),
                    "layerNum" => Value::Int(self.p.tech.layer(s.layer).map_or(0, |l| l.gds_layer as i64)),
                    "purpose" => string("drawing"),
                    "lpp" => list(vec![string(lname(self, s.layer)), string("drawing")]),
                    "bBox" => self.box_val(s.bbox()),
                    "cellView" => Value::Obj(Obj::Cv(cv)),
                    "points" => match &s.geom {
                        Geometry::Polygon { points } | Geometry::Path { points, .. } => {
                            list(points.iter().map(|p| self.pt_val(*p)).collect())
                        }
                        Geometry::Rect(r) => list(
                            [(r.x0, r.y0), (r.x0, r.y1), (r.x1, r.y1), (r.x1, r.y0)].iter().map(|p| self.pt_val(*p)).collect(),
                        ),
                        Geometry::Label { at, .. } => list(vec![self.pt_val(*at)]),
                    },
                    "nPoints" => Value::Int(match &s.geom {
                        Geometry::Polygon { points } | Geometry::Path { points, .. } => points.len() as i64,
                        Geometry::Rect(_) => 4,
                        Geometry::Label { .. } => 1,
                    }),
                    "width" => match &s.geom {
                        Geometry::Path { width, .. } => self.uu(*width),
                        _ => self.uu(s.bbox().width()),
                    },
                    "theLabel" => match &s.geom {
                        Geometry::Label { text, .. } => string(text.as_str()),
                        _ => Value::Nil,
                    },
                    "xy" => match &s.geom {
                        Geometry::Label { at, .. } => self.pt_val(*at),
                        _ => Value::Nil,
                    },
                    "height" => match &s.geom {
                        Geometry::Label { size, .. } => self.uu(*size),
                        _ => self.uu(s.bbox().height()),
                    },
                    "orient" => string("R0"),
                    "justify" => string("lowerLeft"),
                    "area" => Value::Float(
                        s.geom.rects().iter().map(|r| r.width() as f64 * r.height() as f64).sum::<f64>() / self.dbu().powi(2),
                    ),
                    _ => return fail(format!("unknown shape property {prop}")),
                })
            }
            Obj::Inst(cv, id) => {
                let Some(i) = self.layout(cv).and_then(|l| l.instance(id)).cloned() else {
                    return fail("the instance was deleted");
                };
                Ok(match prop {
                    "objType" => string(if i.array.is_some() { "mosaic" } else { "inst" }),
                    "name" => i.name.as_deref().map_or(Value::Nil, string),
                    "libName" => string(i.cell.lib.as_str()),
                    "cellName" => string(i.cell.cell.as_str()),
                    "viewName" => string("layout"),
                    "xy" => self.pt_val((i.trans.x, i.trans.y)),
                    "orient" => string(orient_name(i.trans.rot, i.trans.mirror)),
                    "transform" => list(vec![
                        self.pt_val((i.trans.x, i.trans.y)),
                        string(orient_name(i.trans.rot, i.trans.mirror)),
                        Value::Float(1.0),
                    ]),
                    "bBox" => self.box_val(self.inst_bbox(&i)),
                    "cellView" => Value::Obj(Obj::Cv(cv)),
                    "master" => self.open_cv(i.cell.clone(), false),
                    "rows" => Value::Int(i.dims().1 as i64),
                    "columns" => Value::Int(i.dims().0 as i64),
                    "numInst" => Value::Int(i.count() as i64),
                    _ => return fail(format!("unknown instance property {prop}")),
                })
            }
        }
    }

    fn set_prop(&mut self, o: &Value, prop: &str, v: &Value) -> R<()> {
        let Value::Obj(obj) = o else { return fail(format!("cannot set property {prop} of {}", self.repr(o))) };
        match *obj {
            Obj::Shape(cv, id) => {
                let layer = if matches!(prop, "layerName" | "lpp") { Some(self.layer(v)?) } else { None };
                let r = if prop == "bBox" { Some(self.bbox(v)?) } else { None };
                let pts = if prop == "points" { Some(self.points(v)?) } else { None };
                let xy = if prop == "xy" { Some(self.point(v)?) } else { None };
                let len = if matches!(prop, "width" | "height") { Some(self.to_dbu(as_f64(v))) } else { None };
                let text = self.display(v);
                let l = self.layout_mut(cv)?;
                let Some(s) = l.shape_mut(id) else { return fail("the shape was deleted") };
                match (prop, &mut s.geom) {
                    ("layerName" | "lpp", _) => s.layer = layer.unwrap(),
                    ("bBox", Geometry::Rect(rect)) => *rect = r.unwrap(),
                    ("points", Geometry::Polygon { points } | Geometry::Path { points, .. }) => *points = pts.unwrap(),
                    ("width", Geometry::Path { width, .. }) => *width = len.unwrap(),
                    ("theLabel", Geometry::Label { text: t, .. }) => *t = text,
                    ("xy", Geometry::Label { at, .. }) => *at = xy.unwrap(),
                    ("height", Geometry::Label { size, .. }) => *size = len.unwrap().max(1),
                    _ => return fail(format!("cannot set property {prop} of this shape")),
                }
                Ok(())
            }
            Obj::Inst(cv, id) => {
                let xy = if prop == "xy" { Some(self.point(v)?) } else { None };
                let orient = if prop == "orient" { Some(parse_orient(&self.display(v))?) } else { None };
                let name = self.display(v);
                let l = self.layout_mut(cv)?;
                let Some(i) = l.instance_mut(id) else { return fail("the instance was deleted") };
                match prop {
                    "xy" => (i.trans.x, i.trans.y) = xy.unwrap(),
                    "orient" => (i.trans.rot, i.trans.mirror) = orient.unwrap(),
                    "name" => i.name = Some(name).filter(|n| !n.is_empty() && n != "nil"),
                    _ => return fail(format!("cannot set instance property {prop}")),
                }
                Ok(())
            }
            Obj::Cv(_) => fail(format!("cannot set cellview property {prop}")),
        }
    }
}

impl Value {
    fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }
}

fn set_table(t: &Rc<RefCell<Table>>, k: Value, v: Value) {
    let mut t = t.borrow_mut();
    match t.entries.iter_mut().find(|(x, _)| equal(x, &k)) {
        Some(e) => e.1 = v,
        None => t.entries.push((k, v)),
    }
}

fn arith(op: &str, a: &Value, b: &Value) -> R<Value> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            let r = match op {
                "plus" => x.checked_add(*y),
                "difference" => x.checked_sub(*y),
                "times" => x.checked_mul(*y),
                _ => {
                    if *y == 0 {
                        return fail("division by zero");
                    }
                    x.checked_div(*y)
                }
            };
            r.map(Value::Int).map_or_else(|| fail("integer overflow"), Ok)
        }
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            let (x, y) = (as_f64(a), as_f64(b));
            Ok(Value::Float(match op {
                "plus" => x + y,
                "difference" => x - y,
                "times" => x * y,
                _ => {
                    if y == 0.0 {
                        return fail("division by zero");
                    }
                    x / y
                }
            }))
        }
        (Value::List(p), Value::List(q)) if p.len() == 2 && q.len() == 2 && matches!(op, "plus" | "difference") => {
            Ok(list(vec![arith(op, &p[0], &q[0])?, arith(op, &p[1], &q[1])?]))
        }
        _ => {
            let name = match op {
                "plus" => "+",
                "difference" => "-",
                "times" => "*",
                _ => "/",
            };
            fail(format!("arithmetic {name} needs numbers, got {} and {}", type_name(a), type_name(b)))
        }
    }
}

fn box_coords(v: &Value) -> Option<(f64, f64, f64, f64)> {
    match items(v) {
        [a, b] => match (items(a), items(b)) {
            ([x0, y0], [x1, y1]) => Some((as_f64(x0), as_f64(y0), as_f64(x1), as_f64(y1))),
            _ => None,
        },
        _ => None,
    }
}

/// Cadence orientation: mirror about the x axis (for `MX` / `MXR90` / `MY` / `MYR90`), then
/// rotate counter-clockwise.
fn parse_orient(s: &str) -> R<(u8, bool)> {
    Ok(match s {
        "R0" | "nil" | "" => (0, false),
        "R90" => (1, false),
        "R180" => (2, false),
        "R270" => (3, false),
        "MX" => (0, true),
        "MXR90" => (1, true),
        "MY" => (2, true),
        "MYR90" => (3, true),
        o => return fail(format!("unknown orientation '{o}' (R0 R90 R180 R270 MX MY MXR90 MYR90)")),
    })
}

fn orient_name(rot: u8, mirror: bool) -> &'static str {
    match (rot % 4, mirror) {
        (0, false) => "R0",
        (1, false) => "R90",
        (2, false) => "R180",
        (3, false) => "R270",
        (0, true) => "MX",
        (1, true) => "MXR90",
        (2, true) => "MY",
        _ => "MYR90",
    }
}

fn c_exponent(s: &str) -> String {
    match s.split_once('e') {
        Some((m, e)) => {
            let (sign, digits) = match e.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', e),
            };
            format!("{m}e{sign}{digits:0>2}")
        }
        None => s.to_string(),
    }
}

fn format_g(x: f64, prec: usize) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let p = prec.max(1);
    let exp = x.abs().log10().floor() as i32;
    let trim = |s: String| {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };
    if exp < -4 || exp >= p as i32 {
        let s = format!("{:.*e}", p - 1, x);
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        c_exponent(&format!("{}e{e}", trim(m.to_string())))
    } else {
        trim(format!("{:.*}", (p as i32 - 1 - exp).max(0) as usize, x))
    }
}

/// Minimal regular expression match: `^`, `$`, `.`, `*` and literal characters.
fn simple_match(pat: &str, s: &str) -> bool {
    fn here(p: &[char], s: &[char]) -> bool {
        match p {
            [] => true,
            ['$'] => s.is_empty(),
            [c, '*', rest @ ..] => {
                let mut i = 0;
                loop {
                    if here(rest, &s[i..]) {
                        return true;
                    }
                    if i < s.len() && (*c == '.' || s[i] == *c) {
                        i += 1;
                    } else {
                        return false;
                    }
                }
            }
            [c, rest @ ..] => !s.is_empty() && (*c == '.' || s[0] == *c) && here(rest, &s[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = s.chars().collect();
    if p.first() == Some(&'^') {
        return here(&p[1..], &s);
    }
    (0..=s.len()).any(|i| here(&p, &s[i..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> String {
        let mut p = Project::new("t");
        match run(&mut p, src, None) {
            Ok(r) => r.value,
            Err(e) => panic!("{e}"),
        }
    }

    fn output(src: &str) -> String {
        let mut p = Project::new("t");
        run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}")).output
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("1 + 2 * 3"), "7");
        assert_eq!(eval("(1 + 2) * 3"), "9");
        assert_eq!(eval("7 / 2"), "3");
        assert_eq!(eval("7.0 / 2"), "3.5");
        assert_eq!(eval("2 ** 10"), "1024");
        assert_eq!(eval("(plus 1 2 3)"), "6");
        assert_eq!(eval("-3 + 1"), "-2");
        assert_eq!(eval("1.5u"), "1.5e-06");
        assert_eq!(eval("x = 3 y = x * 2 x + y"), "9");
        assert_eq!(eval("1 < 2 && !(3 > 4)"), "t");
        assert_eq!(eval("0:1.5"), "(0 1.5)");
    }

    #[test]
    fn lists_and_strings() {
        assert_eq!(eval("list(1 2 3)"), "(1 2 3)");
        assert_eq!(eval("'(a b (c))"), "(a b (c))");
        assert_eq!(eval("cadr('(1 2 3))"), "2");
        assert_eq!(eval("append(list(1) '(2 3))"), "(1 2 3)");
        assert_eq!(eval("reverse(list(1 2 3))"), "(3 2 1)");
        assert_eq!(eval("nth(1 '(a b c))"), "b");
        assert_eq!(eval("member(2 '(1 2 3))"), "(2 3)");
        assert_eq!(eval("sort(list(3 1 2) 'lessp)"), "(1 2 3)");
        assert_eq!(eval("sort(list(3 1 2) lambda((a b) a > b))"), "(3 2 1)");
        assert_eq!(eval("strcat(\"a\" \"b\" \"c\")"), "\"abc\"");
        assert_eq!(eval("parseString(\"a b  c\")"), "(\"a\" \"b\" \"c\")");
        assert_eq!(eval("sprintf(nil \"%d-%05.2f-%s\" 3 2.5 \"x\")"), "\"3-02.50-x\"");
        assert_eq!(eval("buildString(list(\"a\" \"b\") \",\")"), "\"a,b\"");
        assert_eq!(eval("p = list(nil 'w 1) p->w = 5 p->l = 2 list(p->w p->l)"), "(5 2)");
        assert_eq!(eval("mapcar(lambda((x) x * x) '(1 2 3))"), "(1 4 9)");
        assert_eq!(eval("setof(x '(1 2 3 4) evenp(x))"), "(2 4)");
    }

    #[test]
    fn control_flow_and_procedures() {
        let src = r#"
            procedure(fact(n)
              if(n <= 1 then 1 else n * fact(n - 1)))
            (defun sq (x) (times x x))
            procedure(opt(a @optional (b 10) @key (c 100)) a + b + c)
            list(fact(10) sq(4) opt(1) opt(1 2 ?c 3))
        "#;
        assert_eq!(eval(src), "(3628800 16 111 6)");
        assert_eq!(eval("let(((s 0)) for(i 1 10 s = s + i) s)"), "55");
        assert_eq!(eval("prog((i) i = 0 while(t i++ when(i > 4 return(i))))"), "5");
        assert_eq!(eval("cond((nil 1) ((1 > 0) 2) (t 3))"), "2");
        assert_eq!(eval("case(\"b\" (\"a\" 1) ((\"b\" \"c\") 2) (t 3))"), "2");
        assert_eq!(eval("case('x (y 1) (x 2))"), "2");
        assert_eq!(eval("foreach(mapcar x '(1 2) x + 1)"), "(2 3)");
        assert_eq!(eval("errset(error(\"boom\"))"), "nil");
        assert_eq!(eval("tbl = makeTable(\"t\" 0) tbl[\"a\"] = 5 list(tbl[\"a\"] tbl[\"b\"] length(tbl))"), "(5 0 1)");
        assert_eq!(eval("if(nil 1 2)"), "2");
        assert_eq!(eval("unless(nil 7)"), "7");
        assert_eq!(output("foreach((a b) '(1 2) '(3 4) printf(\"%d \" a * b))"), "3 8 ");
    }

    #[test]
    fn printing() {
        assert_eq!(output("printf(\"%s=%d %L\\n\" \"n\" 4 list(1 \"x\"))"), "n=4 (1 \"x\")\n");
        assert_eq!(output("println(list(1.0 'a))"), "(1.0 a)\n");
        assert_eq!(output("printf(\"%g %e\" 0.000125 1500.0)"), "0.000125 1.500000e+03");
    }

    #[test]
    fn errors_have_lines() {
        let mut p = Project::new("t");
        let e = run(&mut p, "x = 1\nprintf(\"hi\\n\")\nfoo(x)\n", None).unwrap_err();
        assert_eq!(e.line, 3);
        assert!(e.message.contains("undefined function foo"), "{}", e.message);
        assert_eq!(e.output, "hi\n");
        let e = run(&mut p, "list(1\n2", None).unwrap_err();
        assert!(e.message.contains("end of input"));
        let e = run(&mut p, "procedure(f(x)\n  x + y)\nf(1)", None).unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.message.contains("unbound variable y"));
        let e = run(&mut p, "procedure(f(x) f(x)) f(1)", None).unwrap_err();
        assert!(e.message.contains("depth"));
    }

    #[test]
    fn layout_generation() {
        let mut p = Project::new("t");
        let src = r#"
            ; a contact row and a placed copy
            cv = dbOpenCellViewByType("gen" "pad" "layout" "maskLayout" "w")
            dbCreateRect(cv list("metal1" "drawing") list(0:0 2:1.5))
            dbCreatePolygon(cv "metal2" list(0:0 1:0 1:1))
            dbCreatePath(cv "poly" list(0:0 0:5) 0.18)
            dbCreateLabel(cv "text" 1:0.5 "VDD" "centerCenter" "R0" "roman" 0.2)
            for(i 0 2 dbCreateRect(cv "contact" list(i*0.5:0 i*0.5+0.22:0.22)))
            dbSave(cv)
            top = dbOpenCellViewByType("gen" "top" "layout" "maskLayout" "w")
            inst = dbCreateInstByMasterName(top "gen" "pad" "layout" "I0" 10:0 "R90")
            dbCreateSimpleMosaic(top dbOpenCellViewByType("gen" "pad" "layout") "A0" 0:20 "R0" 2 3 4.0 5.0)
            printf("%d shapes, inst %s at %L %s\n" length(cv~>shapes) inst~>name inst~>xy inst~>orient)
            list(length(top~>instances) car(cv~>shapes)~>layerName cv~>bBox)
        "#;
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.output, "7 shapes, inst I0 at (10.0 0.0) R90\n");
        assert_eq!(r.value, "(2 \"metal1\" ((-0.09 -0.09) (2.0 5.09)))");
        assert_eq!(r.modified, vec![CellRef::new("gen", "pad"), CellRef::new("gen", "top")]);
        let pad = p.cell(&CellRef::new("gen", "pad")).unwrap().layout.as_ref().unwrap();
        assert_eq!(pad.shapes.len(), 7);
        assert_eq!(pad.shapes[0].geom, Geometry::Rect(IRect::new(0, 0, 2000, 1500)));
        assert!(matches!(&pad.shapes[2].geom, Geometry::Path { width: 180, .. }));
        let top = p.cell(&CellRef::new("gen", "top")).unwrap().layout.as_ref().unwrap();
        assert_eq!(top.instances[0].trans, Trans::with(10000, 0, 1, false));
        assert_eq!(top.instances[0].name.as_deref(), Some("I0"));
        assert_eq!(top.instances[1].array, Some(ArraySpec { cols: 3, rows: 2, col: (5000, 0), row: (0, 4000) }));
    }

    #[test]
    fn editing_existing_layout() {
        let mut p = Project::new("t");
        let src = r#"
            cv = dbOpenCellViewByType("lib" "c" "layout" "maskLayout" "w")
            r = dbCreateRect(cv "metal1" list(0:0 1:1))
            r~>bBox = list(0:0 3:1)
            r2 = dbCopyFig(r cv list(5:0 "R0"))
            dbMoveFig(r cv list(0:2 "R0"))
            dbDeleteObject(r2)
            ro = dbOpenCellViewByType("lib" "c" "layout")
            mapcar(lambda((s) s~>bBox) ro~>shapes)
        "#;
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "(((0.0 2.0) (3.0 3.0)))");
        assert_eq!(eval("dbOpenCellViewByType(\"x\" \"y\" \"layout\")"), "nil");
        let e = run(
            &mut p,
            "cv = dbOpenCellViewByType(\"lib\" \"c\" \"layout\" \"\" \"r\")\ndbCreateRect(cv \"metal1\" list(0:0 1:1))",
            None,
        );
        let e = e.unwrap_err();
        assert!(e.message.contains("read-only"), "{}", e.message);
        let e = run(
            &mut p,
            "cv = dbOpenCellViewByType(\"lib\" \"c\" \"layout\" \"\" \"a\") dbCreateRect(cv \"nolayer\" list(0:0 1:1))",
            None,
        );
        assert!(e.unwrap_err().message.contains("unknown layer 'nolayer'"));
    }

    #[test]
    fn edit_cellview_and_load() {
        let dir = std::env::temp_dir().join(format!("ics_skill_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.il"), "procedure(box(cv x) dbCreateRect(cv \"metal1\" list(x:0 x+1:1)))\n").unwrap();
        std::fs::write(dir.join("main.il"), "load(\"lib.il\")\ncv = geGetEditCellView()\nbox(cv 0) box(cv 2)\n").unwrap();
        let mut p = Project::new("t");
        if p.libraries.iter().all(|l| l.name != "work") {
            p.add_library("work").unwrap();
        }
        let cell = p.add_cell("work", Cell::new("top")).unwrap();
        let r = run_file_with(&mut p, &dir.join("main.il"), Some(&cell)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.modified, vec![cell.clone()]);
        assert_eq!(p.cell(&cell).unwrap().layout.as_ref().unwrap().shapes.len(), 2);
        std::fs::write(dir.join("bad.il"), "\n\nundefinedThing()\n").unwrap();
        std::fs::write(dir.join("main2.il"), "load(\"bad.il\")\n").unwrap();
        let e = run_file(&mut p, &dir.join("main2.il")).unwrap_err();
        assert_eq!(e.line, 3);
        assert!(e.file.unwrap().ends_with("bad.il"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_keeps_state() {
        let mut p = Project::new("t");
        let mut s = Session::new();
        s.eval(&mut p, "x = 2 procedure(dbl(v) v * 2)", None, None).unwrap();
        assert_eq!(s.eval(&mut p, "dbl(x)", None, None).unwrap().value, "4");
        s.eval(&mut p, "cv = dbOpenCellViewByType(\"work\" \"c\" \"layout\" \"\" \"w\")", None, None).unwrap();
        let r = s.eval(&mut p, "dbCreateRect(cv \"metal1\" list(0:0 1:1)) length(cv~>shapes)", None, None).unwrap();
        assert_eq!((r.value.as_str(), r.modified.len()), ("1", 1));
        assert!(s.eval(&mut p, "undefinedFn()", None, None).is_err());
        assert_eq!(s.eval(&mut p, "x", None, None).unwrap().value, "2");
        assert_eq!(s.procedures(), vec!["dbl".to_string()]);
    }

    #[test]
    fn completeness() {
        assert!(is_complete("list(1 2)"));
        assert!(!is_complete("procedure(f(x)"));
        assert!(!is_complete("printf(\"a"));
        assert!(!is_complete("/* open"));
        assert!(is_complete("x = 1)"));
        assert!(is_complete(""));
    }
}
