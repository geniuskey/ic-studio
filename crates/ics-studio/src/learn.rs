//! The Learn course: the chapters of the user guide's "Learn IC design" part (`docs/learn`,
//! Korean in `docs/ko/learn`), built into the app and parsed into blocks for the Learn
//! window (`learn_view`). The guide's Markdown is the single source: the app adds what a web
//! page cannot do through HTML comments, which the web guide hides:
//!
//! ```text
//! <!-- try: Open the inverter schematic | open cmos_inverter schematic -->
//! <!-- check: The PMOS is in saturation | my_inv region M1 saturation -->
//! <!-- quiz
//! ? Which transistor conducts when the input is high?
//! - The PMOS
//! + The NMOS
//! > A high gate turns the NMOS on and the PMOS off.
//! -->
//! ```
//!
//! `<!-- explore: mosfet -->` places an explorable figure (sliders and a live curve, see
//! `explore_view`).
//!
//! A numeric quiz answers with `= 10 ±0.5 ns` instead of the `-`/`+` options. The label of a
//! `try` or `check` and the text of a quiz are in the file's language; the part after `|`
//! and the answers are the same in both languages (a test checks).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::app::Workspace;
use crate::i18n::Lang;

pub struct Chapter {
    pub id: &'static str,
    en: &'static str,
    ko: &'static str,
}

macro_rules! chapter {
    ($id:literal) => {
        Chapter {
            id: $id,
            en: include_str!(concat!("../../../docs/learn/", $id, ".md")),
            ko: include_str!(concat!("../../../docs/ko/learn/", $id, ".md")),
        }
    };
}

/// The course in reading order (as in `docs/SUMMARY.md`).
pub const CHAPTERS: &[Chapter] = &[
    chapter!("index"),
    chapter!("circuits"),
    chapter!("mosfet"),
    chapter!("cmos-logic"),
    chapter!("tutorial-inverter"),
    chapter!("layout-basics"),
    chapter!("tutorial-layout"),
    chapter!("analog-basics"),
    chapter!("tutorial-amplifier"),
    chapter!("tutorial-derive"),
    chapter!("glossary"),
];

impl Chapter {
    pub fn source(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::En => self.en,
            Lang::Ko => self.ko,
        }
    }
}

/// A chapter of the open project (`<project>/learn/*.md`), after the course's own: a teacher's
/// lab or a team's onboarding, with the same directives. `name.ko.md` next to `name.md` is
/// its Korean version.
pub struct ProjectChapter {
    /// `project:<file stem>`, the key of its progress.
    pub id: &'static str,
    en: Vec<Block>,
    ko: Option<Vec<Block>>,
}

thread_local! {
    /// The open project's chapters, on the UI thread (one per app; the UI tests run apps in
    /// parallel threads). Leaked when replaced: they are few and small.
    static PROJECT: std::cell::RefCell<Vec<&'static ProjectChapter>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The folder of a project's chapters.
pub fn project_dir(root: &std::path::Path) -> std::path::PathBuf {
    root.join("learn")
}

/// Load the chapters of the project at `root` (none: no project), replacing the last ones.
/// Returns how many were found.
pub fn load_project_chapters(root: Option<&std::path::Path>) -> usize {
    let mut found: Vec<&'static ProjectChapter> = vec![];
    if let Some(dir) = root.map(project_dir)
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        let mut files: Vec<std::path::PathBuf> = entries
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "md") && !p.to_string_lossy().ends_with(".ko.md"))
            .collect();
        files.sort();
        for f in files {
            let Ok(en) = std::fs::read_to_string(&f) else { continue };
            let stem = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ko = std::fs::read_to_string(f.with_extension("ko.md")).ok().map(|s| parse(&s));
            let id: &'static str = Box::leak(format!("project:{stem}").into_boxed_str());
            found.push(Box::leak(Box::new(ProjectChapter { id, en: parse(&en), ko })));
        }
    }
    let n = found.len();
    PROJECT.with_borrow_mut(|p| *p = found);
    n
}

fn project_chapter(i: usize) -> Option<&'static ProjectChapter> {
    let i = i.checked_sub(CHAPTERS.len())?;
    PROJECT.with_borrow(|p| p.get(i).copied())
}

