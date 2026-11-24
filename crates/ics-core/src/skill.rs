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
//!   (no-ops: the caller saves the project) and `geGetEditCellView` for the cell open in the editor;
//!   layer Boolean operations `dbLayerAnd` / `dbLayerOr` / `dbLayerXor` / `dbLayerAndNot`
//!   `(cv layer shapes1 shapes2)` and `dbLayerSize(cv layer shapes size)`, exact for any-angle
//!   geometry (see [`crate::polybool`]); `dbCreateParamInst` / `dbCreateParamInstByMasterName` on the
//!   built-in PCells (`nmos`, `pmos`, `poly_res`, `mom_cap`, `ntap`, `ptap`, `nring`, `pring`, with
//!   CDF-style `list(name type value)` parameters), `dbCreateVia` with `techFindViaDefByName`, nets,
//!   terms and pins (`dbCreateNet`, `dbMakeNet`, `dbFindNetByName`, `dbCreateTerm`, `dbCreatePin`,
//!   `dbAddFigToNet`: a pin is labelled with its net), properties (`dbReplaceProp`,
//!   `dbCreateProp`, `dbGetPropByName`, read back with `~>`);
//! - ROD: `rodCreateRect`, `rodCreatePath`, `rodCreatePolygon` with keyword arguments;
//! - SKILL PCells: `pcDefinePCell(list(ddGetObj(lib) cell "layout") ((name default) ...) body)`
//!   stores the definition on the master cell (evaluated at the defaults); each
//!   `dbCreateParamInst` parameter set gets its own variant cell in the master's library, made by
//!   running the body with `pcCellView` and the parameters bound;
//! - files: `outfile`, `infile`, `fprintf` to a port, `gets`, `close`; arrays with `declare`;
//! - forms (`hi*`) without a window: fields and forms are tables (`form->field->value`), and
//!   `hiDisplayForm` runs the OK callback at once with the current values;
//! - SKILL++: in `.ils` files, `lambda` and `procedure` make closures over the variables in
//!   scope where they are created (shared with that scope and the other closures made there,
//!   kept between calls), which do not see their callers' variables;
//! - limits: evaluation runs on its own thread with a large stack; recursion, nesting, steps,
//!   string and list sizes are bounded and end in SKILL errors. A [`Sandbox`] (used for agent
//!   code) also confines file access to given directories and hides environment variables.
//!
//! Coordinates are user units (µm), converted to database units with the technology's
//! `dbu_per_um`. A layer-purpose pair is a layer name, `list("metal1" "drawing")` or a layer index;
//! a purpose other than `drawing` first looks for a layer named `metal1.pin` / `metal1_pin`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::geom::IRect;
use crate::layout::{ArraySpec, CellInst, Geometry, Layout, Trans};
use crate::polybool;
use crate::project::{Cell, CellRef, Project};
use crate::region::BoolOp;

/// Result of a SKILL run.
#[derive(Debug, Clone, Default)]
pub struct RunResult {
    /// Everything the script printed (`printf`, `println`, `print`, ...).
    pub output: String,
    /// The value of the last top-level form, printed SKILL style.
    pub value: String,
    /// Cells whose layout the script created or changed, in first-touched order.
    pub modified: Vec<CellRef>,
    /// Forms the script displayed in an interactive session ([`Session::interactive`]), for the
    /// application to show; [`Session::form_ok`] runs their OK callback.
    pub forms: Vec<FormRequest>,
}

/// A form for the user to fill in (`hiDisplayForm` in an interactive session).
#[derive(Debug, Clone, PartialEq)]
pub struct FormRequest {
    /// Global variable holding the form.
    pub symbol: String,
    pub title: String,
    pub fields: Vec<FormField>,
}

/// A field of a [`FormRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    pub name: String,
    pub prompt: String,
    pub kind: FieldKind,
    /// Current value, as text (strings without quotes; booleans `t` / `nil`).
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    String,
    Float,
    Int,
    Bool,
    /// One of the choices.
    Choice(Vec<String>),
    /// Shown, not edited (labels, buttons).
    Label,
}

impl FormField {
    /// The value as a SKILL expression.
    fn skill_value(&self) -> String {
        match &self.kind {
            FieldKind::Float | FieldKind::Int => {
                let t = self.value.trim();
                if t.parse::<f64>().is_ok() { t.to_string() } else { "nil".into() }
            }
            FieldKind::Bool => {
                if matches!(self.value.trim(), "t" | "true" | "1") {
                    "t".into()
                } else {
                    "nil".into()
                }
            }
            _ => format!("\"{}\"", self.value.replace('\\', "\\\\").replace('"', "\\\"")),
        }
    }
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

/// Restrictions for running untrusted SKILL (e.g. code from an AI agent): file I/O only under
/// `roots`, no environment variables, an optional wall-clock limit.
#[derive(Debug, Clone, Default)]
pub struct Sandbox {
    /// Directories that `load`, `outfile` and `infile` may access (with their subdirectories).
    /// Relative paths are taken relative to the first one. Empty: no file access at all.
    pub roots: Vec<PathBuf>,
    /// Stop an evaluation that runs longer than this.
    pub time_limit: Option<std::time::Duration>,
}

/// An interpreter whose global variables, procedures, open cellviews, properties and file
/// ports persist between evaluations, like Virtuoso's CIW.
///
/// The interpreter runs on a thread of its own with a large stack (deep but bounded recursion in
/// scripts ends in a SKILL error, never a stack overflow, also on Windows where the main thread
/// has 1 MB). Its values never leave that thread; the project is handed over for each
/// evaluation and handed back with the result. [`Session::start`] runs an evaluation in the
/// background, so an application can stay responsive and offer [`Session::cancel`].
pub struct Session {
    /// Forms are shown to the user: `hiDisplayForm` returns them in [`RunResult::forms`]
    /// instead of running their OK callback at once.
    pub interactive: bool,
    /// Restrict file and environment access (`None`: full access, for the user's own scripts).
    pub sandbox: Option<Sandbox>,
    worker: Option<Worker>,
    /// No evaluation thread could be started: each evaluation runs on the calling thread with
    /// a small stack budget and fresh state.
    no_thread: bool,
    /// A result computed on the calling thread, waiting for [`Session::try_finish`].
    ready: Option<Finished>,
    running: bool,
    cancel: Arc<AtomicBool>,
    procedures: Vec<String>,
}

/// An evaluation for [`Session::start`].
#[derive(Debug, Clone)]
pub enum Task {
    /// Evaluate source; `file` names it for errors and is the base directory of relative
    /// `load()` paths.
    Eval { src: String, file: Option<PathBuf> },
    /// Read and evaluate a `.il` file.
    Load(PathBuf),
    /// OK on a displayed form: set its fields to the edited values and run its callback.
    FormOk(FormRequest, Vec<FormField>),
}

/// The end of an evaluation: the project (with the script's edits) and the result.
#[derive(Debug)]
pub struct Finished {
    /// `None` only if the evaluation thread died and took the project with it.
    pub project: Option<Project>,
    pub result: Result<RunResult, SkillError>,
}

struct Job {
    project: Project,
    task: Task,
    current: Option<CellRef>,
    interactive: bool,
    sandbox: Option<Sandbox>,
}

struct Done {
    finished: Finished,
    procedures: Vec<String>,
}

struct Worker {
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Done>,
}

/// Stack of the evaluation thread.
const EVAL_STACK: usize = 256 << 20;

/// Roughly the current stack position, to measure how deep evaluation has gone.
#[inline(never)]
fn stack_pos() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

fn worker_main(jobs: mpsc::Receiver<Job>, done: mpsc::Sender<Done>, cancel: Arc<AtomicBool>) {
    let mut state = State::new();
    for mut job in jobs {
        state.interactive = job.interactive;
        state.sandbox = job.sandbox.take();
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.run_task(&mut job.project, &job.task, job.current.as_ref(), EVAL_STACK - (32 << 20), &cancel)
        }));
        let result = run.unwrap_or_else(|panic| {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            // The interpreter state is lost with the panic: start over.
            state = State::new();
            Err(SkillError::plain(format!("internal interpreter error: {msg}"), None))
        });
        let procedures = state.procedures();
        if done.send(Done { finished: Finished { project: Some(job.project), result }, procedures }).is_err() {
            break;
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session {
            interactive: false,
            sandbox: None,
            worker: None,
            no_thread: false,
            ready: None,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            procedures: vec![],
        }
    }

    /// A session that shows forms to the user ([`Session::interactive`]).
    pub fn interactive() -> Self {
        let mut s = Session::new();
        s.interactive = true;
        s
    }

    /// A session restricted by `sandbox` ([`Session::sandbox`]).
    pub fn sandboxed(sandbox: Sandbox) -> Self {
        let mut s = Session::new();
        s.sandbox = Some(sandbox);
        s
    }

    /// Start evaluating `task` on `project` in the background; [`Session::try_finish`] or
    /// [`Session::wait`] hand the project back. `current` is the cell `geGetEditCellView()`
    /// returns. Panics if an evaluation is still running.
    pub fn start(&mut self, project: Project, task: Task, current: Option<CellRef>) {
        assert!(!self.running, "a SKILL evaluation is already running");
        self.running = true;
        self.cancel.store(false, Ordering::SeqCst);
        let job = Job { project, task, current, interactive: self.interactive, sandbox: self.sandbox.clone() };
        if !self.no_thread && self.worker.is_none() {
            let (tx, jobs) = mpsc::channel();
            let (done, rx) = mpsc::channel();
            let cancel = self.cancel.clone();
            let spawned = std::thread::Builder::new()
                .name("skill".into())
                .stack_size(EVAL_STACK)
                .spawn(move || worker_main(jobs, done, cancel));
            match spawned {
                Ok(_) => self.worker = Some(Worker { tx, rx }),
                Err(_) => self.no_thread = true,
            }
        }
        let job = match &self.worker {
            Some(w) => match w.tx.send(job) {
                Ok(()) => return,
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        // No evaluation thread: run here, on the caller's stack.
        self.worker = None;
        self.no_thread = true;
        let mut state = State::new();
        let Job { mut project, task, current, interactive, sandbox } = job;
        state.interactive = interactive;
        state.sandbox = sandbox;
        let result = state.run_task(&mut project, &task, current.as_ref(), 256 << 10, &self.cancel);
        self.procedures = state.procedures();
        self.ready = Some(Finished { project: Some(project), result });
    }

    /// The finished evaluation, if it has ended.
    pub fn try_finish(&mut self) -> Option<Finished> {
        self.finish(Some(std::time::Duration::ZERO))
    }

    /// Wait up to `timeout` for the running evaluation.
    pub fn wait_timeout(&mut self, timeout: std::time::Duration) -> Option<Finished> {
        self.finish(Some(timeout))
    }

    /// Wait for the running evaluation to end. Panics if none is running.
    pub fn wait(&mut self) -> Finished {
        self.finish(None).expect("a SKILL evaluation is running")
    }

    fn finish(&mut self, timeout: Option<std::time::Duration>) -> Option<Finished> {
        if !self.running {
            return None;
        }
        if let Some(f) = self.ready.take() {
            self.running = false;
            return Some(f);
        }
        let w = self.worker.as_ref()?;
        let done = match timeout {
            None => w.rx.recv().ok(),
            Some(t) if t.is_zero() => w.rx.try_recv().ok(),
            Some(t) => w.rx.recv_timeout(t).ok(),
        };
        let done = match done {
            Some(d) => d,
            None if self.worker_gone() => {
                // The thread ended without answering: the project went with it.
                self.running = false;
                self.worker = None;
                return Some(Finished {
                    project: None,
                    result: Err(SkillError::plain("the SKILL evaluation thread stopped".into(), None)),
                });
            }
            None => return None,
        };
        self.running = false;
        self.procedures = done.procedures;
        Some(done.finished)
    }

    fn worker_gone(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| matches!(w.rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)))
    }

    /// Whether an evaluation started with [`Session::start`] has not been collected yet.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Ask the running evaluation to stop; it ends with a "cancelled" error soon after.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Run `task` to the end on `project`.
    fn run(&mut self, project: &mut Project, task: Task, current: Option<&CellRef>) -> Result<RunResult, SkillError> {
        let owned = std::mem::replace(project, Project::new(""));
        self.start(owned, task, current.cloned());
        let f = self.wait();
        if let Some(p) = f.project {
            *project = p;
        }
        f.result
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
        self.run(project, Task::Eval { src: src.to_string(), file: file.map(Path::to_path_buf) }, current)
    }

    /// OK on a displayed form: set its fields to `fields` (as edited) and run its callback.
    pub fn form_ok(
        &mut self,
        project: &mut Project,
        form: &FormRequest,
        fields: &[FormField],
        current: Option<&CellRef>,
    ) -> Result<RunResult, SkillError> {
        self.run(project, Task::FormOk(form.clone(), fields.to_vec()), current)
    }

    /// Read and evaluate a `.il` file.
    pub fn load(&mut self, project: &mut Project, path: &Path, current: Option<&CellRef>) -> Result<RunResult, SkillError> {
        self.run(project, Task::Load(path.to_path_buf()), current)
    }

    /// Names of the procedures defined so far, sorted.
    pub fn procedures(&self) -> Vec<String> {
        self.procedures.clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A running evaluation stops soon; the thread ends when its job channel closes.
        self.cancel();
    }
}

impl SkillError {
    fn plain(message: String, file: Option<String>) -> Self {
        SkillError { message, file, line: 0, output: String::new(), modified: vec![] }
    }
}

/// The interpreter state of a [`Session`]; it lives on the evaluation thread only.
#[derive(Default)]
struct State {
    globals: HashMap<Name, Value>,
    funcs: HashMap<Name, Rc<Func>>,
    cvs: Vec<CvState>,
    files: Vec<Option<PathBuf>>,
    ports: Vec<PortFile>,
    props: HashMap<(usize, u64), Vec<(String, Value)>>,
    interactive: bool,
    sandbox: Option<Sandbox>,
}

impl State {
    fn new() -> Self {
        let globals = ["poport", "errport", "woport", "piport"].into_iter().map(|p| (p.into(), Value::Port(0))).collect();
        State { globals, ..Default::default() }
    }

    fn procedures(&self) -> Vec<String> {
        let mut v: Vec<String> = self.funcs.keys().map(|k| k.to_string()).collect();
        v.sort();
        v
    }