/// How many chapters there are: the course's and the open project's.
pub fn count() -> usize {
    CHAPTERS.len() + PROJECT.with_borrow(|p| p.len())
}

/// The id of chapter `i`: its file name, `project:<name>` for the project's.
pub fn id(i: usize) -> &'static str {
    CHAPTERS.get(i).map(|c| c.id).or_else(|| project_chapter(i).map(|c| c.id)).unwrap_or("")
}

/// Whether chapter `i` comes from the open project.
pub fn is_project(i: usize) -> bool {
    i >= CHAPTERS.len()
}

/// The index of a chapter by its file name (`mosfet` or `mosfet.md`; `project:lab1`).
pub fn chapter_index(id: &str) -> Option<usize> {
    let id = id.strip_suffix(".md").unwrap_or(id);
    (0..count()).find(|&i| self::id(i) == id)
}

/// The parsed blocks of chapter `i` in `lang`.
pub fn blocks(lang: Lang, i: usize) -> &'static [Block] {
    static EN: OnceLock<Vec<Vec<Block>>> = OnceLock::new();
    static KO: OnceLock<Vec<Vec<Block>>> = OnceLock::new();
    if let Some(c) = project_chapter(i) {
        return match (&c.ko, lang) {
            (Some(ko), Lang::Ko) => ko,
            _ => &c.en,
        };
    }
    let cell = if lang == Lang::Ko { &KO } else { &EN };
    cell.get_or_init(|| CHAPTERS.iter().map(|c| parse(c.source(lang))).collect()).get(i).map_or(&[], |b| b.as_slice())
}

/// The chapter title: its first heading.
pub fn title(lang: Lang, i: usize) -> &'static str {
    blocks(lang, i)
        .iter()
        .find_map(|b| match b {
            Block::Heading { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or(id(i))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
        slug: String,
    },
    /// Inline Markdown (see `learn_view::inline`).
    Para(String),
    /// Items with their nesting depth (0 = top level).
    List {
        ordered: bool,
        items: Vec<(usize, String)>,
    },
    Code {
        lang: String,
        text: String,
    },
    Table {
        head: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Quote(String),
    Image {
        alt: String,
        path: String,
    },
    Try {
        label: String,
        action: Action,
    },
    Quiz(Quiz),
    Check(Check),
    /// An explorable figure (`explore_view::NAMES`): sliders and a live curve.
    Explore(String),
    Rule,
}

/// What a "Try it" button does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Open the demo project (the cells the course uses).
    Demo,
    /// Open the topic example projects folder by name (File › Open Example).
    Example(String),
    Open(String, Option<Workspace>),
    /// Open the cell and simulate it.
    Simulate(String),
    Drc(String),
    Lvs(String),
    Derive(String),
    NewCell,
    Calculator,
    Tutorial,
    Palette,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quiz {
    pub question: String,
    pub answer: Answer,
    pub explain: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Choice {
        options: Vec<String>,
        correct: usize,
    },
    /// A number in `unit`, right within `tol`.
    Number {
        value: f64,
        tol: f64,
        unit: String,
    },
}

/// A condition on the user's own design, checked live while the chapter is open.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub label: String,
    pub cell: String,
    pub kind: CheckKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckKind {
    /// The cell has a schematic with at least `devices` devices.
    Schematic {
        devices: usize,
    },
    Layout,
    Simulated,
    /// A calculator expression on the last results, within the limits (SPICE numbers, empty
    /// for none).
    Measure {
        analysis: String,
        expr: String,
        min: String,
        max: String,
    },
    /// A device's operating region in the last operating point.
    Region {
        device: String,
        region: String,
    },
    DrcClean,
    LvsClean,
}

/// The result of a [`Check`], computed by the app.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckState {
    Pass(String),
    /// What is wrong, and what to try (plain-language advice).
    Fail(String, Option<String>),
    /// Not possible to tell yet (no such cell, not simulated, ...): what to do first.
    Waiting(String),
}

/// What the user has done in the course, kept in the settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    /// Chapters marked as done.
    pub done: BTreeSet<String>,
    /// Quizzes (`chapter#n`) and whether the last answer was right.
    pub quiz: BTreeMap<String, bool>,
    /// The chapter last open.
    pub last: Option<String>,
    /// The window is docked at the right side of the main window instead of floating.
    pub docked: bool,
}

impl Progress {
    pub fn quiz_key(chapter: usize, n: usize) -> String {
        format!("{}#{n}", id(chapter))
    }

    /// The chapter to continue with: the last one open, or the first not done.
    pub fn resume(&self) -> usize {
        self.last
            .as_deref()
            .and_then(chapter_index)
            .or_else(|| (0..count()).position(|i| !self.done.contains(id(i))))
            .unwrap_or(0)
    }

    pub fn chapters_done(&self) -> usize {
        (0..count()).filter(|&i| self.done.contains(id(i))).count()
    }
}

/// The anchor of a heading as mdBook makes it: lower case, spaces to `-`, letters (any
/// script), digits, `-` and `_` kept.
pub fn slug(text: &str) -> String {
    let plain = strip_inline(text);
    plain
        .trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Inline text without its Markdown and HTML marks.
pub fn strip_inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            rest = &rest[end + 1..];
            continue;
        }
        if c == '['
            && let Some((label, after)) = rest[1..].split_once("](")
            && let Some(end) = after.find(')')
        {
            out.push_str(&strip_inline(label));
            rest = &after[end + 1..];
            continue;
        }
        if !matches!(c, '*' | '`' | '\\') {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The glossary chapter's terms, for the hover help on terms in the other chapters.
pub struct Glossary {
    /// The term as written and its meaning (inline Markdown).
    pub terms: Vec<(String, String)>,
    /// Lower-case names (the bold term, its parenthesised aliases) to `terms`.
    keys: BTreeMap<String, usize>,
}

fn gloss_key(s: &str) -> String {
    strip_inline(s).trim().to_lowercase()
}

impl Glossary {
    fn new(blocks: &[Block]) -> Glossary {
        let mut g = Glossary { terms: vec![], keys: BTreeMap::new() };
        for b in blocks {
            let Block::Table { rows, .. } = b else { continue };
            for row in rows {
                let [term, meaning, ..] = row.as_slice() else { continue };
                let i = g.terms.len();
                g.terms.push((strip_inline(term), meaning.clone()));
                let mut names: Vec<&str> = vec![];
                for (n, part) in term.split("**").enumerate() {
                    if n % 2 == 1 {
                        names.push(part);
                    } else if let Some(inner) = part.split_once('(').and_then(|(_, r)| r.split_once(')')).map(|(x, _)| x) {
                        names.extend(inner.split(','));
                    }
                }
                for name in names {
                    let bare = name.split('(').next().unwrap_or(name);
                    for part in [name, bare].into_iter().chain(name.split([',', '/'])) {
                        let k = gloss_key(part);
                        if k.chars().count() >= 2 {
                            g.keys.entry(k).or_insert(i);
                        }
                    }
                }
            }
        }
        g
    }

    /// The entry of a term as it appears in a chapter (also its plural).
    pub fn lookup(&self, text: &str) -> Option<&(String, String)> {
        let k = gloss_key(text);
        let i = self.keys.get(&k).or_else(|| {
            let singular = k.strip_suffix("es").filter(|s| s.ends_with('s')).or_else(|| k.strip_suffix('s'))?;
            self.keys.get(singular)
        })?;
        self.terms.get(*i)
    }
}

/// A term's meaning from the glossary in the UI language, as plain text for a tooltip.
pub fn glossary_tip(term: &str) -> Option<String> {
    let (name, meaning) = glossary(crate::i18n::current()).lookup(term)?;
    Some(format!("{name}: {}", strip_inline(meaning)))
}

/// Add the glossary's meaning of `term` to a widget's tooltip, after `tip` if any.
pub fn glossary_hover(resp: eframe::egui::Response, term: &str, tip: Option<&str>) -> eframe::egui::Response {
    match (tip, glossary_tip(term)) {
        (Some(a), Some(g)) => resp.on_hover_text(format!("{a}\n\n{g}")),
        (Some(a), None) => resp.on_hover_text(a),
        (None, Some(g)) => resp.on_hover_text(g),
        (None, None) => resp,
    }
}

/// The glossary in `lang`.
pub fn glossary(lang: Lang) -> &'static Glossary {
    static EN: OnceLock<Glossary> = OnceLock::new();
    static KO: OnceLock<Glossary> = OnceLock::new();
    let cell = if lang == Lang::Ko { &KO } else { &EN };
    cell.get_or_init(|| Glossary::new(chapter_index("glossary").map_or(&[], |i| blocks(lang, i))))
}