    fn run_task(
        &mut self,
        project: &mut Project,
        task: &Task,
        current: Option<&CellRef>,
        stack_budget: usize,
        cancel: &Arc<AtomicBool>,
    ) -> Result<RunResult, SkillError> {
        match task {
            Task::Eval { src, file } => self.eval_here(project, src, file.as_deref(), current, stack_budget, cancel),
            Task::FormOk(form, fields) => {
                let mut src = String::new();
                for f in fields.iter().filter(|f| f.kind != FieldKind::Label) {
                    src += &format!("{}->{}->value = {}\n", form.symbol, f.name, f.skill_value()); // i18n: ignore
                }
                src += &format!("hiiFormOk({})\n", form.symbol); // i18n: ignore
                self.eval_here(project, &src, None, current, stack_budget, cancel)
            }
            Task::Load(path) => {
                let err = |message: String| SkillError::plain(message, Some(path.display().to_string()));
                let path = match &self.sandbox {
                    Some(sb) => sandbox_path(sb, path).map_err(err)?,
                    None => path.to_path_buf(),
                };
                let src = std::fs::read_to_string(&path).map_err(|e| err(e.to_string()))?;
                self.eval_here(project, &src, Some(&path), current, stack_budget, cancel)
            }
        }
    }

    /// Evaluate on the current thread, using at most about `stack_budget` bytes of its stack for
    /// nested evaluation.
    fn eval_here(
        &mut self,
        project: &mut Project,
        src: &str,
        file: Option<&Path>,
        current: Option<&CellRef>,
        stack_budget: usize,
        cancel: &Arc<AtomicBool>,
    ) -> Result<RunResult, SkillError> {
        let interactive = self.interactive;
        let mut it = Interp::new(project, current.cloned(), std::mem::take(self));
        it.stack_base = stack_pos();
        it.stack_budget = stack_budget;
        it.interactive = interactive;
        it.cancel = Some(cancel.clone());
        let r = it.run_source(src, file.map(Path::to_path_buf));
        let modified = std::mem::take(&mut it.modified);
        let output = std::mem::take(&mut it.out);
        let forms = std::mem::take(&mut it.forms);
        let res = match r {
            Ok(v) | Err(Flow::Return(v)) => Ok(RunResult { output, value: it.repr(&v), modified, forms }),
            Err(Flow::Err(e)) => Err(SkillError {
                message: e.msg,
                file: e.pos.and_then(|p| it.files.get(p.file as usize).cloned().flatten()).map(|p| p.display().to_string()),
                line: e.pos.map_or(0, |p| p.line as usize),
                output,
                modified,
            }),
            Err(Flow::NotSpecial) => unreachable!("not a special form"),
        };
        *self = it.into_state();
        self.interactive = interactive;
        res
    }
}