/// Split a directive's arguments on blanks, keeping `"quoted strings"` whole.
fn words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn workspace(w: Option<&String>) -> Result<Option<Workspace>, String> {
    Ok(match w.map(|s| s.as_str()) {
        None => None,
        Some("schematic") => Some(Workspace::Schematic),
        Some("simulation") => Some(Workspace::Simulation),
        Some("layout") => Some(Workspace::Layout),
        Some(o) => return Err(format!("unknown workspace '{o}'")),
    })
}

pub fn parse_action(s: &str) -> Result<Action, String> {
    let w = words(s);
    let arg = |i: usize| w.get(i).cloned().ok_or_else(|| format!("'{s}': missing argument"));
    Ok(match w.first().map(|s| s.as_str()) {
        Some("demo") => Action::Demo,
        Some("example") => Action::Example(arg(1)?),
        Some("open") => Action::Open(arg(1)?, workspace(w.get(2))?),
        Some("simulate") => Action::Simulate(arg(1)?),
        Some("drc") => Action::Drc(arg(1)?),
        Some("lvs") => Action::Lvs(arg(1)?),
        Some("derive") => Action::Derive(arg(1)?),
        Some("new-cell") => Action::NewCell,
        Some("calculator") => Action::Calculator,
        Some("tutorial") => Action::Tutorial,
        Some("palette") => Action::Palette,
        _ => return Err(format!("unknown action '{s}'")),
    })
}

pub fn parse_check(label: &str, s: &str) -> Result<Check, String> {
    let w = words(s);
    let arg = |i: usize| w.get(i).cloned().ok_or_else(|| format!("'{s}': missing argument"));
    let kind = match w.get(1).map(|s| s.as_str()) {
        Some("schematic") => CheckKind::Schematic {
            devices: w.get(2).map(|n| n.parse().map_err(|_| format!("'{s}': bad count"))).transpose()?.unwrap_or(1),
        },
        Some("layout") => CheckKind::Layout,
        Some("simulated") => CheckKind::Simulated,
        Some("measure") => CheckKind::Measure { analysis: arg(2)?, expr: arg(3)?, min: arg(4)?, max: arg(5)? },
        Some("region") => CheckKind::Region { device: arg(2)?, region: arg(3)? },
        Some("drc") => CheckKind::DrcClean,
        Some("lvs") => CheckKind::LvsClean,
        _ => return Err(format!("unknown check '{s}'")),
    };
    Ok(Check { label: label.trim().to_string(), cell: arg(0)?, kind })
}

/// `10 ±0.5 ns`, `10 +-0.5 ns` or `10 ns` (exact to 0.1 %).
fn parse_number(s: &str) -> Result<Answer, String> {
    let s = s.replace("+-", "±");
    let (value, rest) = match s.split_once('±') {
        Some((v, r)) => (v.trim().to_string(), r.trim().to_string()),
        None => {
            let mut it = s.trim().splitn(2, ' ');
            (it.next().unwrap_or("").to_string(), format!("_ {}", it.next().unwrap_or("")))
        }
    };
    let value: f64 = value.parse().map_err(|_| format!("bad number in '= {s}'"))?;
    let mut it = rest.splitn(2, ' ');
    let tol = it.next().unwrap_or("");
    let unit = it.next().unwrap_or("").trim().to_string();
    let tol = if tol == "_" { value.abs() * 1e-3 } else { tol.parse().map_err(|_| format!("bad tolerance in '= {s}'"))? };
    Ok(Answer::Number { value, tol, unit })
}

fn parse_quiz(body: &str) -> Result<Quiz, String> {
    let (mut question, mut explain) = (String::new(), String::new());
    let (mut options, mut correct, mut number) = (vec![], None, None);
    let mut last = ' ';
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (mark, text) = (line.chars().next().unwrap(), line[1..].trim());
        let append = |s: &mut String| {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(text);
        };
        match mark {
            '?' => append(&mut question),
            '>' => append(&mut explain),
            '-' => options.push(text.to_string()),
            '+' => {
                if correct.replace(options.len()).is_some() {
                    return Err("a quiz has one right answer".into());
                }
                options.push(text.to_string());
            }
            '=' => number = Some(parse_number(text)?),
            _ => match last {
                '?' => append(&mut question),
                '>' => append(&mut explain),
                _ => return Err(format!("quiz line '{line}' starts with none of ? - + = >")),
            },
        }
        if matches!(mark, '?' | '>') {
            last = mark;
        }
    }
    let answer = match (number, correct) {
        (Some(n), None) if options.is_empty() => n,
        (None, Some(correct)) if options.len() >= 2 => Answer::Choice { options, correct },
        _ => return Err(format!("quiz '{question}': give options with one '+', or '= number'")),
    };
    if question.is_empty() {
        return Err("quiz without a '?' question".into());
    }
    Ok(Quiz { question, answer, explain })
}

/// The directive in an HTML comment (`None` for a plain comment such as the SEO header).
fn directive(comment: &str) -> Option<Result<Block, String>> {
    let c = comment.trim();
    if let Some(body) = c.strip_prefix("quiz") {
        return Some(parse_quiz(body).map(Block::Quiz));
    }
    let (kind, rest) = c.split_once(':')?;
    let (label, args) = rest.split_once('|').unwrap_or(("", rest));
    match kind.trim() {
        "try" => Some(parse_action(args).map(|action| Block::Try { label: label.trim().to_string(), action })),
        "check" => Some(parse_check(label, args).map(Block::Check)),
        "explore" => {
            let name = args.trim();
            Some(if crate::explore_view::NAMES.contains(&name) {
                Ok(Block::Explore(name.to_string()))
            } else {
                Err(format!("unknown explorable figure '{name}'"))
            })
        }
        _ => None,
    }
}

fn table_row(line: &str) -> Vec<String> {
    let line = line.trim().trim_start_matches('|').trim_end_matches('|');
    let mut cells = vec![];
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => cur.push(chars.next().unwrap()),
            '|' => cells.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn list_item(line: &str) -> Option<(usize, bool, &str)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return Some((indent / 2, false, rest));
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(rest) = t[digits..].strip_prefix(". ")
    {
        return Some((indent / 3, true, rest));
    }
    None
}

/// The blocks of a chapter. Directive errors become paragraphs that say so (a test keeps
/// the shipped chapters free of them).
pub fn parse(md: &str) -> Vec<Block> {
    try_parse(md).unwrap_or_else(|e| vec![Block::Para(format!("**{e}**"))])
}