/// `path` if `sandbox` allows access to it: resolved (relative paths against the first root,
/// symbolic links followed) and inside one of the roots.
fn sandbox_path(sandbox: &Sandbox, path: &Path) -> Result<PathBuf, String> {
    let denied = || format!("access to {} is not allowed here (only files under the project directory)", path.display());
    let full = match (path.is_relative(), sandbox.roots.first()) {
        (true, Some(root)) => root.join(path),
        (true, None) => return Err(denied()),
        _ => path.to_path_buf(),
    };
    let resolved = match full.canonicalize() {
        Ok(p) => p,
        // A file to create: its directory must exist and be allowed.
        Err(_) => match (full.parent(), full.file_name()) {
            (Some(dir), Some(name)) => dir.canonicalize().map_err(|_| denied())?.join(name),
            _ => return Err(denied()),
        },
    };
    if sandbox.roots.iter().filter_map(|r| r.canonicalize().ok()).any(|r| resolved.starts_with(r)) {
        Ok(resolved)
    } else {
        Err(denied())
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
    List(Rc<ListData>),
    Func(Rc<Func>),
    Obj(Obj),
    Table(Rc<RefCell<Table>>),
    /// Output / input port: 0 is the console, others are files opened by `outfile` / `infile`.
    Port(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Obj {
    Cv(usize),
    Shape(usize, u64),
    Inst(usize, u64),
}

/// The elements of a list.
#[derive(Debug)]
struct ListData(Vec<Value>);

impl std::ops::Deref for ListData {
    type Target = Vec<Value>;
    fn deref(&self) -> &Vec<Value> {
        &self.0
    }
}

#[derive(Debug, Default)]
struct Table {
    name: String,
    default: Option<Value>,
    entries: Vec<(Value, Value)>,
}

/// Local variables of a `let`, a loop or a function call; shared with the SKILL++ closures
/// created in it.
type Frame = Rc<RefCell<HashMap<Name, Value>>>;

fn frame(vars: HashMap<Name, Value>) -> Frame {
    Rc::new(RefCell::new(vars))
}

fn frame_new() -> Frame {
    frame(HashMap::new())
}

#[derive(Debug)]
struct Func {
    name: Name,
    params: Params,
    body: Vec<Node>,
    /// SKILL++ (`.ils`) closure: the variable frames it was created in, shared with that scope
    /// and the other closures made there. A closure sees these and the globals, not its
    /// caller's variables.
    env: Option<Vec<Frame>>,
}

// Values nest without limit (a list in a list in a list ...), so their parts are dropped with a
// loop instead of recursion, which could overflow the stack.

fn nests(v: &Value) -> bool {
    matches!(v, Value::List(_) | Value::Table(_) | Value::Func(_))
}

/// Drop `stack`, taking apart the lists, tables and functions it holds the last reference to.
fn drop_values(mut stack: Vec<Value>) {
    while let Some(v) = stack.pop() {
        match v {
            Value::List(l) => {
                if let Ok(mut l) = Rc::try_unwrap(l) {
                    stack.append(&mut l.0);
                }
            }
            Value::Table(t) => {
                if let Ok(t) = Rc::try_unwrap(t) {
                    let mut t = t.into_inner();
                    stack.extend(t.default.take());
                    for (k, x) in t.entries.drain(..) {
                        stack.push(k);
                        stack.push(x);
                    }
                }
            }
            Value::Func(f) => {
                if let Ok(mut f) = Rc::try_unwrap(f) {
                    for fr in f.env.take().into_iter().flatten() {
                        if let Ok(fr) = Rc::try_unwrap(fr) {
                            stack.extend(fr.into_inner().into_values());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Drop for ListData {
    fn drop(&mut self) {
        if self.0.iter().any(nests) {
            drop_values(std::mem::take(&mut self.0));
        }
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        let mut stack: Vec<Value> = self.default.take().into_iter().collect();
        for (k, x) in self.entries.drain(..) {
            stack.push(k);
            stack.push(x);
        }
        if stack.iter().any(nests) {
            drop_values(stack);
        }
    }
}

impl Drop for Func {
    fn drop(&mut self) {
        let mut stack = vec![];
        for fr in self.env.take().into_iter().flatten() {
            if let Ok(fr) = Rc::try_unwrap(fr) {
                stack.extend(fr.into_inner().into_values());
            }
        }
        if stack.iter().any(nests) {
            drop_values(stack);
        }
    }
}

#[derive(Debug, Default)]
struct Params {
    req: Vec<Name>,
    opt: Vec<(Name, Option<Node>)>,
    key: Vec<(Name, Option<Node>)>,
    rest: Option<Name>,
}

fn list(v: Vec<Value>) -> Value {
    if v.is_empty() { Value::Nil } else { Value::List(Rc::new(ListData(v))) }
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

/// Structural equality, with a loop instead of recursion (lists nest without limit).
fn equal(a: &Value, b: &Value) -> bool {
    let mut todo = vec![(a, b)];
    while let Some((a, b)) = todo.pop() {
        let same = match (a, b) {
            (Value::Nil, Value::Nil) | (Value::T, Value::T) => true,
            (Value::Port(x), Value::Port(y)) => x == y,
            (Value::Int(x), Value::Int(y)) => x == y,
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => as_f64(a) == as_f64(b),
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::Sym(x), Value::Sym(y)) => x == y,
            (Value::List(x), Value::List(y)) if Rc::ptr_eq(x, y) => true,
            (Value::List(x), Value::List(y)) => {
                todo.extend(x.iter().zip(y.iter()));
                x.len() == y.len()
            }
            (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
            (Value::Obj(x), Value::Obj(y)) => x == y,
            (Value::Table(x), Value::Table(y)) => Rc::ptr_eq(x, y),
            _ => false,
        };
        if !same {
            return false;
        }
    }
    true
}

fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::List(x), Value::List(y)) => Rc::ptr_eq(x, y),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y) || x == y,
        _ => equal(a, b),
    }
}

/// A string literal, quoted and escaped.
fn write_str(x: &str, s: &mut String) {
    s.push('"');
    for c in x.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c => s.push(c),
        }
    }
    s.push('"');
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
        Value::Port(_) => "port",
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
    /// From [`Interp::special`] only: the name is not a special form.
    NotSpecial,
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
    /// Nesting of the expression being parsed.
    depth: usize,
}

/// Deepest nesting of expressions and quoted data the parser accepts (and `eval` converts).
const MAX_NESTING: usize = 1000;

/// Height of the tree under `n` (quoted data included), without recursion.
fn node_depth(n: &Node) -> usize {
    enum Item<'a> {
        N(&'a Node),
        V(&'a Value),
    }
    let mut max = 0;
    let mut todo = vec![(Item::N(n), 1)];
    while let Some((it, d)) = todo.pop() {
        max = max.max(d);
        match it {
            Item::N(Node::Call(_, args, _) | Node::Form(args, _)) => todo.extend(args.iter().map(|a| (Item::N(a), d + 1))),
            Item::N(Node::Get(o, _, _)) => todo.push((Item::N(o), d + 1)),
            Item::N(Node::Index(o, i, _)) => todo.extend([(Item::N(o), d + 1), (Item::N(i), d + 1)]),
            Item::N(Node::Lit(v)) | Item::V(v) => {
                if let Value::List(l) = v {
                    todo.extend(l.iter().map(|x| (Item::V(x), d + 1)));
                }
            }
            Item::N(_) => {}
        }
    }
    max
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

    /// Run `f` one nesting level deeper.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        if self.depth >= MAX_NESTING {
            return self.err(format!("expression nested too deeply (more than {MAX_NESTING} levels)"));
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    /// Check that a node of height `h` built at this level stays within [`MAX_NESTING`].
    fn check_height(&self, h: usize) -> R<()> {
        if self.depth + h > MAX_NESTING {
            return self.err(format!("expression nested too deeply (more than {MAX_NESTING} levels)"));
        }
        Ok(())
    }

    fn expr(&mut self, min: u8) -> R<Node> {
        self.nested(|s| s.expr_inner(min))
    }

    fn expr_inner(&mut self, min: u8) -> R<Node> {
        let mut lhs = self.unary()?;
        let mut h = 0;
        while let Tok::Op(op) = *self.peek() {
            let Some((prec, name)) = binary(op) else { break };
            if prec < min {
                break;
            }
            let pos = self.pos();
            self.next();
            let right_assoc = matches!(op, "=" | "**");
            let rhs = self.expr(if right_assoc { prec } else { prec + 1 })?;
            // A chain `a + b + c ...` deepens the tree without recursing here.
            if h == 0 {
                h = node_depth(&lhs);
            }
            h = 1 + h.max(node_depth(&rhs));
            self.check_height(h)?;
            lhs = Node::Call(name.into(), vec![lhs, rhs], pos);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> R<Node> {
        self.nested(|s| s.unary_inner())
    }

    fn unary_inner(&mut self) -> R<Node> {
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
        let mut h = 0;
        loop {
            let pos = self.pos();
            // `a~>b~>c ...` and `a[1][2] ...` deepen the tree without recursing here.
            if !matches!(self.peek(), Tok::Op("~>" | "->" | "++" | "--") | Tok::LBracket) {
                return Ok(e);
            }
            if h == 0 {
                h = node_depth(&e);
            }
            h += 1;
            self.check_height(h)?;
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
        self.nested(|s| s.datum_inner())
    }

    fn datum_inner(&mut self) -> R<Value> {
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
    Parser { toks: lex(src, file)?, i: 0, depth: 0 }.program()
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

/// Code as data: `f(a 'b)` is `(f a (quote b))`, `x~>p` is `(getq x p)` and `x[i]` is
/// `(arrayref x i)`; [`datum_to_node`] converts back.
fn node_to_datum(n: &Node) -> Value {
    match n {
        Node::Lit(v @ (Value::Sym(_) | Value::List(_))) => list(vec![sym("quote"), v.clone()]),
        Node::Lit(v) => v.clone(),
        Node::Var(s) => Value::Sym(s.clone()),
        Node::Key(k) => sym(&format!("?{k}")),
        Node::At(a) => sym(&format!("@{a}")),
        Node::Get(o, p, _) => list(vec![sym("getq"), node_to_datum(o), Value::Sym(p.clone())]),
        Node::Index(o, i, _) => list(vec![sym("arrayref"), node_to_datum(o), node_to_datum(i)]),
        Node::Form(v, _) => list(v.iter().map(node_to_datum).collect()),
        Node::Call(f, args, _) => {
            let mut v = vec![Value::Sym(f.clone())];
            v.extend(args.iter().map(node_to_datum));
            list(v)
        }
    }
}

/// A `case` key: literals stand for themselves.
fn case_key(n: &Node) -> Value {
    match n {
        Node::Lit(v) => v.clone(),
        n => node_to_datum(n),
    }
}

fn datum_to_node(v: &Value) -> R<Node> {
    datum_to_node_at(v, 0)
}

fn datum_to_node_at(v: &Value, depth: usize) -> R<Node> {
    if depth >= MAX_NESTING {
        return fail(format!("eval: expression nested too deeply (more than {MAX_NESTING} levels)"));
    }
    let pos = Pos { file: 0, line: 0 };
    let sub = |x: &Value| datum_to_node_at(x, depth + 1);
    Ok(match v {
        Value::Sym(s) => Node::Var(s.clone()),
        Value::List(l) => match (&l[0], l.len()) {
            (Value::Sym(q), 2) if &**q == "quote" => Node::Lit(l[1].clone()),
            (Value::Sym(q), 3) if &**q == "getq" && matches!(l[2], Value::Sym(_)) => {
                let Value::Sym(p) = &l[2] else { unreachable!() };
                Node::Get(Box::new(sub(&l[1])?), p.clone(), pos)
            }
            (Value::Sym(q), 3) if &**q == "arrayref" => Node::Index(Box::new(sub(&l[1])?), Box::new(sub(&l[2])?), pos),
            (Value::Sym(f), _) => Node::Call(f.clone(), l[1..].iter().map(sub).collect::<R<_>>()?, pos),
            _ => Node::Form(l.iter().map(sub).collect::<R<_>>()?, pos),
        },
        v => Node::Lit(v.clone()),
    })
}

/// SKILL source for `n` that parses back to the same tree (for storing PCell bodies).
fn unparse(n: &Node, s: &mut String) -> R<()> {
    fn name(x: &str, s: &mut String) {
        for (i, c) in x.chars().enumerate() {
            if !(if i == 0 { ident_start(c) } else { ident_char(c) }) {
                s.push('\\');
            }
            s.push(c);
        }
    }
    fn datum(v: &Value, s: &mut String) -> R<()> {
        match v {
            Value::Nil => s.push_str("nil"),
            Value::T => s.push('t'),
            Value::Int(i) => s.push_str(&i.to_string()),
            Value::Float(f) if f.is_finite() => s.push_str(&fmt_float(*f)),
            Value::Str(x) => write_str(x, s),
            Value::Sym(x) => match x.strip_prefix(['?', '@']) {
                Some(rest) if rest.chars().next().is_some_and(ident_start) => {
                    s.push_str(&x[..1]);
                    name(rest, s);
                }
                _ => name(x, s),
            },
            Value::List(l) => {
                s.push('(');
                for (i, x) in l.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    datum(x, s)?;
                }
                s.push(')');
            }
            v => return fail(format!("cannot write {} as source", type_name(v))),
        }
        Ok(())
    }
    let seq = |items: &[Node], s: &mut String| -> R<()> {
        for (i, x) in items.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            unparse(x, s)?;
        }
        Ok(())
    };
    // Every node prints as a primary expression, so `~>` and `[]` can follow any of them.
    match n {
        // A negative number after another argument would read as a subtraction.
        Node::Lit(Value::Int(i)) if *i < 0 => s.push_str(&format!("({i})")),
        Node::Lit(Value::Float(f)) if *f < 0.0 && f.is_finite() => s.push_str(&format!("({})", fmt_float(*f))),
        Node::Lit(v @ (Value::Sym(_) | Value::List(_))) => {
            s.push('\'');
            datum(v, s)?;
        }
        Node::Lit(v) => datum(v, s)?,
        Node::Var(x) => name(x, s),
        Node::Key(k) => {
            s.push('?');
            name(k, s);
        }
        Node::At(a) => {
            s.push('@');
            name(a, s);
        }
        Node::Call(f, args, _) => {
            name(f, s);
            s.push('(');
            seq(args, s)?;
            s.push(')');
        }
        // `(-1)` is already how a negative literal prints.
        Node::Form(items, _) if matches!(items.as_slice(), [Node::Lit(Value::Int(i))] if *i < 0) => unparse(&items[0], s)?,
        Node::Form(items, _) if matches!(items.as_slice(), [Node::Lit(Value::Float(f))] if *f < 0.0 && f.is_finite()) => {
            unparse(&items[0], s)?
        }
        Node::Form(items, _) => {
            s.push('(');
            seq(items, s)?;
            s.push(')');
        }
        Node::Get(o, p, _) => {
            unparse(o, s)?;
            s.push_str("~>");
            name(p, s);
        }
        Node::Index(o, i, _) => {
            unparse(o, s)?;
            s.push('[');
            unparse(i, s)?;
            s.push(']');
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------
// Interpreter

/// Deepest nesting of procedure calls.
const MAX_DEPTH: usize = 10_000;
/// Deepest nesting of expression evaluation (calls and the expressions inside them).
const MAX_EVAL_DEPTH: usize = 100_000;
/// Evaluation steps (expressions and loop iterations) per run.
const MAX_STEPS: u64 = 200_000_000;
/// Longest string a script may build or print (bytes).
const MAX_STR: usize = 64 << 20;
/// Longest list `append` and friends may build.
const MAX_LIST: usize = 50_000_000;
/// Widest field and highest precision of a `printf` directive.
const MAX_FORMAT_WIDTH: usize = 10_000;
/// Most rectangles one `rodCreatePath ?subRect` may create.
const MAX_SUBRECTS: usize = 100_000;
/// Largest coordinate or length (dbu) the ROD path arithmetic accepts.
const COORD_LIMIT: i64 = 1 << 40;
/// Most bytes a run may write to files.
const MAX_FILE_BYTES: u64 = 1 << 30;

struct CvState {
    cell: CellRef,
    writable: bool,
}

struct Interp<'p> {
    p: &'p mut Project,
    globals: HashMap<Name, Value>,
    frames: Vec<Frame>,
    funcs: HashMap<Name, Rc<Func>>,
    out: String,
    cvs: Vec<CvState>,
    modified: Vec<CellRef>,
    files: Vec<Option<PathBuf>>,
    load_stack: Vec<Option<PathBuf>>,
    /// Procedure call nesting.
    depth: usize,
    /// Expression evaluation nesting.
    eval_depth: usize,
    /// Stack position where evaluation started, and how much stack it may use from there.
    stack_base: usize,
    stack_budget: usize,
    steps: u64,
    /// Bytes written to files in this run.
    file_bytes: u64,
    /// State of `random`.
    rng: u64,
    /// Restrictions on file and environment access.
    sandbox: Option<Sandbox>,
    /// When a sandboxed run must stop.
    deadline: Option<std::time::Instant>,
    /// Set from outside to stop the evaluation ([`Session::cancel`]).
    cancel: Option<Arc<AtomicBool>>,
    current: Option<CellRef>,
    /// Files opened by `outfile` / `infile`; port k is `ports[k - 1]`.
    ports: Vec<PortFile>,
    /// Properties set with `dbReplaceProp` / `dbCreateProp`, by object.
    props: HashMap<(usize, u64), Vec<(String, Value)>>,
    /// Running SKILL++ (`.ils`) code: new functions are closures.
    lexical: bool,
    /// The form whose callback is running (`hiGetCurrentForm`).
    current_form: Option<Value>,
    /// Show forms to the user instead of running their callback at once.
    interactive: bool,
    /// Forms displayed during this run (interactive sessions).
    forms: Vec<FormRequest>,
}

/// A file port.
enum PortFile {
    /// Output: written through (appended) on every `fprintf`.
    Out(PathBuf),
    /// Input: the lines not read yet.
    In(std::collections::VecDeque<String>),
    Closed,
}

/// A disembodied property list `(nil k1 v1 k2 v2 ...)`.
fn dpl(pairs: &[(&str, Value)]) -> Value {
    let mut v = vec![Value::Nil];
    for (k, x) in pairs {
        v.push(sym(k));
        v.push(x.clone());
    }
    list(v)
}

/// A length from a CDF-style parameter in µm: strings and numbers below 1e-3 are metres.
fn param_um(v: &Value) -> Option<f64> {
    match v {
        Value::Str(s) => crate::units::parse_spice(s).map(|m| m * 1e6),
        Value::Int(_) | Value::Float(_) => {
            let x = as_f64(v);
            Some(if x.abs() < 1e-3 { x * 1e6 } else { x })
        }
        _ => None,
    }
}

struct Args {
    pos: Vec<Value>,
    keys: Vec<(Name, Value)>,
}

impl<'p> Interp<'p> {
    fn new(p: &'p mut Project, current: Option<CellRef>, s: State) -> Self {
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
            eval_depth: 0,
            stack_base: stack_pos(),
            stack_budget: 256 << 10,
            steps: 0,
            file_bytes: 0,
            rng: 0,
            deadline: s.sandbox.as_ref().and_then(|sb| sb.time_limit).map(|t| std::time::Instant::now() + t),
            sandbox: s.sandbox,
            current,
            ports: s.ports,
            props: s.props,
            lexical: false,
            current_form: None,
            interactive: false,
            forms: vec![],
            cancel: None,
        }
    }

    fn into_state(self) -> State {
        State {
            globals: self.globals,
            funcs: self.funcs,
            cvs: self.cvs,
            files: self.files,
            ports: self.ports,
            props: self.props,
            interactive: self.interactive,
            sandbox: self.sandbox,
        }
    }

    /// In SKILL++ code, the variables a new function closes over: the frames in scope, shared.
    fn closure_env(&self) -> Option<Vec<Frame>> {
        self.lexical.then(|| self.frames.clone())
    }

    /// Whether nested evaluation has used up its stack budget.
    fn stack_low(&self) -> bool {
        self.stack_base.abs_diff(stack_pos()) > self.stack_budget
    }

    /// Count an evaluation step (an expression or a loop iteration) against the limits.
    fn tick(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return fail("evaluation step limit exceeded (infinite loop?)");
        }
        if self.steps.is_multiple_of(4096) {
            if self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                return fail("cancelled");
            }
            if let Some(d) = self.deadline
                && std::time::Instant::now() > d
            {
                return fail("evaluation time limit exceeded");
            }
        }
        Ok(())
    }

    /// `path` if the sandbox (if any) allows `f` to access it.
    fn file_path(&self, f: &str, path: &Path) -> R<PathBuf> {
        match &self.sandbox {
            Some(sb) => sandbox_path(sb, path).or_else(|e| fail(format!("{f}: {e}"))),
            None => Ok(path.to_path_buf()),
        }
    }

    /// Append `text` to the output.
    fn emit(&mut self, text: &str) -> R<()> {
        if self.out.len() + text.len() > MAX_STR {
            return fail(format!("output longer than {} MB", MAX_STR >> 20));
        }
        self.out.push_str(text);
        Ok(())
    }

    /// The variable `name` in the innermost frame.
    fn set_local(&mut self, name: Name, v: Value) {
        if let Some(f) = self.frames.last() {
            f.borrow_mut().insert(name, v);
        }
    }

    fn run_source(&mut self, src: &str, file: Option<PathBuf>) -> R<Value> {
        // `.ils` files are SKILL++: lexically scoped closures.
        let ils = file.as_ref().and_then(|f| f.extension()).is_some_and(|e| e.eq_ignore_ascii_case("ils"));
        if ils && !self.lexical {
            self.lexical = true;
            let r = self.run_source_inner(src, file);
            self.lexical = false;
            return r;
        }
        self.run_source_inner(src, file)
    }

    fn run_source_inner(&mut self, src: &str, file: Option<PathBuf>) -> R<Value> {
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
            if let Some(v) = f.borrow().get(name) {
                return Ok(v.clone());
            }
        }
        match self.globals.get(name) {
            Some(v) => Ok(v.clone()),
            None => fail(format!("unbound variable {name}")),
        }
    }

    fn assign(&mut self, name: &Name, v: Value) {
        for f in self.frames.iter().rev() {
            if let Some(slot) = f.borrow_mut().get_mut(name) {
                *slot = v;
                return;
            }
        }
        self.globals.insert(name.clone(), v);
    }

    fn with_frame<T>(&mut self, vars: HashMap<Name, Value>, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        self.frames.push(frame(vars));
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
        self.tick()?;
        if self.eval_depth >= MAX_EVAL_DEPTH || self.stack_low() {
            return fail("expressions nested too deeply (infinite recursion?)");
        }
        self.eval_depth += 1;
        let r = self.eval_node(n);
        self.eval_depth -= 1;
        r
    }

    fn eval_node(&mut self, n: &Node) -> R<Value> {
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
        // A SKILL++ closure runs on its own captured variables instead of the caller's.
        let saved = func.env.as_ref().map(|env| std::mem::replace(&mut self.frames, env.clone()));
        let r = self.with_frame(frame, |s| {
            for (p, d) in defaults {
                let v = match d {
                    Some(n) => s.eval(&n)?,
                    None => Value::Nil,
                };
                s.set_local(p, v);
            }
            s.body(&func.body)
        });
        if let Some(caller) = saved {
            self.frames = caller;
        }
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
        let f = Rc::new(Func { name: name.clone(), params, body: body.to_vec(), env: self.closure_env() });
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
                "pcDefinePCell" => {
                    // pcDefinePCell(list(lib cell "layout") ((name default) ...) body...)
                    argc(2)?;
                    let target = self.eval(&args[0])?;
                    let t = items(&target).to_vec();
                    if t.len() < 2 {
                        return fail("pcDefinePCell: the first argument is list(library cell view)");
                    }
                    let (lib, cell) = (self.display(&t[0]), self.display(&t[1]));
                    let mut params = Vec::new();
                    for p in elems(&args[1]) {
                        let pe = elems(&p);
                        let Some(Node::Var(name)) = pe.first() else {
                            return fail("pcDefinePCell: a parameter is (name default)");
                        };
                        let default = match pe.get(1) {
                            Some(n) => self.eval(n)?,
                            None => Value::Nil,
                        };
                        params.push((name.to_string(), self.repr(&default)));
                    }
                    // The body is kept as source text that parses back to the same code.
                    let mut body = String::new();
                    for (i, n) in args[2..].iter().enumerate() {
                        if i > 0 {
                            body.push('\n');
                        }
                        unparse(n, &mut body).map_err(|e| match e {
                            Flow::Err(ErrInfo { msg, pos }) => Flow::Err(ErrInfo { msg: format!("pcDefinePCell: {msg}"), pos }),
                            e => e,
                        })?;
                    }
                    let def = crate::pcell::PCell::SkillDef { params: params.clone(), body };
                    let r = CellRef::new(&lib, &cell);
                    let old = match self.p.cell(&r) {
                        Some(c) => match &c.pcell {
                            Some(d @ crate::pcell::PCell::SkillDef { .. }) => Some(d.clone()),
                            _ if c.layout.as_ref().is_some_and(|l| !l.is_empty()) => {
                                return fail(format!(
                                    "pcDefinePCell: {lib}/{cell} already has a layout that is not a SKILL PCell; \
                                     delete it or use another cell name"
                                ));
                            }
                            _ => None,
                        },
                        None => None,
                    };
                    if self.p.libraries.iter().all(|l| l.name != lib) {
                        self.p.add_library(&lib).or_else(fail)?;
                    }
                    if self.p.cell(&r).is_none() {
                        self.p.add_cell(&lib, Cell::new(&cell)).or_else(fail)?;
                    }
                    let was_writable = self.cvs.iter().find(|c| c.cell == r).map(|c| c.writable);
                    self.clear_layout(&r)?;
                    if let Some(c) = self.p.cell_mut(&r) {
                        c.pcell = Some(def.clone());
                        c.description = def.summary(1000);
                    }
                    // The master's own layout is the PCell at its defaults.
                    self.eval_skill_pcell(&r, &r, &params)?;
                    // A changed definition: rebuild the variants already made from the old one.
                    if old.is_some_and(|o| o != def) {
                        self.rebuild_variants(&r, &params)?;
                    }
                    let cv = self.open_cv(r, false);
                    if let Ok(i) = self.cv(&cv) {
                        self.cvs[i].writable = was_writable.unwrap_or(false);
                    }
                    Ok(cv)
                }
                "declare" => {
                    // declare(a[n]): an array, kept as a table indexed 0 .. n-1.
                    argc(1)?;
                    let Node::Index(target, size, _) = &args[0] else { return fail("declare: use declare(name[size])") };
                    let n = self.eval(size)?;
                    let t = Value::Table(Rc::new(RefCell::new(Table {
                        name: "array".into(),
                        default: Some(Value::Nil),
                        entries: vec![],
                    })));
                    if let Value::Int(n) = n
                        && n < 0
                    {
                        return fail("declare: negative size");
                    }
                    self.set_target(target, t.clone())?;
                    Ok(t)
                }
                "gets" => {
                    // gets(var port): the next line of an input port (with its newline) into var.
                    argc(2)?;
                    let port = self.eval(&args[1])?;
                    let line = match port {
                        Value::Port(k) if k > 0 => match self.ports.get_mut(k as usize - 1) {
                            Some(PortFile::In(lines)) => lines.pop_front().map(|l| string(format!("{l}\n"))),
                            _ => None,
                        },
                        _ => None,
                    }
                    .unwrap_or(Value::Nil);
                    self.set_target(&args[0], line.clone())?;
                    Ok(line)
                }
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
                            Node::Form(..) | Node::Call(..) => elems(head).iter().any(|k| equal(&case_key(k), &v)),
                            k => equal(&case_key(k), &v),
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
                        self.tick()?;
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
                            s.tick()?;
                            s.set_local(var.clone(), Value::Int(i));
                            s.body(&args[3..])?;
                            // Stop at i64::MAX instead of overflowing.
                            let Some(next) = i.checked_add(1) else { break };
                            i = next;
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
                            s.tick()?;
                            for (v, l) in vars.iter().zip(&lists) {
                                s.set_local(v.clone(), l[i].clone());
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
                            s.tick()?;
                            s.set_local(var.clone(), x.clone());
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
                        self.frames.push(frame_new());
                        let r = (|| {
                            for b in elems(&args[0]) {
                                let one = self.bind_list(&Node::Form(vec![b], Pos { file: 0, line: 0 }))?;
                                if let Some(f) = self.frames.last() {
                                    f.borrow_mut().extend(one.clone());
                                }
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
                    let env = self.closure_env();
                    Ok(Value::Func(Rc::new(Func { name: "lambda".into(), params, body: args[1..].to_vec(), env })))
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
                _ => Err(Flow::NotSpecial),
            }
        })())
        .filter(|r| !matches!(r, Err(Flow::NotSpecial)))
    }

    // ----- printing -----

    fn repr(&self, v: &Value) -> String {
        let mut s = String::new();
        self.write_repr(v, &mut s);
        s
    }

    /// Print `v` SKILL style, with a loop instead of recursion (lists nest without limit); text
    /// beyond [`MAX_STR`] bytes is cut off with `...`.
    fn write_repr(&self, v: &Value, s: &mut String) {
        enum Item<'a> {
            V(&'a Value),
            Text(&'static str),
        }
        let mut todo = vec![Item::V(v)];
        while let Some(it) = todo.pop() {
            if s.len() > MAX_STR {
                s.push_str("...");
                return;
            }
            let v = match it {
                Item::Text(t) => {
                    s.push_str(t);
                    continue;
                }
                Item::V(v) => v,
            };
            match v {
                Value::Nil => s.push_str("nil"),
                Value::T => s.push('t'),
                Value::Int(i) => s.push_str(&i.to_string()),
                Value::Float(f) => s.push_str(&fmt_float(*f)),
                Value::Str(x) => write_str(x, s),
                Value::Sym(x) => s.push_str(x),
                Value::List(l) => {
                    s.push('(');
                    todo.push(Item::Text(")"));
                    for (i, x) in l.iter().enumerate().rev() {
                        todo.push(Item::V(x));
                        if i > 0 {
                            todo.push(Item::Text(" "));
                        }
                    }
                }
                Value::Func(f) => s.push_str(&format!("funobj:{}", f.name)), // i18n: ignore
                Value::Obj(o) => {
                    let (cv, id) = match o {
                        Obj::Cv(c) => (*c, 0),
                        Obj::Shape(c, id) | Obj::Inst(c, id) => (*c, *id),
                    };
                    s.push_str(&format!("db:0x{:08x}", ((cv as u64 + 1) << 24) | id));
                }
                Value::Table(t) => s.push_str(&format!("table:{}", t.borrow().name)), // i18n: ignore
                Value::Port(0) => s.push_str("port:\"*stdout*\""),
                Value::Port(k) => s.push_str(&format!("port:\"file{k}\"")), // i18n: ignore
            }
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
                prec = Some(p.parse().unwrap_or(usize::MAX));
            }
            let w: usize = if width.is_empty() { 0 } else { width.parse().unwrap_or(usize::MAX) };
            if w > MAX_FORMAT_WIDTH || prec.is_some_and(|p| p > MAX_FORMAT_WIDTH) {
                return fail(format!("format: field width or precision above {MAX_FORMAT_WIDTH}"));
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
            if out.len() > MAX_STR {
                return fail(format!("format: result longer than {} MB", MAX_STR >> 20));
            }
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
            Value::Obj(obj) => {
                let key = match obj {
                    Obj::Cv(c) => (*c, 0),
                    Obj::Shape(c, id) | Obj::Inst(c, id) => (*c, *id),
                };
                if let Some((_, v)) = self.props.get(&key).and_then(|p| p.iter().find(|(n, _)| **n == **prop)) {
                    return Ok(v.clone());
                }
                self.db_prop(*obj, prop)
            }
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
                if self.eval_depth >= MAX_EVAL_DEPTH || self.stack_low() {
                    return fail(format!("~>{prop}: list nested too deeply"));
                }
                self.eval_depth += 1;
                let out = l.iter().map(|x| self.get_prop(x, prop)).collect::<R<_>>();
                self.eval_depth -= 1;
                Ok(list(out?))
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
                    (Value::Int(b), Value::Int(e)) if *e >= 0 => u32::try_from(*e)
                        .ok()
                        .and_then(|e| b.checked_pow(e))
                        .map(Value::Int)
                        .map_or_else(|| fail("expt: integer overflow"), Ok),
                    _ => Ok(float(num(0)?.powf(num(1)?))),
                }
            }
            "abs" => match arg(0) {
                Value::Int(i) => i.checked_abs().map(Value::Int).map_or_else(|| fail("abs: integer overflow"), Ok),
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
                let r = if f == "modulo" { x.checked_rem_euclid(y) } else { x.checked_rem(y) };
                r.map(Value::Int).map_or_else(|| fail(format!("{f}: integer overflow")), Ok)
            }
            "random" => {
                let k = if n > 0 { int(0)?.max(1) } else { i64::MAX };
                self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                Ok(Value::Int(((self.rng >> 17) as i64).rem_euclid(k)))
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
            "list" => {
                // ?key value items stay in the list (ROD sub-path specs are written that way).
                let mut v = v;
                for (k, x) in a.keys {
                    v.push(sym(&format!("?{k}")));
                    v.push(x);
                }
                Ok(list(v))
            }
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
            "nthelem" => Ok(lst(1)?.get(int(0)?.saturating_sub(1).max(0) as usize).cloned().unwrap_or(Value::Nil)),
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
                for (k, x) in v.iter().enumerate() {
                    if out.len() + items(x).len() > MAX_LIST {
                        return fail(format!("{f}: list longer than {MAX_LIST} elements"));
                    }
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
                // Bounded: lists nest without limit and may share sublists (exponential copies).
                fn rec(new: &Value, old: &Value, x: &Value, depth: usize, budget: &mut usize) -> R<Value> {
                    if depth > 10 * MAX_NESTING || *budget == 0 {
                        return fail("subst: list too large or nested too deeply");
                    }
                    *budget -= 1;
                    if equal(x, old) {
                        return Ok(new.clone());
                    }
                    Ok(match x {
                        Value::List(l) => list(l.iter().map(|e| rec(new, old, e, depth + 1, budget)).collect::<R<_>>()?),
                        x => x.clone(),
                    })
                }
                let mut budget = MAX_LIST;
                rec(&v[0], &v[1], &v[2], 0, &mut budget)
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
                    self.tick()?;
                    let r = self.call1(&v[0], lists.iter().map(|l| l[i].clone()).collect())?;
                    out.push(r);
                }
                if f == "mapcan" && out.iter().map(|r| items(r).len()).sum::<usize>() > MAX_LIST {
                    return fail(format!("{f}: list longer than {MAX_LIST} elements"));
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
                let node = datum_to_node(&v[0])?;
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
                let path = self.file_path(f, &path)?;
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
                    if out.len() > MAX_STR {
                        return fail(format!("strcat: string longer than {} MB", MAX_STR >> 20));
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
                let mut out = String::new();
                for (i, x) in lst(0)?.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&sep);
                    }
                    out.push_str(&self.display(x));
                    if out.len() > MAX_STR {
                        return fail(format!("buildString: string longer than {} MB", MAX_STR >> 20));
                    }
                }
                Ok(string(out))
            }
            "concat" => {
                let mut out = String::new();
                for x in &v {
                    out.push_str(&self.display(x));
                    if out.len() > MAX_STR {
                        return fail(format!("concat: string longer than {} MB", MAX_STR >> 20));
                    }
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
                self.emit(&text)?;
                Ok(Value::T)
            }
            "warn" => {
                let fmt = s(0)?;
                let text = self.format(&fmt, &v[1..])?;
                let nl = if text.ends_with('\n') { "" } else { "\n" };
                self.emit(&format!("*WARNING* {text}{nl}"))?;
                Ok(Value::Nil)
            }
            "fprintf" => {
                let fmt = s(1)?;
                let text = self.format(&fmt, &v[2..])?;
                match v.first() {
                    Some(Value::Port(k)) if *k > 0 => self.write_port(*k, &text)?,
                    _ => self.emit(&text)?,
                }
                Ok(Value::T)
            }
            "outfile" | "infile" => {
                need(1)?;
                let path = PathBuf::from(&*s(0)?);
                // Checked before anything is opened: outfile truncates.
                let path = self.file_path(f, &path)?;
                let port = if f == "outfile" {
                    let append = matches!(v.get(1), Some(Value::Str(m)) if m.starts_with('a'));
                    if !append {
                        std::fs::write(&path, "").or_else(|e| fail(format!("{f}: {}: {e}", path.display())))?;
                    }
                    PortFile::Out(path)
                } else {
                    match std::fs::read_to_string(&path) {
                        Ok(t) => PortFile::In(t.lines().map(str::to_string).collect()),
                        Err(_) => return Ok(Value::Nil),
                    }
                };
                self.ports.push(port);
                Ok(Value::Port(self.ports.len() as u32))
            }
            "close" => {
                if let Some(Value::Port(k)) = v.first()
                    && let Some(p) = (*k as usize).checked_sub(1).and_then(|i| self.ports.get_mut(i))
                {
                    *p = PortFile::Closed;
                }
                Ok(Value::T)
            }
            "portp" => Ok(boolv(matches!(arg(0), Value::Port(_)))),
            // Library objects are their names here: list(ddGetObj("lib") "cell" "layout").
            "ddGetObj" => Ok(match v.first() {
                Some(x @ (Value::Str(_) | Value::Sym(_))) => string(self.display(x)),
                _ => Value::Nil,
            }),
            "arrayp" => Ok(boolv(matches!(arg(0), Value::Table(ref t) if t.borrow().name == "array"))),
            "techGetTechFile" => Ok(dpl(&[("objType", string("techFile")), ("libName", string(self.p.tech.name.clone()))])),
            "techFindViaDefByName" => {
                need(2)?;
                let name = s(1)?;
                Ok(match self.p.tech.vias.iter().find(|d| *d.name == *name) {
                    Some(d) => dpl(&[("objType", string("viaDef")), ("name", string(d.name.clone()))]),
                    None => Value::Nil,
                })
            }
            "techGetLayerName" => {
                need(2)?;
                let k = int(1)?;
                Ok(self.p.tech.layers.iter().find(|l| l.gds_layer as i64 == k).map_or(Value::Nil, |l| string(l.name.clone())))
            }
            f if f.starts_with("rod") => self.rod_call(f, &v, &a.keys),
            f if f.starts_with("hi") => self.hi_call(f, &v, &a.keys),
            "print" | "println" | "pprint" => {
                need(1)?;
                let mut text = self.repr(&v[0]);
                if f != "print" {
                    text.push('\n');
                }
                self.emit(&text)?;
                Ok(Value::Nil)
            }
            "printlev" => {
                let text = self.repr(&arg(0));
                self.emit(&format!("{text}\n"))?;
                Ok(Value::Nil)
            }
            "newline" => {
                self.emit("\n")?;
                Ok(Value::Nil)
            }
            "drain" | "gc" | "sstatus" | "status" | "hiRegTimer" => Ok(Value::T),
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
            "getShellEnvVar" => {
                let name = s(0)?;
                if self.sandbox.is_some() {
                    return fail("getShellEnvVar: environment variables are not available here");
                }
                Ok(std::env::var(&*name).map_or(Value::Nil, string))
            }

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
            "dbCreateParamInst" | "dbCreateParamInstByMasterName" => {
                let by_name = f.ends_with("MasterName");
                need(if by_name { 7 } else { 5 })?;
                let cv = self.cv(&v[0])?;
                let (lib, cell, k) = if by_name {
                    (text(1)?, text(2)?, 4)
                } else {
                    let m = self.cv(&v[1])?;
                    (self.cvs[m].cell.lib.clone(), self.cvs[m].cell.cell.clone(), 2)
                };
                let params = items(&arg(k + 4)).to_vec();
                let master = CellRef::new(&lib, &cell);
                if let Some(crate::pcell::PCell::SkillDef { params: defaults, .. }) =
                    self.p.cell(&master).and_then(|c| c.pcell.clone())
                {
                    let r = self.skill_variant(&master, &defaults, &params)?;
                    return self.place(cv, r, &v[k], &v[k + 1], &v[k + 2], None);
                }
                match self.param_pcell(&cell, &params)? {
                    Some(pc) => {
                        let r = self.p.ensure_pcell(&pc).or_else(fail)?;
                        self.place(cv, r, &v[k], &v[k + 1], &v[k + 2], None)
                    }
                    None => match self.master(cv, &lib, &cell)? {
                        Some(r) => {
                            if !params.is_empty() {
                                self.out
                                    .push_str(&format!("*WARNING* {f}: {lib}/{cell} is not a PCell here; parameters ignored\n"));
                            }
                            self.place(cv, r, &v[k], &v[k + 1], &v[k + 2], None)
                        }
                        None => {
                            self.out.push_str(&format!("*WARNING* {f}: cannot open master {lib}/{cell} layout\n"));
                            Ok(Value::Nil)
                        }
                    },
                }
            }
            "dbCreateVia" => {
                need(3)?;
                let cv = self.cv(&v[0])?;
                let name = match &v[1] {
                    Value::Str(s) | Value::Sym(s) => s.to_string(),
                    d => match self.get_prop(d, &Name::from("name"))? {
                        Value::Str(s) => s.to_string(),
                        _ => return fail(format!("{f}: via definition expected, got {}", self.repr(d))),
                    },
                };
                let at = self.point(&v[2])?;
                let def = self.p.tech.vias.iter().find(|d| d.name == name).cloned();
                let Some(def) = def else { return fail(format!("{f}: no via '{name}' in the technology")) };
                let items = crate::pcell::via_at(&self.p.tech, &def, at).or_else(fail)?;
                let l = self.layout_mut(cv)?;
                let ids: Vec<u64> = items.into_iter().map(|(layer, g)| l.add(layer, g)).collect();
                Ok(ids.first().map_or(Value::Nil, |id| Value::Obj(Obj::Shape(cv, *id))))
            }
            "dbCreateNet" | "dbMakeNet" | "dbFindNetByName" => {
                need(2)?;
                let cv = self.cv(&v[0])?;
                let name = text(1)?;
                if f == "dbFindNetByName" {
                    let has = self.layout(cv).is_some_and(|l| {
                        l.shapes.iter().any(|s| matches!(&s.geom, Geometry::Label { text, .. } if *text == name))
                    });
                    if !has {
                        return Ok(Value::Nil);
                    }
                }
                Ok(dpl(&[("objType", string("net")), ("name", string(name)), ("cellView", Value::Obj(Obj::Cv(cv)))]))
            }
            "dbCreateTerm" => {
                need(2)?;
                let net = self.get_prop(&v[0], &Name::from("name"))?;
                let dir = v.get(2).map(|d| self.display(d)).unwrap_or_else(|| "inputOutput".into());
                Ok(dpl(&[("objType", string("term")), ("name", net.clone()), ("net", v[0].clone()), ("direction", string(dir))]))
            }
            "dbCreatePin" | "dbAddFigToNet" => {
                // A pin is the figure plus a label with the net name at its centre, on the
                // layer's net label layer, which extraction and LVS read as a port.
                need(2)?;
                let (net_v, fig_v) = if f == "dbCreatePin" { (&v[0], &v[1]) } else { (&v[1], &v[0]) };
                let name = match self.get_prop(net_v, &Name::from("name"))? {
                    Value::Str(s) => s.to_string(),
                    x => return fail(format!("{f}: net expected, got {}", self.repr(&x))),
                };
                let (cv, id) = self.fig(fig_v)?;
                let shape = self.layout(cv).and_then(|l| l.shape(id)).cloned();
                let Some(shape) = shape else { return fail(format!("{f}: the figure is not a shape")) };
                let b = shape.bbox();
                let layer = self.p.tech.net_label_layer(shape.layer).unwrap_or(shape.layer);
                let size = self.to_dbu(0.1).max(1);
                let at = ((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2);
                self.layout_mut(cv)?.add(layer, Geometry::Label { text: name.clone(), at, size });
                Ok(dpl(&[("objType", string("pin")), ("name", string(name)), ("fig", fig_v.clone())]))
            }
            "dbReplaceProp" | "dbCreateProp" => {
                need(4)?;
                let (cv, id) = self.fig(&v[0]).or_else(|_| match &v[0] {
                    Value::Obj(Obj::Cv(c)) => Ok((*c, 0)),
                    x => fail(format!("{f}: object expected, got {}", self.repr(x))),
                })?;
                let name = text(1)?;
                let props = self.props.entry((cv, id)).or_default();
                props.retain(|(n, _)| *n != name);
                props.push((name.clone(), v[3].clone()));
                Ok(dpl(&[("objType", string("prop")), ("name", string(name)), ("value", v[3].clone())]))
            }
            "dbGetPropByName" => {
                need(2)?;
                let key = match &v[0] {
                    Value::Obj(Obj::Cv(c)) => (*c, 0),
                    o => self.fig(o)?,
                };
                let name = text(1)?;
                Ok(match self.props.get(&key).and_then(|p| p.iter().find(|(n, _)| *n == name)) {
                    Some((_, x)) => dpl(&[("objType", string("prop")), ("name", string(name)), ("value", x.clone())]),
                    None => Value::Nil,
                })
            }
            "geGetSelSet" | "geGetSelectedSet" => Ok(Value::Nil),
            "geSelectFig" | "geDeselectAll" | "geDeselectFig" => Ok(Value::T),
            "leIsPointInsideFig" => {
                need(2)?;
                let (cv, id) = self.fig(&v[0])?;
                let p = self.point(&v[1])?;
                Ok(boolv(self.layout(cv).and_then(|l| l.shape(id)).is_some_and(|s| s.hit(p.0, p.1, 0))))
            }
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
            "dbLayerAnd" | "dbLayerOr" | "dbLayerXor" | "dbLayerAndNot" | "dbLayerSize" => {
                need(if f == "dbLayerOr" { 3 } else { 4 })?;
                let cv = self.cv(&v[0])?;
                let layer = self.layer(&v[1])?;
                let a = self.shape_geoms(f, &v[2])?;
                let res = if f == "dbLayerSize" {
                    let d = match &v[3] {
                        x @ (Value::Int(_) | Value::Float(_)) => self.to_dbu(as_f64(x)),
                        x => return fail(format!("{f}: size must be a number, got {}", self.repr(x))),
                    };
                    polybool::sized_geometry(&a, d)
                } else {
                    let b = self.shape_geoms(f, &arg(3))?;
                    let op = match f {
                        "dbLayerAnd" => BoolOp::And,
                        "dbLayerOr" => BoolOp::Or,
                        "dbLayerXor" => BoolOp::Xor,
                        _ => BoolOp::Not,
                    };
                    polybool::boolean_geometry(&a, &b, op)
                };
                let l = self.layout_mut(cv)?;
                Ok(list(res.into_iter().map(|g| Value::Obj(Obj::Shape(cv, l.add(layer, g)))).collect()))
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

    /// Geometry of a shape or a list of shapes (`nil`: none).
    fn shape_geoms(&self, f: &str, v: &Value) -> R<Vec<Geometry>> {
        let one = std::slice::from_ref(v);
        let figs = match v {
            Value::Nil => &[][..],
            Value::List(l) => l.as_slice(),
            _ => one,
        };
        figs.iter()
            .map(|x| match x {
                Value::Obj(Obj::Shape(cv, id)) => match self.layout(*cv).and_then(|l| l.shape(*id)) {
                    Some(s) => Ok(s.geom.clone()),
                    None => fail(format!("{f}: shape no longer exists")),
                },
                x => fail(format!("{f}: shape expected, got {}", self.repr(x))),
            })
            .collect()
    }

    /// User interface forms (`hi*`) without a window: forms and fields are tables whose values
    /// scripts read and set (`form->field->value`), and `hiDisplayForm` runs the form's OK
    /// callback at once with the current values, as when OK is pressed on the defaults.
    fn hi_call(&mut self, f: &str, v: &[Value], keys: &[(Name, Value)]) -> R<Value> {
        let key = |k: &str| keys.iter().find(|(n, _)| &**n == k).map(|(_, v)| v.clone());
        let table = |name: &str, entries: Vec<(Value, Value)>| {
            Value::Table(Rc::new(RefCell::new(Table { name: name.into(), default: None, entries })))
        };
        match f {
            "hiCreateStringField"
            | "hiCreateFloatField"
            | "hiCreateIntField"
            | "hiCreateBooleanButton"
            | "hiCreateCyclicField"
            | "hiCreateRadioField"
            | "hiCreateListBoxField"
            | "hiCreateLabel"
            | "hiCreateButton"
            | "hiCreateComboField"
            | "hiCreateToggleField"
            | "hiCreatePointField"
            | "hiCreateFileSelectorField" => {
                let name = key("name").unwrap_or(Value::Nil);
                let value = key("value").or_else(|| key("defValue")).unwrap_or(Value::Nil);
                let mut e = vec![
                    (sym("name"), name),
                    (sym("value"), value.clone()),
                    (sym("defValue"), value),
                    (sym("hiFieldType"), string(f)),
                ];
                for k in ["prompt", "choices", "items", "callback", "range"] {
                    if let Some(x) = key(k) {
                        e.push((sym(k), x));
                    }
                }
                Ok(table("field", e))
            }
            "hiCreateAppForm" | "hiCreateForm" | "hiCreateLayoutForm" => {
                let name = key("name").or_else(|| v.first().cloned()).unwrap_or(Value::Nil);
                let fields = key("fields").or_else(|| v.get(2).cloned()).unwrap_or(Value::Nil);
                let mut e = vec![(sym("hiFormSym"), name.clone()), (sym("fieldList"), fields.clone())];
                if let Some(t) = key("formTitle").or_else(|| v.get(1).cloned()) {
                    e.push((sym("formTitle"), t));
                }
                for fld in items(&fields).iter() {
                    // A field, or list(field x y ...) in a layout form.
                    let fld = match fld {
                        Value::List(l) => l[0].clone(),
                        x => x.clone(),
                    };
                    let fname = self.get_prop(&fld, &Name::from("name"))?;
                    if let Value::Sym(n) = fname {
                        e.push((Value::Sym(n), fld));
                    }
                }
                if let Some(cb) = key("callback").or_else(|| v.get(3).cloned()) {
                    e.push((sym("callback"), cb));
                }
                let form = table("form", e);
                if let Value::Sym(n) = &name {
                    self.globals.insert(n.clone(), form.clone());
                }
                Ok(form)
            }
            "hiDisplayForm" | "hiDisplayModalDBox" | "hiiFormOk" => {
                let form = v.first().cloned().unwrap_or(Value::Nil);
                let form = match &form {
                    Value::Sym(n) => self.lookup(n)?,
                    x => x.clone(),
                };
                if self.interactive && f != "hiiFormOk" {
                    let req = self.form_request(&form)?;
                    self.forms.push(req);
                    return Ok(Value::T);
                }
                let cb = self.get_prop(&form, &Name::from("callback"))?;
                // list(okCallback cancelCallback): OK.
                let cb = match &cb {
                    Value::List(l) if !matches!(l[0], Value::Sym(ref s) if &**s == "lambda") => l[0].clone(),
                    x => x.clone(),
                };
                let prev = self.current_form.replace(form.clone());
                let r = match cb {
                    Value::Nil => Ok(Value::Nil),
                    Value::Str(src) => self.run_source(&src, None),
                    Value::Sym(n) => match self.funcs.get(&n).cloned() {
                        Some(func) => self.apply(&func, Args { pos: vec![form.clone()], keys: vec![] }),
                        None => self.run_source(&n, None),
                    },
                    Value::Func(func) => self.apply(&func, Args { pos: vec![form.clone()], keys: vec![] }),
                    x => fail(format!("{f}: cannot call the callback {}", self.repr(&x))),
                };
                self.current_form = prev;
                r.map(|_| Value::T)
            }
            "hiGetCurrentForm" => Ok(self.current_form.clone().unwrap_or(Value::Nil)),
            "hiFormDone" | "hiFormCancel" | "hiFormClose" | "hiFormApply" | "hiRedraw" | "hiFlush" | "hiDeleteForm"
            | "hiRegTimer" | "hiSetBindKey" | "hiSetBindKeys" | "hiInsertBannerMenu" | "hiAddMenuItem" => Ok(Value::T),
            "hiGetCurrentWindow" | "hiCreateMenu" | "hiCreateMenuItem" | "hiCreatePulldownMenu" | "hiGetAppType" => {
                Ok(Value::Nil)
            }
            "hiQuestionBox" | "hiDisplayAppDBox" => Ok(Value::T),
            _ => fail(format!("undefined function {f} (interactive forms are not shown; see hiDisplayForm)")),
        }
    }

    /// The [`FormRequest`] of a form table, registering it under a global name if it has none.
    fn form_request(&mut self, form: &Value) -> R<FormRequest> {
        let symbol = match self.get_prop(form, &Name::from("hiFormSym"))? {
            Value::Sym(n) => n.to_string(),
            _ => {
                let n = format!("icsForm{}", self.globals.len()); // i18n: ignore
                self.globals.insert(n.as_str().into(), form.clone());
                n
            }
        };
        let title = match self.get_prop(form, &Name::from("formTitle"))? {
            Value::Nil => symbol.clone(),
            t => self.display(&t),
        };
        let mut fields = vec![];
        let list_v = self.get_prop(form, &Name::from("fieldList"))?;
        for fld in items(&list_v).to_vec() {
            let fld = match &fld {
                Value::List(l) => l[0].clone(),
                x => x.clone(),
            };
            let Value::Sym(name) = self.get_prop(&fld, &Name::from("name"))? else { continue };
            let kind_v = self.get_prop(&fld, &Name::from("hiFieldType"))?;
            let kind_name = self.display(&kind_v);
            let value = self.get_prop(&fld, &Name::from("value"))?;
            let prompt = match self.get_prop(&fld, &Name::from("prompt"))? {
                Value::Nil => name.to_string(),
                p => self.display(&p),
            };
            let choices_v = self.get_prop(&fld, &Name::from("choices"))?;
            let choices: Vec<String> = items(&choices_v).iter().map(|c| self.display(c)).collect();
            let kind = match kind_name.as_str() {
                "hiCreateFloatField" => FieldKind::Float,
                "hiCreateIntField" => FieldKind::Int,
                "hiCreateBooleanButton" | "hiCreateToggleField" => FieldKind::Bool,
                "hiCreateCyclicField" | "hiCreateRadioField" | "hiCreateComboField" | "hiCreateListBoxField" => {
                    FieldKind::Choice(choices)
                }
                "hiCreateLabel" | "hiCreateButton" => FieldKind::Label,
                _ => FieldKind::String,
            };
            let value = match (&kind, &value) {
                (FieldKind::Bool, v) => {
                    if truthy(v) {
                        "t".into()
                    } else {
                        "nil".into()
                    }
                }
                (_, Value::Nil) => String::new(),
                (_, v) => self.display(v),
            };
            fields.push(FormField { name: name.to_string(), prompt, kind, value });
        }
        Ok(FormRequest { symbol, title, fields })
    }

    /// Relative Object Design shapes: `rodCreateRect`, `rodCreatePath`, `rodCreatePolygon` with
    /// keyword arguments (`?cvId ?layer ?bBox | ?width ?length ?origin`, `?pts ?width`).
    fn rod_call(&mut self, f: &str, pos: &[Value], keys: &[(Name, Value)]) -> R<Value> {
        let key = |k: &str| keys.iter().find(|(n, _)| &**n == k).map(|(_, v)| v.clone());
        let cv = match key("cvId").or_else(|| pos.first().cloned()) {
            Some(c) => self.cv(&c)?,
            None => match self.current.clone() {
                Some(c) => {
                    let o = self.open_cv(c, true);
                    self.cv(&o)?
                }
                None => return fail(format!("{f}: ?cvId missing")),
            },
        };
        let layer = match key("layer") {
            Some(l) => self.layer(&l)?,
            None => return fail(format!("{f}: ?layer missing")),
        };
        let geom = match f {
            "rodCreateRect" => {
                let r = match key("bBox") {
                    Some(b) => self.bbox(&b)?,
                    None => {
                        let (Some(w), Some(h)) = (key("width"), key("length")) else {
                            return fail(format!("{f}: give ?bBox, or ?width and ?length"));
                        };
                        let o = match key("origin") {
                            Some(o) => self.point(&o)?,
                            None => (0, 0),
                        };
                        IRect::new(o.0, o.1, o.0 + self.to_dbu(as_f64(&w)), o.1 + self.to_dbu(as_f64(&h)))
                    }
                };
                Geometry::Rect(r)
            }
            "rodCreatePath" => {
                let pts = match key("pts") {
                    Some(p) => self.points(&p)?,
                    None => return fail(format!("{f}: ?pts missing")),
                };
                let w = key("width").map_or(0, |w| self.to_dbu(as_f64(&w)));
                let w = if w > 0 { w } else { self.p.tech.layer(layer).map_or(0, |d| d.min_width) };
                if pts.len() < 2 {
                    return fail(format!("{f}: a path needs at least 2 points"));
                }
                let subs = self.rod_subpaths(f, &pts, w, key("encSubPath"), key("offsetSubPath"), key("subRect"))?;
                let l = self.layout_mut(cv)?;
                for (layer, g) in subs {
                    l.add(layer, g);
                }
                Geometry::Path { points: pts, width: w }
            }
            "rodCreatePolygon" => {
                let pts = match key("pts") {
                    Some(p) => self.points(&p)?,
                    None => return fail(format!("{f}: ?pts missing")),
                };
                Geometry::Polygon { points: pts }
            }
            _ => return fail(format!("undefined function {f}")),
        };
        let id = self.layout_mut(cv)?.add(layer, geom);
        Ok(Value::Obj(Obj::Shape(cv, id)))
    }

    /// Shapes of the sub-paths of a ROD multipart path along `pts` (master width `w`):
    /// enclosing paths (`?layer ?enclosure`), offset paths (`?layer ?width ?sep
    /// ?justification`) and rows of rectangles (`?layer ?width ?length ?space`).
    fn rod_subpaths(
        &self,
        f: &str,
        pts: &[(i64, i64)],
        w: i64,
        enc: Option<Value>,
        offset: Option<Value>,
        rects: Option<Value>,
    ) -> R<Vec<(usize, Geometry)>> {
        let specs = |v: Option<Value>| -> Vec<Vec<Value>> {
            let Some(v) = v else { return vec![] };
            let l = items(&v).to_vec();
            // One spec, or a list of specs.
            if l.first().is_some_and(|x| matches!(x, Value::Sym(s) if s.starts_with('?'))) {
                vec![l]
            } else {
                l.iter().map(|x| items(x).to_vec()).collect()
            }
        };
        let get = |spec: &[Value], k: &str| -> Option<Value> {
            spec.iter()
                .position(|x| matches!(x, Value::Sym(s) if s.strip_prefix('?') == Some(k)))
                .and_then(|i| spec.get(i + 1).cloned())
        };
        if w.abs() > COORD_LIMIT || pts.iter().any(|p| p.0.abs() > COORD_LIMIT || p.1.abs() > COORD_LIMIT) {
            return fail(format!("{f}: coordinates or width out of range"));
        }
        let mut out = vec![];
        for spec in specs(enc) {
            let Some(lv) = get(&spec, "layer") else { return fail(format!("{f}: ?encSubPath needs ?layer")) };
            let layer = self.layer(&lv)?;
            let e = get(&spec, "enclosure").map_or(0, |v| self.to_dbu(as_f64(&v)));
            let Some(width) = e.checked_mul(2).and_then(|e2| w.checked_add(e2)) else {
                return fail(format!("{f}: ?encSubPath enclosure out of range"));
            };
            if width > 0 {
                out.push((layer, Geometry::Path { points: pts.to_vec(), width }));
            }
        }
        for spec in specs(offset) {
            let Some(lv) = get(&spec, "layer") else { return fail(format!("{f}: ?offsetSubPath needs ?layer")) };
            let layer = self.layer(&lv)?;
            let sw = get(&spec, "width").map_or(w, |v| self.to_dbu(as_f64(&v)));
            let sep = get(&spec, "sep").map_or(0, |v| self.to_dbu(as_f64(&v)));
            if sw.abs() > COORD_LIMIT || sep.abs() > COORD_LIMIT {
                return fail(format!("{f}: ?offsetSubPath size out of range"));
            }
            let right = get(&spec, "justification").is_some_and(|j| self.display(&j) == "right");
            let d = (w / 2 + sep + sw / 2) * if right { -1 } else { 1 };
            let Some(line) = crate::reshape::offset_polyline(pts, d, None) else {
                return fail(format!("{f}: ?offsetSubPath needs a Manhattan path without U-turns"));
            };
            out.push((layer, Geometry::Path { points: line, width: sw }));
        }
        for spec in specs(rects) {
            let Some(lv) = get(&spec, "layer") else { return fail(format!("{f}: ?subRect needs ?layer")) };
            let layer = self.layer(&lv)?;
            let rw = get(&spec, "width").map_or(0, |v| self.to_dbu(as_f64(&v)));
            if rw <= 0 {
                return fail(format!("{f}: ?subRect needs a ?width"));
            }
            let rl = get(&spec, "length").map_or(rw, |v| self.to_dbu(as_f64(&v)));
            let space = get(&spec, "space").map_or(rw, |v| self.to_dbu(as_f64(&v)));
            // Bounded values keep the arithmetic below far from overflow.
            if [rw, rl, space].iter().any(|x| x.abs() > COORD_LIMIT) {
                return fail(format!("{f}: ?subRect size out of range"));
            }
            let pitch = (rl + space).max(1);
            for seg in pts.windows(2) {
                let (a, b) = (seg[0], seg[1]);
                let len = (b.0 - a.0).abs() + (b.1 - a.1).abs();
                // Keep clear of the segment ends (corners) by half the master width plus a space.
                let usable = len - 2 * (w / 2 + space) - rl;
                if usable < 0 {
                    continue;
                }
                let n = usable / pitch + 1;
                if out.len() as i64 + n > MAX_SUBRECTS as i64 {
                    return fail(format!("{f}: ?subRect would create more than {MAX_SUBRECTS} rectangles"));
                }
                let start = w / 2 + space + (usable - (n - 1) * pitch) / 2;
                let dir = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
                for k in 0..n {
                    let t = start + k * pitch;
                    let (cx, cy) = (a.0 + dir.0 * t, a.1 + dir.1 * t);
                    let r = if dir.1 == 0 {
                        IRect::new(cx.min(cx + dir.0 * rl), cy - rw / 2, cx.max(cx + dir.0 * rl), cy + rw / 2)
                    } else {
                        IRect::new(cx - rw / 2, cy.min(cy + dir.1 * rl), cx + rw / 2, cy.max(cy + dir.1 * rl))
                    };
                    out.push((layer, Geometry::Rect(r)));
                }
            }
        }
        Ok(out)
    }

    /// The IC Studio PCell for a parametric instance of master `cell` (`nmos`, `pmos`,
    /// `poly_res`, `mom_cap`, `ntap`, `ptap`, `nring`, `pring`, or a `pcells` variant of one)
    /// with CDF-style parameters `list(name type value)`; `None` for other masters.
    fn param_pcell(&self, cell: &str, params: &[Value]) -> R<Option<crate::pcell::PCell>> {
        use crate::pcell::PCell;
        let kind = ["nmos", "pmos", "poly_res", "mom_cap", "ntap", "ptap", "nring", "pring"]
            .into_iter()
            .find(|k| cell == *k || cell.strip_prefix(k).is_some_and(|r| r.starts_with('_')));
        let Some(kind) = kind else { return Ok(None) };
        let get = |names: &[&str]| -> Option<Value> {
            params.iter().find_map(|p| {
                let it = items(p);
                let n = match it.first() {
                    Some(Value::Str(s) | Value::Sym(s)) => s.to_string(),
                    _ => return None,
                };
                names.iter().any(|x| x.eq_ignore_ascii_case(&n)).then(|| it.last().cloned()).flatten()
            })
        };
        let len =
            |names: &[&str], default_um: f64| -> i64 { self.to_dbu(get(names).and_then(|v| param_um(&v)).unwrap_or(default_um)) };
        let count = |names: &[&str]| -> u32 {
            get(names)
                .and_then(|v| match v {
                    Value::Str(s) => s.parse::<f64>().ok(),
                    x @ (Value::Int(_) | Value::Float(_)) => Some(as_f64(&x)),
                    _ => None,
                })
                .map_or(1, |x| x.max(1.0) as u32)
        };
        Ok(Some(match kind {
            "nmos" | "pmos" => {
                let pmos = kind == "pmos";
                let fingers = count(&["fingers", "nf", "nfing"]);
                // w is per device in most CDFs; IC Studio's w is per finger.
                let total = len(&["w", "width"], if pmos { 2.0 } else { 1.0 });
                let per =
                    if get(&["fw", "fingerWidth"]).is_some() { len(&["fw", "fingerWidth"], 1.0) } else { total / fingers as i64 };
                PCell::mos(pmos, per.max(1), len(&["l", "length"], 0.18), fingers)
            }
            "poly_res" => PCell::PolyRes { w: len(&["w"], 0.4), l: len(&["l"], 5.0) },
            "mom_cap" => PCell::MomCap { w: len(&["w"], 5.0), h: len(&["h", "l"], 5.0) },
            "ntap" | "ptap" => PCell::Tap { well: kind == "ntap", w: len(&["w", "h"], 1.0) },
            _ => PCell::GuardRing { well: kind == "nring", w: len(&["w"], 5.0), h: len(&["h", "l"], 5.0), width: 0, gap: 0 },
        }))
    }

    /// The variant cell of SKILL PCell `master` for the instance parameters `given`
    /// (`list(name type value)`, the others take their defaults), evaluated on first use.
    fn skill_variant(&mut self, master: &CellRef, defaults: &[(String, String)], given: &[Value]) -> R<CellRef> {
        let mut values: Vec<(String, String)> = defaults.to_vec();
        for p in given {
            let it = items(p);
            let (Some(n), Some(val)) = (it.first(), it.last()) else { continue };
            let n = self.display(n);
            let text = self.repr(val);
            match values.iter_mut().find(|(k, _)| *k == n) {
                Some(slot) => slot.1 = text,
                None => self.out.push_str(&format!("*WARNING* {}: no parameter {n}\n", master.cell)),
            }
        }
        let variant =
            crate::pcell::PCell::SkillVariant { master: format!("{}/{}", master.lib, master.cell), params: values.clone() };
        let r = CellRef::new(&master.lib, variant.variant_name());
        if self.p.cell(&r).is_none() {
            let mut c = Cell::new(&r.cell);
            c.description = variant.summary(1000);
            c.pcell = Some(variant);
            c.layout = Some(Layout::default());
            self.p.add_cell(&master.lib, c).or_else(fail)?;
            self.eval_skill_pcell(master, &r, &values)?;
        }
        Ok(r)
    }

    /// Empty the layout of `cell`, as a tracked edit (see [`Interp::layout_mut`]).
    fn clear_layout(&mut self, cell: &CellRef) -> R<()> {
        let cv = self.open_cv(cell.clone(), true);
        let cv = self.cv(&cv)?;
        *self.layout_mut(cv)? = Layout::default();
        Ok(())
    }

    /// Re-run the (redefined) SKILL PCell `master` into each of its existing variants, with the
    /// variant's parameter values over the new `defaults`.
    fn rebuild_variants(&mut self, master: &CellRef, defaults: &[(String, String)]) -> R<()> {
        let key = format!("{}/{}", master.lib, master.cell);
        let variants: Vec<(CellRef, Vec<(String, String)>)> = self
            .p
            .all_cells()
            .filter_map(|(r, c)| match &c.pcell {
                Some(crate::pcell::PCell::SkillVariant { master: m, params }) if *m == key => Some((r, params.clone())),
                _ => None,
            })
            .collect();
        for (r, given) in variants {
            let mut values = defaults.to_vec();
            for (n, v) in &given {
                if let Some(slot) = values.iter_mut().find(|(k, _)| k == n) {
                    slot.1 = v.clone();
                }
            }
            self.clear_layout(&r)?;
            if let Some(c) = self.p.cell_mut(&r) {
                let variant = crate::pcell::PCell::SkillVariant { master: key.clone(), params: values.clone() };
                c.description = variant.summary(1000);
                c.pcell = Some(variant);
            }
            self.eval_skill_pcell(master, &r, &values)?;
        }
        Ok(())
    }

    /// Run the body of SKILL PCell `master` into cell `into`, with `pcCellView` and the
    /// parameters (SKILL text) bound.
    fn eval_skill_pcell(&mut self, master: &CellRef, into: &CellRef, values: &[(String, String)]) -> R<()> {
        let Some(crate::pcell::PCell::SkillDef { body, .. }) = self.p.cell(master).and_then(|c| c.pcell.clone()) else {
            return fail(format!("{}/{} is not a SKILL PCell", master.lib, master.cell));
        };
        let cv = self.open_cv(into.clone(), true);
        let mut frame: HashMap<Name, Value> = HashMap::new();
        frame.insert("pcCellView".into(), cv);
        for (n, text) in values {
            let v = self.run_source(&format!("'{text}"), None)?;
            frame.insert(n.as_str().into(), v);
        }
        self.with_frame(frame, |s| s.run_source(&body, None)).map(|_| ())
    }

    fn write_port(&mut self, k: u32, text: &str) -> R<()> {
        use std::io::Write;
        match self.ports.get(k as usize - 1) {
            Some(PortFile::Out(path)) => {
                self.file_bytes += text.len() as u64;
                if self.file_bytes > MAX_FILE_BYTES {
                    return fail(format!("fprintf: more than {} MB written to files", MAX_FILE_BYTES >> 20));
                }
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
                    .or_else(|e| fail(format!("fprintf: {}: {e}", path.display())))?;
                f.write_all(text.as_bytes()).or_else(|e| fail(format!("fprintf: {e}")))
            }
            _ => fail("fprintf: the port is not open for output"),
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
                    "isParamCell" | "superMaster" | "mosaics" => Value::Nil,
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
///
/// Runs as a set of NFA states in time O(pattern × text), never backtracking.
fn simple_match(pat: &str, s: &str) -> bool {
    let mut p: Vec<char> = pat.chars().collect();
    let anchored = p.first() == Some(&'^');
    if anchored {
        p.remove(0);
    }
    // Items (character or '.', repeated with '*'); a final '$' anchors the end.
    let mut items: Vec<(char, bool)> = vec![];
    let mut at_end = false;
    let mut i = 0;
    while i < p.len() {
        if p[i] == '$' && i + 1 == p.len() {
            at_end = true;
        } else if p.get(i + 1) == Some(&'*') {
            items.push((p[i], true));
            i += 1;
        } else {
            items.push((p[i], false));
        }
        i += 1;
    }
    let n = items.len();
    // Add state k and the states after the starred items it can skip.
    let add = |set: &mut Vec<bool>, mut k: usize| {
        while k <= n && !set[k] {
            set[k] = true;
            if k < n && items[k].1 {
                k += 1;
            } else {
                break;
            }
        }
    };
    let mut cur = vec![false; n + 1];
    add(&mut cur, 0);
    let text: Vec<char> = s.chars().collect();
    for &c in &text {
        if cur[n] && !at_end {
            return true;
        }
        let mut next = vec![false; n + 1];
        for (k, &(pc, star)) in items.iter().enumerate() {
            if cur[k] && (pc == '.' || pc == c) {
                // A starred item may repeat (its closure includes what follows).
                add(&mut next, if star { k } else { k + 1 });
            }
        }
        if !anchored {
            add(&mut next, 0);
        }
        cur = next;
    }
    cur[n]
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

    /// The session is a plain handle: it can move between threads (no `Rc` escapes the
    /// evaluation thread).
    #[test]
    fn session_is_send() {
        fn send<T: Send>() {}
        send::<Session>();
    }

    /// A background evaluation keeps the session's state, can be polled and can be cancelled.
    #[test]
    fn background_evaluation_and_cancel() {
        let mut s = Session::new();
        s.start(Project::new("t"), Task::Eval { src: "procedure(f() 42)".into(), file: None }, None);
        let f = s.wait();
        assert!(f.result.is_ok());
        assert_eq!(s.procedures(), vec!["f".to_string()]);
        assert!(!s.is_running() && s.try_finish().is_none());

        s.start(f.project.unwrap(), Task::Eval { src: "while(t 1)".into(), file: None }, None);
        assert!(s.is_running());
        assert!(s.wait_timeout(std::time::Duration::from_millis(50)).is_none(), "still looping");
        s.cancel();
        let f = s.wait();
        assert!(f.result.unwrap_err().message.contains("cancelled"));
        // The state survives a cancelled run.
        let mut p = f.project.unwrap();
        assert_eq!(s.eval(&mut p, "f()", None, None).unwrap().value, "42");
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
    fn layer_booleans() {
        let mut p = Project::new("t");
        let src = r#"
            cv = dbOpenCellViewByType("lib" "b" "layout" "maskLayout" "w")
            tri = dbCreatePolygon(cv "metal1" list(0:0 2:0 0:2))
            box = dbCreateRect(cv "metal1" list(0:0 1:2))
            a = dbLayerAnd(cv "metal2" list(tri) list(box))
            o = dbLayerOr(cv "metal2" list(tri box))
            n = dbLayerAndNot(cv "metal3" tri box)
            x = dbLayerXor(cv "metal3" list(tri) list(box))
            s = dbLayerSize(cv "poly" list(box) 0.1)
            list(length(a) car(a)~>nPoints length(o) length(n) car(n)~>nPoints length(x) car(s)~>bBox dbLayerAnd(cv "metal2" tri nil))
        "#;
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "(1 4 1 1 3 2 ((-0.1 -0.1) (1.1 2.1)) nil)");
        let l = p.cell(&CellRef::new("lib", "b")).unwrap().layout.as_ref().unwrap();
        let m2 = p.tech.layer_index("metal2").unwrap();
        let and: Vec<&Geometry> = l.shapes.iter().filter(|s| s.layer == m2).map(|s| &s.geom).collect();
        assert_eq!(and[0], &Geometry::Polygon { points: vec![(0, 0), (1000, 0), (1000, 1000), (0, 2000)] });
        let e = run(&mut p, "cv = dbOpenCellViewByType(\"lib\" \"b\" \"layout\" \"\" \"a\") dbLayerAnd(cv \"metal2\" 1 2)", None);
        assert!(e.unwrap_err().message.contains("shape expected"));
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

    /// Functions common in Virtuoso layout scripts: parametric instances, vias, ROD shapes,
    /// nets and pins, properties, file ports and arrays.
    #[test]
    fn layout_script_extras() {
        let dir = std::env::temp_dir().join(format!("ics-skill-extras-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("report.txt");
        let src = format!(
            r#"
cv = dbOpenCellViewByType("work" "top" "layout" "maskLayout" "w")
m = dbCreateParamInstByMasterName(cv "pcells" "nmos" "layout" "M1" 0:0 "R0" 1
      list(list("w" "string" "2u") list("l" "float" 0.18e-6) list("nf" "int" 2)))
r = rodCreateRect(?cvId cv ?layer "metal1" ?bBox list(10:0 12:1))
w = rodCreatePath(?cvId cv ?layer "metal2" ?pts list(0:5 10:5) ?width 0.3)
tf = techGetTechFile(cv)
vd = techFindViaDefByName(tf "via1")
v = dbCreateVia(cv vd 11:0.5 "R0")
net = dbCreateNet(cv "out")
dbCreateTerm(net "out" "output")
dbCreatePin(net r)
dbReplaceProp(m "matchGroup" "string" "A")
declare(arr[3])
arr[0] = 7
p = outfile("{out}")
fprintf(p "%s %d\n" m~>name arr[0])
close(p)
q = infile("{out}")
gets(line q)
close(q)
list(m~>matchGroup dbFindNetByName(cv "out")~>name line arrayp(arr) w~>width)
"#,
            out = out.display().to_string().replace('\\', "/")
        );
        let mut p = Project::new("t");
        let r = run(&mut p, &src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "(\"A\" \"out\" \"M1 7\\n\" t 0.3)", "{}", r.output);
        let top = p.cell(&CellRef::new("work", "top")).and_then(|c| c.layout.as_ref()).unwrap();
        let inst = &top.instances[0];
        assert_eq!(inst.name.as_deref(), Some("M1"));
        assert_eq!(inst.cell.cell, "nmos_w1000_l180_nf2", "2 µm over 2 fingers");
        assert!(top.shapes.iter().any(|s| matches!(&s.geom, Geometry::Label { text, .. } if text == "out")), "pin label");
        let via1 = p.tech.layer_index("via1").unwrap();
        assert!(top.shapes.iter().any(|s| s.layer == via1), "via cut");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A PCell written in SKILL: defined once, evaluated per parameter set, reused for equal
    /// parameters, and kept with the project.
    #[test]
    fn skill_pcells() {
        let src = r#"
pcDefinePCell(list(ddGetObj("mylib") "plate" "layout")
  ((w 1.0) (l 2.0) (layer "metal1"))
  let((half)
    half = w / 2
    dbCreateRect(pcCellView layer list(-half:0 half:l))
    when(l > 3 dbCreateLabel(pcCellView layer 0:0 "long" "centerCenter" "R0" "roman" 0.1))))
cv = dbOpenCellViewByType("work" "top" "layout" "maskLayout" "w")
a = dbCreateParamInstByMasterName(cv "mylib" "plate" "layout" "P1" 0:0 "R0" 1 list(list("w" "float" 2.0)))
b = dbCreateParamInstByMasterName(cv "mylib" "plate" "layout" "P2" 10:0 "R0" 1 list(list("l" "float" 4.0) list("layer" "string" "metal2")))
c = dbCreateParamInstByMasterName(cv "mylib" "plate" "layout" "P3" 20:0 "R0" 1 list(list("w" "float" 2.0)))
list(a~>master~>cellName == c~>master~>cellName  a~>master~>cellName == b~>master~>cellName)
"#;
        let mut p = Project::new("t");
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "(t nil)", "equal parameters share a variant: {}", r.output);
        let k = p.tech.dbu_per_um;
        let shapes = |p: &Project, cell: &CellRef| p.cell(cell).and_then(|c| c.layout.clone()).unwrap().shapes;
        let master = CellRef::new("mylib", "plate");
        let m = shapes(&p, &master);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].bbox(), IRect::new(-k / 2, 0, k / 2, 2 * k), "the master is the PCell at its defaults");
        let top = p.cell(&CellRef::new("work", "top")).and_then(|c| c.layout.clone()).unwrap();
        let v1 = shapes(&p, &top.instances[0].cell);
        assert_eq!(v1[0].bbox(), IRect::new(-k, 0, k, 2 * k));
        let v2 = shapes(&p, &top.instances[1].cell);
        assert_eq!(v2.len(), 2, "label for l > 3");
        assert_eq!(v2[0].layer, p.tech.layer_index("metal2").unwrap());
        // Saved and reopened, the definition is still there and new variants evaluate.
        let dir = std::env::temp_dir().join(format!("ics-skill-pcell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        p.save_as(&dir).unwrap();
        let mut q = Project::open(&dir).unwrap();
        assert!(matches!(q.cell(&master).and_then(|c| c.pcell.clone()), Some(crate::pcell::PCell::SkillDef { .. })));
        let src2 = r#"cv = dbOpenCellViewByType("work" "top" "layout" "maskLayout" "a")
x = dbCreateParamInstByMasterName(cv "mylib" "plate" "layout" "P4" 30:0 "R0" 1 list(list("w" "float" 3.0)))
x~>master~>bBox"#;
        let r2 = run(&mut q, src2, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r2.value, "((-1.5 0.0) (1.5 2.0))");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A ROD multipart path as Virtuoso guard ring scripts write it: enclosing implant,
    /// contact rows and an offset metal path.
    #[test]
    fn rod_multipart_path() {
        let src = r#"
cv = dbOpenCellViewByType("work" "mpp" "layout" "maskLayout" "w")
rodCreatePath(?cvId cv ?layer "active" ?width 0.5 ?pts list(0:0 10:0 10:6)
  ?encSubPath list(list(?layer "pplus" ?enclosure 0.12))
  ?subRect list(list(?layer "contact" ?width 0.22 ?space 0.25))
  ?offsetSubPath list(list(?layer "metal1" ?width 0.3 ?sep 0.2 ?justification "right")))
length(cv~>shapes)
"#;
        let mut p = Project::new("t");
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        let l = p.cell(&CellRef::new("work", "mpp")).and_then(|c| c.layout.clone()).unwrap();
        let on = |name: &str| {
            let k = p.tech.layer_index(name).unwrap();
            l.shapes.iter().filter(|s| s.layer == k).cloned().collect::<Vec<_>>()
        };
        assert_eq!(on("active").len(), 1);
        assert!(matches!(&on("pplus")[0].geom, Geometry::Path { width, .. } if *width == 740), "{:?}", on("pplus"));
        let cuts = on("contact");
        assert!(cuts.len() > 20, "{} cuts", cuts.len());
        assert!(cuts.iter().all(|c| c.bbox().width() == 220 && c.bbox().height() == 220));
        // Right of the travel direction: below the first leg, 0.25 + 0.2 + 0.15 = 0.6 µm off centre.
        let Geometry::Path { points, width } = &on("metal1")[0].geom else { panic!() };
        assert_eq!((points[0], *width), ((0, -600), 300));
        assert_eq!(r.value, (3 + cuts.len()).to_string());
    }

    /// SKILL++ (`.ils`): closures keep their variables, and functions do not see their
    /// callers' locals; the same code in a `.il` file is dynamically scoped.
    #[test]
    fn skillpp_closures_are_lexical() {
        let dir = std::env::temp_dir().join(format!("ics-skillpp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let code = r#"
procedure(makeCounter() let(((n 0)) lambda(() n = n + 1)))
c = makeCounter()
funcall(c) funcall(c)
a = funcall(c)
d = makeCounter()
procedure(inner() y)
procedure(outer() let(((y 5)) inner()))
b = car(errset(outer()))
list(a funcall(d) b)
"#;
        let ils = dir.join("t.ils");
        std::fs::write(&ils, code).unwrap();
        let mut p = Project::new("t");
        let r = run_file(&mut p, &ils).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "(3 1 nil)");
        // Dynamic scoping in .il: inner sees outer's y.
        let il = dir.join("t.il");
        std::fs::write(&il, "procedure(inner() y)\nprocedure(outer() let(((y 5)) inner()))\nouter()").unwrap();
        assert_eq!(run_file(&mut Project::new("t"), &il).unwrap().value, "5");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A form-driven generator runs headless: the OK callback sees the field values.
    #[test]
    fn hi_forms_run_their_callback() {
        let src = r#"
procedure(myGenCB(form)
  let((cv w)
    w = form->wField->value
    cv = dbOpenCellViewByType("work" "gen" "layout" "maskLayout" "w")
    dbCreateRect(cv "metal1" list(0:0 w:form->hField->value))))
hiCreateAppForm(?name 'myGenForm ?formTitle "Generator"
  ?fields list(hiCreateFloatField(?name 'wField ?prompt "W" ?value 1.0)
               hiCreateFloatField(?name 'hField ?prompt "H" ?value 2.0))
  ?callback "myGenCB(hiGetCurrentForm())")
myGenForm->wField->value = 3.0
hiDisplayForm(myGenForm)
"#;
        let mut p = Project::new("t");
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "t");
        let l = p.cell(&CellRef::new("work", "gen")).and_then(|c| c.layout.clone()).unwrap();
        let k = p.tech.dbu_per_um;
        assert_eq!(l.shapes[0].bbox(), IRect::new(0, 0, 3 * k, 2 * k));
    }

    /// In an interactive session a displayed form comes back to the application, and OK with
    /// the edited values runs its callback.
    #[test]
    fn interactive_forms_wait_for_ok() {
        let src = r#"
procedure(myGenCB(form)
  let((cv)
    cv = dbOpenCellViewByType("work" "gen" "layout" "maskLayout" "w")
    when(form->big->value dbCreateRect(cv form->lay->value list(0:0 form->wField->value:1)))))
hiCreateAppForm(?name 'myGenForm ?formTitle "Generator"
  ?fields list(hiCreateFloatField(?name 'wField ?prompt "W" ?value 1.0)
               hiCreateBooleanButton(?name 'big ?buttonText "Draw" ?value t)
               hiCreateCyclicField(?name 'lay ?prompt "Layer" ?choices list("metal1" "metal2") ?value "metal1"))
  ?callback "myGenCB(hiGetCurrentForm())")
hiDisplayForm(myGenForm)
"#;
        let mut p = Project::new("t");
        let mut s = Session::new();
        s.interactive = true;
        let r = s.eval(&mut p, src, None, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.cell(&CellRef::new("work", "gen")).is_none(), "nothing runs before OK");
        let form = r.forms[0].clone();
        assert_eq!(form.title, "Generator");
        assert_eq!(
            form.fields[0],
            FormField { name: "wField".into(), prompt: "W".into(), kind: FieldKind::Float, value: "1.0".into() }
        );
        assert_eq!(form.fields[1].kind, FieldKind::Bool);
        assert_eq!(form.fields[2].kind, FieldKind::Choice(vec!["metal1".into(), "metal2".into()]));
        let mut fields = form.fields.clone();
        fields[0].value = "4".into();
        fields[2].value = "metal2".into();
        s.form_ok(&mut p, &form, &fields, None).unwrap_or_else(|e| panic!("{e}"));
        let l = p.cell(&CellRef::new("work", "gen")).and_then(|c| c.layout.clone()).unwrap();
        assert_eq!(l.shapes[0].bbox().width(), 4 * p.tech.dbu_per_um);
        assert_eq!(l.shapes[0].layer, p.tech.layer_index("metal2").unwrap());
    }

    fn error(src: &str) -> String {
        let mut p = Project::new("t");
        match run(&mut p, src, None) {
            Ok(r) => panic!("expected an error, got {}", r.value),
            Err(e) => e.message,
        }
    }

    /// Deep recursion, deep nesting in the source and deeply nested values end in SKILL errors
    /// (or work), never in a stack overflow, also when the caller has a small stack.
    #[test]
    fn deep_recursion_and_nesting_are_errors() {
        let small = std::thread::Builder::new().stack_size(512 << 10);
        let r = small
            .spawn(|| {
                let rec = "procedure(r(n) if(n == 0 then 0 else 1 + r(n - 1)))\n";
                let ok = eval(&format!("{rec}r(2000)"));
                let deep = error(&format!("{rec}r(100000000)"));
                // Recursion through built-ins and through deeply nested bodies.
                for body in [
                    "funcall('r n - 1)",
                    "car(mapcar(lambda((k) r(k)) list(n - 1)))",
                    &format!("{}r(n - 1){}", "progn(".repeat(900), ")".repeat(900)),
                    &format!("{}r(n - 1){}", "if(t then ".repeat(300), ")".repeat(300)),
                ] {
                    let e = error(&format!("procedure(r(n) if(n == 0 then 0 else 1 + {body}))\nr(100000000)"));
                    assert!(e.contains("depth limit") || e.contains("too deeply"), "{e}");
                }
                (ok, deep)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(r.0, "2000");
        assert!(r.1.contains("depth limit") || r.1.contains("too deeply"), "{}", r.1);

        // Source nesting.
        let nested = format!("{}1{}", "list(".repeat(5000), ")".repeat(5000));
        assert!(error(&nested).contains("nested too deeply"));
        let quoted = format!("'{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert!(error(&quoted).contains("nested too deeply"));
        let chain = format!("1{}", " + 1".repeat(5000));
        assert!(error(&chain).contains("nested too deeply"));
        assert_eq!(eval(&format!("1{}", " + 1".repeat(500))), "501");

        // A 200000 deep value: built, compared, printed and dropped without recursion.
        let deep = "x = nil for(i 1 200000 x = list(x)) y = nil for(i 1 200000 y = list(y))\n";
        assert_eq!(eval(&format!("{deep}list(length(x) equal(x y))")), "(1 t)");
        assert_eq!(output(&format!("{deep}print(x) 1")).len(), 2 * 200000 + 3);
        assert!(error(&format!("{deep}eval(x)")).contains("nested too deeply"));
        assert!(error(&format!("{deep}subst(1 nil x)")).contains("nested too deeply"));
        // Deep tables and closures drop iteratively too (here, on the 512 KB thread).
        std::thread::Builder::new()
            .stack_size(512 << 10)
            .spawn(|| {
                let mut p = Project::new("t");
                let mut s = Session::new();
                s.eval(&mut p, "tb = nil for(i 1 200000 u = makeTable(\"t\") u[0] = tb tb = u) 1", None, None).unwrap();
                s.eval(&mut p, "x = nil for(i 1 200000 x = list(x)) 1", None, None).unwrap();
                drop(s);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// A PCell body keeps `~>`, `->`, `a[i]` and quoted data exactly (it is stored as source).
    #[test]
    fn pcell_body_round_trips() {
        let src = r#"
pcDefinePCell(list(ddGetObj("mylib") "multi" "layout") ((w 1.0) (lay "metal1"))
  let((r box)
    declare(arr[2])
    arr[0] = w
    foreach(l '("metal1" "metal2") r = dbCreateRect(pcCellView l list(0:0 arr[0]:1)))
    box = r~>bBox
    when(pcCellView->cellName == "multi" && car(box) == '(0.0 0.0)
      dbCreateRect(pcCellView lay list(-1:-1 0:0)))
    case(lay ("metal1" t) (t nil))))
cv = dbOpenCellViewByType("work" "top" "layout" "maskLayout" "w")
i = dbCreateParamInstByMasterName(cv "mylib" "multi" "layout" "I1" 0:0 "R0" 1 list(list("w" "float" 2.0)))
length(i~>master~>shapes)
"#;
        let mut p = Project::new("t");
        let r = run(&mut p, src, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "2", "a variant has two rectangles");
        let m = p.cell(&CellRef::new("mylib", "multi")).and_then(|c| c.layout.clone()).unwrap();
        assert_eq!(m.shapes.len(), 3, "the master (cellName multi) also has the third");

        // Unparsed source parses back to the same code.
        for code in [
            "a~>b->c[1 + 2] = '(x \"y\" (1 -2.5) ?k @rest) f(?k -1 x - 1) (g 1 'h) { a b } if(a then b else c)",
            "x = -1 y = - x z = quote(f(x~>y 'z))",
        ] {
            let nodes = parse(code, 0).unwrap_or_else(|_| panic!("parse {code}"));
            let mut once = String::new();
            for n in &nodes {
                unparse(n, &mut once).unwrap();
                once.push('\n');
            }
            let again = parse(&once, 0).unwrap_or_else(|_| panic!("reparse {once}"));
            let mut twice = String::new();
            for n in &again {
                unparse(n, &mut twice).unwrap();
                twice.push('\n');
            }
            assert_eq!(once, twice);
        }
        assert_eq!(eval("quote(f(x~>y 'z a[1]))"), "(f (getq x y) (quote z) (arrayref a 1))");
        assert_eq!(
            eval("t1 = makeTable(\"t\") t1[1] = 5 o = list(nil 'p 7) list(eval(quote(o~>p)) eval(quote(t1[1])))"),
            "(7 5)"
        );
    }

    /// pcDefinePCell does not overwrite a drawn layout, and its edits are tracked.
    #[test]
    fn pcell_definition_keeps_drawn_layouts() {
        let mut p = Project::new("t");
        let draw = r#"cv = dbOpenCellViewByType("mylib" "hand" "layout" "maskLayout" "w")
dbCreateRect(cv "metal1" list(0:0 1:1))"#;
        run(&mut p, draw, None).unwrap();
        let def =
            r#"pcDefinePCell(list(ddGetObj("mylib") "hand" "layout") ((w 1.0)) dbCreateRect(pcCellView "metal2" list(0:0 w:w)))"#;
        let e = run(&mut p, def, None).unwrap_err();
        assert!(e.message.contains("already has a layout"), "{}", e.message);
        let hand = p.cell(&CellRef::new("mylib", "hand")).unwrap();
        assert_eq!(hand.layout.as_ref().unwrap().shapes.len(), 1);
        assert!(hand.pcell.is_none());
        // A new cell: created and reported as modified.
        let r = run(&mut p, &def.replace("\"hand\"", "\"gen\""), None).unwrap();
        assert!(r.modified.contains(&CellRef::new("mylib", "gen")), "{:?}", r.modified);
    }

    /// Redefining a SKILL PCell re-evaluates the variants already placed.
    #[test]
    fn pcell_redefinition_updates_variants() {
        let mut p = Project::new("t");
        let mut s = Session::new();
        let def1 =
            r#"pcDefinePCell(list(ddGetObj("mylib") "pc" "layout") ((w 1.0)) dbCreateRect(pcCellView "metal1" list(0:0 w:1)))"#;
        s.eval(&mut p, def1, None, None).unwrap();
        let place = r#"cv = dbOpenCellViewByType("work" "top" "layout" "maskLayout" "w")
dbCreateParamInstByMasterName(cv "mylib" "pc" "layout" "I1" 0:0 "R0" 1 list(list("w" "float" 2.0)))~>master~>cellName"#;
        let name = s.eval(&mut p, place, None, None).unwrap().value;
        let variant = CellRef::new("mylib", name.trim_matches('"'));
        let count = |p: &Project| p.cell(&variant).and_then(|c| c.layout.as_ref()).map_or(0, |l| l.shapes.len());
        assert_eq!(count(&p), 1);
        let def2 = def1.replace("list(0:0 w:1)))", "list(0:0 w:1)) dbCreateRect(pcCellView \"metal2\" list(0:0 w:2)))");
        let r = s.eval(&mut p, &def2, None, None).unwrap();
        assert_eq!(count(&p), 2, "the variant follows the new body");
        assert!(r.modified.contains(&variant));
    }

    /// Properties and file ports outlive one CIW command.
    #[test]
    fn session_keeps_properties_and_ports() {
        let dir = std::env::temp_dir().join(format!("ics-skill-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.txt");
        let mut p = Project::new("t");
        let mut s = Session::new();
        let src = format!(
            "cv = dbOpenCellViewByType(\"work\" \"a\" \"layout\" \"maskLayout\" \"w\") dbReplaceProp(cv \"note\" \"string\" \"hi\")\n\
             port = outfile(\"{}\")",
            path.display().to_string().replace('\\', "/")
        );
        s.eval(&mut p, &src, None, None).unwrap();
        assert_eq!(s.eval(&mut p, "cv~>note", None, None).unwrap().value, "\"hi\"");
        s.eval(&mut p, "fprintf(port \"line\\n\") close(port)", None, None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "line\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// SKILL++ closures share the variables they capture.
    #[test]
    fn skillpp_closures_share_variables() {
        let mut p = Project::new("t");
        let src = "let(((count 0)) procedure(incr() count = count + 1) procedure(getc() count)) incr() incr() getc()";
        let r = run(&mut p, src, Some(Path::new("counter.ils"))).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "2");
        let src = "mk = lambda(() let(((n 0)) list(lambda(() n = n + 1) lambda(() n))))\n\
                   c = funcall(mk) funcall(car(c)) funcall(car(c)) d = funcall(mk) funcall(car(d))\n\
                   list(funcall(cadr(c)) funcall(cadr(d)))";
        let r = run(&mut p, src, Some(Path::new("pairs.ils"))).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.value, "(2 1)");
    }

    /// `return(nil)` leaves the procedure once, without running anything again.
    #[test]
    fn return_nil_runs_side_effects_once() {
        let mut p = Project::new("t");
        let r = run(&mut p, "procedure(f() when(t printf(\"side\\n\") return(nil)) 1) f()", None).unwrap();
        assert_eq!((r.output.as_str(), r.value.as_str()), ("side\n", "nil"));
        let r = run(&mut p, "procedure(g() foreach(x '(1 2) printf(\"%d\" x) return(nil)) 1) g()", None).unwrap();
        assert_eq!((r.output.as_str(), r.value.as_str()), ("1", "nil"));
        assert_eq!(eval("procedure(h() let((a) if(t return(nil))) 1) h()"), "nil");
    }

    /// Runaway loops, huge formats and strings, unbounded ROD rows and backtracking patterns
    /// end in errors (or finish quickly).
    #[test]
    fn work_and_memory_are_bounded() {
        let mut p = Project::new("t");
        let mut s = Session::sandboxed(Sandbox { roots: vec![], time_limit: Some(std::time::Duration::from_millis(300)) });
        let e = s.eval(&mut p, "for(i 0 2000000000000)", None, None).unwrap_err();
        assert!(e.message.contains("time limit"), "{}", e.message);
        let e = s.eval(&mut p, "while(t)", None, None).unwrap_err();
        assert!(e.message.contains("time limit"), "{}", e.message);
        assert!(error("sprintf(nil \"%99999999999999d\" 1)").contains("width"));
        assert!(error("sprintf(nil \"%.99999999999f\" 1.0)").contains("precision"));
        assert!(error("s = \"x\" for(i 1 40 s = strcat(s s))").contains("longer than"));
        assert!(error("l = list(1) for(i 1 40 l = append(l l))").contains("longer than"));
        let rod = r#"cv = dbOpenCellViewByType("work" "r" "layout" "maskLayout" "w")
rodCreatePath(?cvId cv ?layer "metal1" ?width 0.5 ?pts list(0:0 1000000:0)
  ?subRect list(list(?layer "contact" ?width 0.001 ?space 0.001)))"#;
        assert!(error(rod).contains("rectangles"));
        let t = std::time::Instant::now();
        assert_eq!(eval(&format!("rexMatchp(\"{}b\" \"{}\")", "a*".repeat(30), "a".repeat(40))), "nil");
        assert!(t.elapsed().as_secs() < 2, "no exponential backtracking");
        for (pat, text, m) in [
            ("^ab*c$", "abbbc", "t"),
            ("^ab*c$", "abbbcd", "nil"),
            ("b.d", "abcde", "t"),
            ("^b", "abc", "nil"),
            ("c$", "abc", "t"),
            ("x*", "", "t"),
            ("a$*", "a$$", "t"),
            ("", "abc", "t"),
        ] {
            assert_eq!(eval(&format!("rexMatchp(\"{pat}\" \"{text}\")")), m, "{pat} on {text}");
        }
    }

    /// Integer edge cases are errors, not panics.
    #[test]
    fn integer_overflow_is_an_error() {
        let min = "(-9223372036854775807 - 1)";
        assert!(error(&format!("mod({min} -1)")).contains("overflow"));
        assert!(error(&format!("remainder({min} -1)")).contains("overflow"));
        assert!(error(&format!("modulo({min} -1)")).contains("overflow"));
        assert!(error(&format!("abs({min})")).contains("overflow"));
        assert!(error("expt(2 4294967296)").contains("overflow"));
        assert_eq!(eval(&format!("nthelem({min} '(1 2))")), "1");
        assert_eq!(eval("n = 0 for(i 9223372036854775806 9223372036854775807 n++) n"), "1");
        let rod = r#"cv = dbOpenCellViewByType("work" "r" "layout" "maskLayout" "w")
rodCreatePath(?cvId cv ?layer "metal1" ?width 0.5 ?pts list(0:0 1e300:0)
  ?subRect list(list(?layer "contact" ?width 0.2 ?space 0.2)) ?encSubPath list(list(?layer "metal2" ?enclosure 1e300)))"#;
        assert!(error(rod).contains("out of range"));
    }

    /// A sandbox confines file access to its roots and hides the environment.
    #[test]
    fn sandbox_limits_files_and_environment() {
        let base = std::env::temp_dir().join(format!("ics-skill-sandbox-{}", std::process::id()));
        let root = base.join("project");
        std::fs::create_dir_all(&root).unwrap();
        let outside = base.join("outside.txt");
        std::fs::write(&outside, "keep").unwrap();
        std::fs::write(root.join("lib.il"), "libValue = 42").unwrap();
        let mut p = Project::new("t");
        let mut s = Session::sandboxed(Sandbox { roots: vec![root.clone()], time_limit: None });
        let e = s.eval(&mut p, "outfile(\"../outside.txt\")", None, None).unwrap_err();
        assert!(e.message.contains("not allowed"), "{}", e.message);
        let abs = outside.display().to_string().replace('\\', "/");
        assert!(s.eval(&mut p, &format!("outfile(\"{abs}\" \"w\")"), None, None).is_err());
        assert!(s.eval(&mut p, &format!("infile(\"{abs}\")"), None, None).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep", "not truncated");
        s.eval(&mut p, "q = outfile(\"inside.txt\") fprintf(q \"ok\") close(q)", None, None).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("inside.txt")).unwrap(), "ok");
        assert_eq!(s.eval(&mut p, "load(\"lib.il\") libValue", None, None).unwrap().value, "42");
        assert!(s.eval(&mut p, "getShellEnvVar(\"PATH\")", None, None).is_err());
        assert!(s.load(&mut p, &outside, None).is_err());
        // Without a sandbox (the user's own scripts) everything stays available.
        assert!(run(&mut p, "getShellEnvVar(\"PATH\")", None).is_ok());
        std::fs::remove_dir_all(&base).ok();
    }
}