pub fn try_parse(md: &str) -> Result<Vec<Block>, String> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = vec![];
    let mut i = 0;
    let mut para = String::new();
    let flush = |para: &mut String, out: &mut Vec<Block>| {
        if !para.trim().is_empty() {
            out.push(Block::Para(std::mem::take(para).trim().to_string()));
        }
        para.clear();
    };
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
        } else if t.starts_with("<!--") {
            flush(&mut para, &mut out);
            let mut comment = t.trim_start_matches("<!--").to_string();
            while !comment.contains("-->") && i + 1 < lines.len() {
                i += 1;
                comment.push('\n');
                comment.push_str(lines[i]);
            }
            let comment = comment.split("-->").next().unwrap_or("");
            if let Some(b) = directive(comment) {
                out.push(b?);
            }
            i += 1;
        } else if t.starts_with("```") {
            flush(&mut para, &mut out);
            let lang = t.trim_start_matches('`').trim().to_string();
            let mut text = vec![];
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                text.push(lines[i]);
                i += 1;
            }
            out.push(Block::Code { lang, text: text.join("\n") });
            i += 1;
        } else if t.starts_with('#') && t.trim_start_matches('#').starts_with(' ') {
            flush(&mut para, &mut out);
            let level = t.chars().take_while(|c| *c == '#').count() as u8;
            let text = t[level as usize..].trim().to_string();
            out.push(Block::Heading { level, slug: slug(&text), text });
            i += 1;
        } else if t.starts_with("![") && t.ends_with(')') {
            flush(&mut para, &mut out);
            let (alt, path) = t[2..t.len() - 1].split_once("](").unwrap_or(("", ""));
            out.push(Block::Image { alt: alt.to_string(), path: path.to_string() });
            i += 1;
        } else if t == "---" || t == "***" {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
            i += 1;
        } else if t.starts_with('|') {
            flush(&mut para, &mut out);
            let head = table_row(t);
            let mut rows = vec![];
            i += 1;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                let row = table_row(lines[i]);
                if !row.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':'))) {
                    rows.push(row);
                }
                i += 1;
            }
            out.push(Block::Table { head, rows });
        } else if let Some(q) = t.strip_prefix('>') {
            flush(&mut para, &mut out);
            let mut text = q.trim().to_string();
            i += 1;
            while i < lines.len() && lines[i].trim().starts_with('>') {
                text.push(' ');
                text.push_str(lines[i].trim().trim_start_matches('>').trim());
                i += 1;
            }
            out.push(Block::Quote(text));
        } else if let Some((_, ordered, _)) = list_item(line) {
            flush(&mut para, &mut out);
            let mut items: Vec<(usize, String)> = vec![];
            while i < lines.len() {
                let l = lines[i];
                if let Some((depth, _, text)) = list_item(l) {
                    items.push((depth, text.trim().to_string()));
                } else if !l.trim().is_empty() && l.starts_with(' ') && !items.is_empty() {
                    let last = items.last_mut().unwrap();
                    last.1.push(' ');
                    last.1.push_str(l.trim());
                } else {
                    break;
                }
                i += 1;
            }
            out.push(Block::List { ordered, items });
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(t);
            i += 1;
        }
    }
    flush(&mut para, &mut out);
    Ok(out)
}

/// The cells a chapter works on: those its "Try it" buttons open and its checks look at.
pub fn chapter_cells(blocks: &[Block]) -> BTreeSet<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Try {
                action: Action::Open(c, _) | Action::Simulate(c) | Action::Drc(c) | Action::Lvs(c) | Action::Derive(c),
                ..
            } => Some(c.clone()),
            Block::Check(c) => Some(c.cell.clone()),
            _ => None,
        })
        .collect()
}

/// The language-independent shape of a chapter: its directives with their actions and
/// answers, and its figures. Both languages of a chapter must have the same.
pub fn skeleton(blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Image { path, .. } => Some(format!("image {}", path.rsplit('/').next().unwrap_or(path))),
            Block::Try { action, .. } => Some(format!("try {action:?}")),
            Block::Check(c) => Some(format!("check {} {:?}", c.cell, c.kind)),
            Block::Explore(name) => Some(format!("explore {name}")),
            Block::Quiz(q) => Some(match &q.answer {
                Answer::Choice { options, correct } => format!("quiz {} options, #{correct}", options.len()),
                Answer::Number { value, tol, unit } => format!("quiz = {value} ±{tol} {unit}"),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives() {
        let md = "# Title\n\nSome *text*\nmore.\n\n<!-- try: Open it | open cmos_inverter schematic -->\n\
                  <!-- check: Fast enough | my_inv measure tran \"delay(v(in), 0.9, v(out), 0.9, rise, fall)\" \"\" 50p -->\n\
                  <!-- quiz\n? Which one?\n- a\n+ b\n> Because.\n-->\n<!-- quiz\n? How long?\n= 10 ±0.5 ns\n-->\n\
                  1. one\n   continued\n   - nested\n2. two\n\n| a | b |\n|---|---|\n| 1 | x \\| y |\n";
        let b = try_parse(md).unwrap();
        assert_eq!(b[0], Block::Heading { level: 1, text: "Title".into(), slug: "title".into() });
        assert_eq!(b[1], Block::Para("Some *text* more.".into()));
        assert_eq!(
            b[2],
            Block::Try { label: "Open it".into(), action: Action::Open("cmos_inverter".into(), Some(Workspace::Schematic)) }
        );
        let Block::Check(c) = &b[3] else { panic!() };
        assert_eq!(c.cell, "my_inv");
        assert_eq!(
            c.kind,
            CheckKind::Measure {
                analysis: "tran".into(),
                expr: "delay(v(in), 0.9, v(out), 0.9, rise, fall)".into(),
                min: "".into(),
                max: "50p".into()
            }
        );
        let Block::Quiz(q) = &b[4] else { panic!() };
        assert_eq!(q.answer, Answer::Choice { options: vec!["a".into(), "b".into()], correct: 1 });
        assert_eq!(q.explain, "Because.");
        let Block::Quiz(q) = &b[5] else { panic!() };
        assert_eq!(q.answer, Answer::Number { value: 10.0, tol: 0.5, unit: "ns".into() });
        assert_eq!(
            b[6],
            Block::List { ordered: true, items: vec![(0, "one continued".into()), (1, "nested".into()), (0, "two".into())] }
        );
        assert_eq!(b[7], Block::Table { head: vec!["a".into(), "b".into()], rows: vec![vec!["1".into(), "x | y".into()]] });
        assert_eq!(slug("Step 8: the delay (transient)"), "step-8-the-delay-transient");
        assert_eq!(slug("**V<sub>M</sub>** and `W`"), "vm-and-w");
        assert!(try_parse("<!-- try: x | fly away -->").is_err());
        assert!(try_parse("<!-- quiz\n? q\n- a\n- b\n-->").is_err());
    }

    /// Every chapter parses in both languages, and the Korean one has the same directives,
    /// answers and figures as the English one.
    #[test]
    fn chapters_match_in_both_languages() {
        for c in CHAPTERS {
            let en = try_parse(c.en).unwrap_or_else(|e| panic!("{} (en): {e}", c.id));
            let ko = try_parse(c.ko).unwrap_or_else(|e| panic!("{} (ko): {e}", c.id));
            assert_eq!(skeleton(&en), skeleton(&ko), "directives and figures of {} differ between en and ko", c.id);
            for b in en.iter().chain(&ko) {
                if let Block::Try { label, .. } | Block::Check(Check { label, .. }) = b {
                    assert!(!label.is_empty(), "{}: a try/check without a label", c.id);
                }
            }
        }
    }

    #[test]
    fn glossary_terms() {
        let g = glossary(Lang::En);
        assert!(g.terms.len() > 50);
        assert!(g.lookup("MOSFET").is_some_and(|(_, m)| m.contains("field-effect")));
        assert_eq!(g.lookup("transistors").map(|(t, _)| t.as_str()), g.lookup("transistor").map(|(t, _)| t.as_str()));
        assert!(g.lookup("diffusion").is_some_and(|(t, _)| t.starts_with("Active")));
        assert!(g.lookup("V<sub>th</sub>").is_some() || g.lookup("threshold voltage").is_some());
        assert!(g.lookup("operating point").is_some(), "a bold name's own parenthesis is dropped");
        assert!(g.lookup("gm").is_some() && g.lookup("vth").is_some(), "symbols, as the simulator names them");
        let ko = glossary(Lang::Ko);
        assert!(ko.lookup("문턱 전압").is_some(), "Korean aliases are looked up");
        assert!(ko.lookup("MOSFET").is_some());
    }

    /// Links between chapters point at chapters and headings that exist, in both languages.
    #[test]
    fn chapter_links_resolve() {
        for lang in [Lang::En, Lang::Ko] {
            for (i, c) in CHAPTERS.iter().enumerate() {
                let src = c.source(lang);
                for (_, after) in src.match_indices("](").map(|(at, _)| src.split_at(at + 2)) {
                    let target = &after[..after.find(')').unwrap_or(0)];
                    if target.starts_with("http")
                        || target.starts_with("../")
                        || !target.contains(".md") && !target.starts_with('#')
                    {
                        continue;
                    }
                    let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
                    let j = if file.is_empty() {
                        i
                    } else {
                        chapter_index(file).unwrap_or_else(|| panic!("{}: link to {file}", c.id))
                    };
                    if !anchor.is_empty() {
                        let found = blocks(lang, j).iter().any(|b| matches!(b, Block::Heading { slug, .. } if slug == anchor));
                        assert!(found, "{} ({lang:?}): no heading #{anchor} in {}", c.id, CHAPTERS[j].id);
                    }
                }
            }
        }
    }
}
