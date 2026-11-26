//! The Learn window: the course chapters (`learn`) rendered in the app, in the UI language,
//! with "Try it" buttons that drive the real app, quizzes, and live checks of the user's own
//! design. Reading next to the canvas is the point, so it is a window, not a workspace.

use std::collections::HashMap;

use eframe::egui::{self, Align, Color32, RichText, Sense, Stroke, TextFormat, Vec2, text::LayoutJob};
use ics_core::CellRef;

use crate::app::{Command, Workspace};
use crate::i18n::{self, t, tf};
use crate::learn::{self, Action, Answer, Block, CHAPTERS, Check, CheckState, Progress};
use crate::theme;

/// The course's figures, by file name (the chapters link them as `../images/<name>`).
const FIGURES: &[(&str, &[u8])] = &[
    ("design-loop.svg", include_bytes!("../../../docs/images/design-loop.svg")),
    ("drc-check.svg", include_bytes!("../../../docs/images/drc-check.svg")),
    ("layout-drawing.svg", include_bytes!("../../../docs/images/layout-drawing.svg")),
    ("learn-chip-zoom.svg", include_bytes!("../../../docs/images/learn-chip-zoom.svg")),
    ("learn-cmos-inverter.svg", include_bytes!("../../../docs/images/learn-cmos-inverter.svg")),
    ("learn-cs-amp.svg", include_bytes!("../../../docs/images/learn-cs-amp.svg")),
    ("learn-layer-stack.svg", include_bytes!("../../../docs/images/learn-layer-stack.svg")),
    ("learn-mosfet-channel.svg", include_bytes!("../../../docs/images/learn-mosfet-channel.svg")),
    ("learn-mosfet-iv.svg", include_bytes!("../../../docs/images/learn-mosfet-iv.svg")),
    ("learn-nand2.svg", include_bytes!("../../../docs/images/learn-nand2.svg")),
    ("learn-rc-charge.svg", include_bytes!("../../../docs/images/learn-rc-charge.svg")),
    ("lvs-compare.svg", include_bytes!("../../../docs/images/lvs-compare.svg")),
    ("pex-backannotate.svg", include_bytes!("../../../docs/images/pex-backannotate.svg")),
    ("sdl-flow.svg", include_bytes!("../../../docs/images/sdl-flow.svg")),
];

/// The figure of an image path, by its file name.
pub fn figure(path: &str) -> Option<&'static [u8]> {
    let name = path.rsplit('/').next()?;
    FIGURES.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// The online user guide, where a link out of the course goes.
const GUIDE: &str = "https://pages.euiyun.com/ic-studio/";

/// What the window needs from the app.
pub struct Env<'a> {
    /// The cell of that name in the open project.
    pub find_cell: &'a dyn Fn(&str) -> Option<CellRef>,
    pub check: &'a dyn Fn(&Check) -> CheckState,
    pub project_open: bool,
    /// Names of the open project's cells, to pick the one a check looks at.
    pub cells: &'a [String],
}

#[derive(Default)]
struct QuizState {
    choice: Option<usize>,
    text: String,
    /// Whether the submitted answer was right.
    result: Option<bool>,
}

#[derive(Default)]
pub struct LearnView {
    pub open: bool,
    pub chapter: usize,
    quizzes: HashMap<(usize, usize), QuizState>,
    /// A heading to scroll to (`#anchor`), or the top of the chapter (empty).
    goto: Option<String>,
    toc: bool,
    figures: HashMap<String, FigureState>,
    /// Cells the user picked for the course's cell names (`my_inv` → `inverter1`).
    pub aliases: HashMap<String, String>,
    explore: HashMap<String, crate::explore_view::State>,
}

/// A figure on screen: its texture and what it shows.
#[derive(Default)]
struct FigureState {
    texture: Option<(egui::TextureHandle, (i18n::Lang, i64, u32))>,
    /// Time shown, seconds into the animation loop (`None` until first drawn: the poster).
    t: Option<f64>,
    playing: bool,
}

/// A course figure parsed once.
fn parsed_figure(name: &str) -> Option<&'static crate::figure::Figure> {
    static FIGS: std::sync::OnceLock<HashMap<&'static str, crate::figure::Figure>> = std::sync::OnceLock::new();
    FIGS.get_or_init(|| {
        FIGURES.iter().filter_map(|(n, b)| Some((*n, crate::figure::Figure::new(std::str::from_utf8(b).ok()?)))).collect()
    })
    .get(name)
}

/// The Korean text of a figure text in the Korean UI.
fn figure_text(en: &str) -> Option<&'static str> {
    if i18n::current() != i18n::Lang::Ko {
        return None;
    }
    crate::i18n_ko::figures::FIGURES.iter().find(|(k, _)| *k == en).map(|(_, v)| *v)
}

/// A link target clicked in the text.
enum Link {
    Chapter(usize, String),
    Url(String),
}

impl LearnView {
    /// Open the window at a chapter, or where the user left off.
    pub fn show(&mut self, chapter: Option<usize>, progress: &Progress) {
        self.open = true;
        self.chapter = chapter.unwrap_or_else(|| progress.resume()).min(learn::count() - 1);
        self.goto = Some(String::new());
        self.toc = true;
    }

    /// Mark the chapter done and move to the next one.
    fn next(&mut self, progress: &mut Progress) {
        progress.done.insert(learn::id(self.chapter).to_string());
        self.go(self.chapter + 1, String::new(), progress);
    }

    /// Show a chapter at a heading (`slug`).
    #[doc(hidden)]
    pub fn go_to(&mut self, chapter: usize, anchor: &str, progress: &mut Progress) {
        self.open = true;
        self.go(chapter, anchor.to_string(), progress);
    }

    fn go(&mut self, chapter: usize, anchor: String, progress: &mut Progress) {
        self.chapter = chapter.min(learn::count() - 1);
        self.goto = Some(anchor);
        progress.last = Some(learn::id(self.chapter).to_string());
    }

    /// The Learn window; returns the commands its buttons ran.
    pub fn window(&mut self, ctx: &egui::Context, progress: &mut Progress, env: &Env) -> Vec<Command> {
        let mut q = vec![];
        if !self.open || progress.docked {
            return q;
        }
        let mut open = true;
        let screen = ctx.content_rect();
        egui::Window::new(crate::theme::window_title(t("Learn IC Design")))
            .id(egui::Id::unique("learn_window"))
            .open(&mut open)
            .collapsible(true)
            .resizable([true, false])
            .default_size(Vec2::new(720.0, (screen.height() - 200.0).clamp(220.0, 760.0) + 50.0))
            .default_pos(screen.right_top() + Vec2::new(-760.0, 60.0))
            .min_width(420.0)
            .show(ctx, |ui| {
                // The height follows the screen: the reading area is as tall as fits, the
                // chapter navigation stays visible below it.
                let body = (screen.height() - 200.0).clamp(220.0, 760.0);
                crate::spot::mark(ui.ctx(), "learn", ui.max_rect());
                self.contents(ui, progress, env, &mut q, body)
            });
        self.open &= open;
        if self.open {
            progress.last = Some(learn::id(self.chapter).to_string());
        }
        q
    }

    /// The course docked at the side of the main window (`progress.docked`).
    pub fn panel(&mut self, ui: &mut egui::Ui, progress: &mut Progress, env: &Env) -> Vec<Command> {
        let mut q = vec![];
        let pal = theme::palette(ui.ctx());
        crate::spot::mark(ui.ctx(), "learn", ui.max_rect());
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Learn IC Design")).strong().color(pal.text_strong));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("×").on_hover_text(t("Close")).clicked() {
                    self.open = false;
                }
            });
        });
        let body = (ui.available_height() - 50.0).max(160.0);
        self.contents(ui, progress, env, &mut q, body);
        if self.open {
            progress.last = Some(learn::id(self.chapter).to_string());
        }
        q
    }

    fn contents(&mut self, ui: &mut egui::Ui, progress: &mut Progress, env: &Env, q: &mut Vec<Command>, height: f32) {
        let pal = theme::palette(ui.ctx());
        let lang = i18n::current();
        let n = learn::count();
        ui.horizontal_wrapped(|ui| {
            let (dock, tip) = if progress.docked {
                (t("Float"), t("Show the course in its own window"))
            } else {
                (t("Dock"), t("Show the course at the right side, next to the canvas instead of over it"))
            };
            if ui.small_button(dock).on_hover_text(tip).clicked() {
                progress.docked = !progress.docked;
                if progress.docked {
                    self.toc = false;
                }
            }
            let toc = if self.toc { t("Hide contents") } else { t("Contents") };
            if ui.small_button(toc).clicked() {
                self.toc = !self.toc;
            }
            if ui.add_enabled(self.chapter > 0, egui::Button::new(t("‹ Previous")).small()).clicked() {
                self.go(self.chapter - 1, String::new(), progress);
            }
            if ui.add_enabled(self.chapter + 1 < n, egui::Button::new(t("Next ›")).small()).clicked() {
                self.next(progress);
            }
            let id = learn::id(self.chapter);
            let mut done = progress.done.contains(id);
            if ui.checkbox(&mut done, t("Chapter done")).changed() {
                if done {
                    progress.done.insert(id.to_string());
                } else {
                    progress.done.remove(id);
                }
            }
            if ui
                .small_button(t("Report…"))
                .on_hover_text(t("Save a progress report: chapters done, quizzes and checks"))
                .clicked()
            {
                q.push(Command::ExportLearnReport);
            }
            ui.add(egui::ProgressBar::new(progress.chapters_done() as f32 / n as f32).desired_width(60.0).desired_height(6.0))
                .on_hover_text(t("Chapters marked as done"));
            ui.label(
                RichText::new(tf("{} of {} chapters done", &[&progress.chapters_done().to_string(), &n.to_string()]))
                    .small()
                    .color(pal.text_weak),
            );
        });
        ui.separator();
        // Hands-on chapters work on the inspector at the right, which the floating window covers.
        if !progress.docked && learn::id(self.chapter).starts_with("tutorial-") {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t(
                        "This chapter has you work in the inspector on the right: dock the course so it does not cover it.",
                    ))
                    .small()
                    .color(pal.warning),
                );
                if ui.small_button(t("Dock now")).clicked() {
                    progress.docked = true;
                    self.toc = false;
                }
            });
        }
        ui.horizontal_top(|ui| {
            if self.toc {
                ui.vertical(|ui| {
                    ui.set_width(180.0);
                    egui::ScrollArea::vertical().id_salt("learn_toc").auto_shrink([false, false]).max_height(height).show(
                        ui,
                        |ui| {
                            for i in 0..learn::count() {
                                if i == CHAPTERS.len() {
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(t("This project")).small().strong().color(pal.text_weak));
                                        if ui
                                            .small_button(t("Reload"))
                                            .on_hover_text(t("Read the project's learn folder again"))
                                            .clicked()
                                        {
                                            q.push(Command::ReloadProjectChapters);
                                        }
                                    });
                                }
                                let done = progress.done.contains(learn::id(i));
                                ui.horizontal(|ui| {
                                    let (r, p) = ui.allocate_painter(Vec2::splat(12.0), Sense::hover());
                                    let c = r.rect.center();
                                    if done {
                                        p.circle_filled(c, 5.0, pal.ok);
                                    } else {
                                        let col = if i == self.chapter { pal.accent } else { pal.border };
                                        p.circle_stroke(c, 4.5, Stroke::new(1.5, col));
                                    }
                                    let text = RichText::new(learn::strip_inline(learn::title(lang, i)));
                                    let text = if i == self.chapter { text.strong() } else { text.color(pal.text_weak) };
                                    if ui.add(egui::Label::new(text).wrap().sense(Sense::click())).clicked() {
                                        self.go(i, String::new(), progress);
                                    }
                                });
                            }
                        },
                    );
                });
                ui.separator();
            }
            ui.vertical(|ui| {
                let goto = self.goto.take();
                let mut area = egui::ScrollArea::vertical()
                    .id_salt(("learn_body", self.chapter))
                    .auto_shrink([false, false])
                    .max_height(height);
                if goto.as_deref() == Some("") {
                    area = area.vertical_scroll_offset(0.0);
                }
                area.show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(760.0));
                    let link = self.chapter_body(ui, progress, env, q, goto.as_deref());
                    if self.chapter + 1 < n {
                        ui.add_space(12.0);
                        let next = learn::strip_inline(learn::title(lang, self.chapter + 1));
                        if ui.button(tf("Next chapter: {} ›", &[&next])).clicked() {
                            self.next(progress);
                        }
                        ui.add_space(12.0);
                    }
                    match link {
                        Some(Link::Chapter(i, anchor)) => self.go(i, anchor, progress),
                        Some(Link::Url(u)) => ui.ctx().open_url(egui::OpenUrl::new_tab(u)),
                        None => {}
                    }
                });
            });
        });
    }

    /// The blocks of the current chapter; returns a clicked link.
    fn chapter_body(
        &mut self,
        ui: &mut egui::Ui,
        progress: &mut Progress,
        env: &Env,
        q: &mut Vec<Command>,
        goto: Option<&str>,
    ) -> Option<Link> {
        let pal = theme::palette(ui.ctx());
        let lang = i18n::current();
        let mut link = None;
        let mut quiz_n = 0;
        let gloss = learn::id(self.chapter) != "glossary";
        for block in learn::blocks(lang, self.chapter) {
            match block {
                Block::Heading { level, text, slug } => {
                    ui.add_space(if *level == 1 { 2.0 } else { 10.0 });
                    let size = match level {
                        1 => 22.0,
                        2 => 17.0,
                        _ => 14.5,
                    };
                    let r = ui.label(RichText::new(learn::strip_inline(text)).size(size).strong().color(pal.text_strong));
                    if goto == Some(slug.as_str()) {
                        r.scroll_to_me(Some(Align::TOP));
                    }
                    ui.add_space(2.0);
                }
                Block::Para(text) => {
                    link = link.or(inline_with(ui, text, None, gloss));
                    ui.add_space(6.0);
                }
                Block::List { ordered, items } => {
                    let mut numbers = [0usize; 8];
                    for (depth, text) in items {
                        let d = (*depth).min(7);
                        numbers[d] += 1;
                        numbers[d + 1..].iter_mut().for_each(|n| *n = 0);
                        let bullet = if *ordered && d == 0 { format!("{}.", numbers[d]) } else { "•".to_string() };
                        ui.horizontal_top(|ui| {
                            ui.add_space(6.0 + 16.0 * d as f32);
                            ui.add_sized(Vec2::new(18.0, 14.0), egui::Label::new(RichText::new(bullet).color(pal.text_weak)));
                            ui.vertical(|ui| link = link.take().or(inline_with(ui, text, None, gloss)));
                        });
                    }
                    ui.add_space(6.0);
                }
                Block::Code { text, .. } => {
                    egui::Frame::new().fill(pal.bg_canvas).corner_radius(6).inner_margin(8).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::horizontal().id_salt(text.len()).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).extend());
                        });
                    });
                    ui.add_space(6.0);
                }
                Block::Table { head, rows } => {
                    let cols = head.len().max(1);
                    let w = ((ui.available_width() - 12.0 * cols as f32) / cols as f32).max(60.0);
                    egui::Frame::new().stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(6).show(ui, |ui| {
                        egui::Grid::new(("learn_table", self.chapter, head.join("|"))).striped(true).spacing([12.0, 4.0]).show(
                            ui,
                            |ui| {
                                for h in head {
                                    ui.vertical(|ui| {
                                        ui.set_max_width(w);
                                        link = link.take().or(inline_with(ui, &format!("**{h}**"), None, false));
                                    });
                                }
                                ui.end_row();
                                for row in rows {
                                    for c in row {
                                        ui.vertical(|ui| {
                                            ui.set_max_width(w);
                                            link = link.take().or(inline_with(ui, c, None, gloss));
                                        });
                                    }
                                    ui.end_row();
                                }
                            },
                        );
                    });
                    ui.add_space(6.0);
                }
                Block::Quote(text) => {
                    egui::Frame::new().fill(pal.bg_widget).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 6)).show(
                        ui,
                        |ui| {
                            ui.set_width(ui.available_width());
                            link = link.take().or(inline_with(ui, text, None, gloss));
                        },
                    );
                    ui.add_space(6.0);
                }
                Block::Image { alt, path } => {
                    let name = path.rsplit('/').next().unwrap_or(path).to_string();
                    figure_card(ui, alt, &name, self.figures.entry(name.clone()).or_default());
                }
                Block::Rule => {
                    ui.separator();
                }
                Block::Explore(name) => crate::explore_view::show(ui, name, self.explore.entry(name.clone()).or_default()),
                Block::Try { label, action } => try_button(ui, label, action, env, q),
                Block::Check(c) => {
                    let mut c = c.clone();
                    let course = c.cell.clone();
                    if let Some(a) = self.aliases.get(&course) {
                        c.cell = a.clone();
                    }
                    let state = (env.check)(&c);
                    let title = learn::strip_inline(learn::title(lang, self.chapter));
                    if let Some(pick) = check_row(ui, &c, &course, &state, env, q, &title) {
                        self.aliases.insert(course, pick);
                    }
                }
                Block::Quiz(quiz) => {
                    let key = Progress::quiz_key(self.chapter, quiz_n);
                    let state = self.quizzes.entry((self.chapter, quiz_n)).or_default();
                    if let Some(right) = quiz_card(ui, quiz, state, progress.quiz.get(&key).copied(), quiz_n, q) {
                        progress.quiz.insert(key, right);
                    }
                    quiz_n += 1;
                }
            }
        }
        link
    }
}

/// Inline Markdown: `**bold**`, `*italic*`, `` `code` ``, `[links](x.md#a)`,
/// `<sub>`/`<sup>` and `<br>`. Returns the link clicked.
fn inline(ui: &mut egui::Ui, text: &str, color: Option<Color32>) -> Option<Link> {
    inline_with(ui, text, color, true)
}

/// [`inline`], with the glossary's meaning shown on bold terms (`gloss`).
fn inline_with(ui: &mut egui::Ui, text: &str, color: Option<Color32>, gloss: bool) -> Option<Link> {
    let pal = theme::palette(ui.ctx());
    let base = egui::TextStyle::Body.resolve(ui.style());
    let color = color.unwrap_or(pal.text);
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let mut links: Vec<(std::ops::Range<usize>, String)> = vec![];
    let (mut bold, mut italic, mut small) = (false, false, false);
    let mut chars = 0usize;
    let push =
        |job: &mut LayoutJob, s: &str, bold: bool, italic: bool, code: bool, small: bool, link: bool, chars: &mut usize| {
            let mut f = TextFormat::simple(base.clone(), if bold { pal.text_strong } else { color });
            if code {
                f.font_id = egui::FontId::monospace(base.size * 0.92);
                f.background = pal.bg_widget;
            }
            if small {
                f.font_id.size *= 0.75;
            }
            f.italics = italic;
            if link {
                f.color = pal.accent;
                f.underline = Stroke::new(1.0, pal.accent.gamma_multiply(0.5));
            }
            *chars += s.chars().count();
            job.append(s, 0.0, f);
        };
    let mut rest = text;
    let mut plain = String::new();
    // Bold runs: their characters, bytes and text (a term looked up in the glossary).
    let mut bolds: Vec<(std::ops::Range<usize>, std::ops::Range<usize>, String)> = vec![];
    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                let (c0, b0) = (chars, job.text.len());
                push(&mut job, &plain, bold, italic, false, small, false, &mut chars);
                if bold {
                    match bolds.last_mut() {
                        Some(last) if last.0.end == c0 => {
                            last.0.end = chars;
                            last.1.end = job.text.len();
                            last.2.push_str(&plain);
                        }
                        _ => bolds.push((c0..chars, b0..job.text.len(), plain.clone())),
                    }
                }
                plain.clear();
            }
        };
    }
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("**") {
            flush!();
            bold = !bold;
            rest = &rest[2..];
        } else if (c == '*' || c == '_' && plain.ends_with(' ')) && rest[1..].contains(c) && !rest[1..].starts_with(' ') {
            flush!();
            italic = !italic;
            rest = &rest[1..];
        } else if c == '*' && italic {
            flush!();
            italic = false;
            rest = &rest[1..];
        } else if c == '`' {
            flush!();
            let end = rest[1..].find('`').map_or(rest.len(), |e| e + 1);
            push(&mut job, &rest[1..end], bold, false, true, small, false, &mut chars);
            rest = &rest[(end + 1).min(rest.len())..];
        } else if c == '[' && rest[1..].contains("](") {
            let (label, after) = rest[1..].split_once("](").unwrap();
            match after.find(')') {
                Some(end) if !label.contains(']') => {
                    flush!();
                    let start = chars;
                    push(&mut job, &learn::strip_inline(label), bold, italic, false, small, true, &mut chars);
                    links.push((start..chars, after[..end].to_string()));
                    rest = &after[end + 1..];
                }
                _ => {
                    plain.push(c);
                    rest = &rest[1..];
                }
            }
        } else if c == '<' {
            let end = rest.find('>').unwrap_or(0);
            let tag = &rest[1..end.max(1)];
            match tag {
                "sub" | "sup" => {
                    flush!();
                    small = true;
                }
                "/sub" | "/sup" => {
                    flush!();
                    small = false;
                }
                "br" | "br/" | "br /" => plain.push('\n'),
                _ if end == 0 => plain.push('<'),
                _ => {}
            }
            rest = &rest[end + 1..];
        } else if c == '\\' && rest.len() > 1 {
            let next = rest[1..].chars().next().unwrap();
            plain.push(next);
            rest = &rest[1 + next.len_utf8()..];
        } else {
            plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    flush!();
    let glossary = learn::glossary(i18n::current());
    let terms: Vec<(std::ops::Range<usize>, &(String, String))> =
        if gloss { bolds.iter().filter_map(|(c, _, text)| Some((c.clone(), glossary.lookup(text)?))).collect() } else { vec![] };
    for (c, b, _) in &bolds {
        if terms.iter().any(|(r, _)| r == c) {
            for sec in job.sections.iter_mut().filter(|s| {
                s.byte_range.start >= egui::text::ByteIndex(b.start) && s.byte_range.end <= egui::text::ByteIndex(b.end)
            }) {
                sec.format.underline = Stroke::new(1.0, pal.text_weak);
            }
        }
    }
    let sense = if links.is_empty() { Sense::hover() } else { Sense::click() };
    let (pos, galley, resp) = egui::Label::new(job).sense(sense).layout_in_ui(ui);
    ui.painter().galley(pos, galley.clone(), color);
    let hovered = resp.hover_pos().map(|p| galley.cursor_from_pos(p - pos).index.0);
    if let Some(at) = hovered
        && let Some((_, (term, meaning))) = terms.iter().find(|(r, _)| r.contains(&at))
    {
        resp.clone().on_hover_ui_at_pointer(|ui| {
            ui.set_max_width(340.0);
            ui.label(RichText::new(term).strong().color(pal.text_strong));
            inline_with(ui, meaning, None, false);
            ui.label(RichText::new(t("From the glossary")).small().color(pal.text_weak));
        });
    }
    if links.is_empty() {
        return None;
    }
    let pointer = resp.hover_pos()?;
    let at = galley.cursor_from_pos(pointer - pos).index.0;
    let (_, target) = links.iter().find(|(r, _)| r.contains(&at))?;
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    if !resp.clicked() {
        return None;
    }
    Some(resolve_link(target))
}

fn resolve_link(target: &str) -> Link {
    if target.starts_with("http") {
        return Link::Url(target.to_string());
    }
    let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
    if let Some(i) = learn::chapter_index(file) {
        return Link::Chapter(i, anchor.to_string());
    }
    // Other pages of the user guide: online, in the language of the UI.
    let page = file.trim_start_matches("../").replace(".md", ".html");
    let lang = if i18n::current() == i18n::Lang::Ko { "ko/" } else { "" };
    let anchor = if anchor.is_empty() { String::new() } else { format!("#{anchor}") };
    Link::Url(format!("{GUIDE}{lang}{page}{anchor}"))
}

/// A figure: its description, and a button that opens the (animated) picture in the browser.
fn figure_card(ui: &mut egui::Ui, alt: &str, name: &str, st: &mut FigureState) {
    let pal = theme::palette(ui.ctx());
    let Some(fig) = parsed_figure(name) else {
        ui.label(RichText::new(alt).italics().color(pal.text_weak));
        return;
    };
    let lang = i18n::current();
    let now = ui.input(|i| i.stable_dt).min(0.1) as f64;
    let mut at = st.t.unwrap_or(fig.poster);
    if st.playing && fig.period > 0.0 {
        at = (at + now) % fig.period;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
    }
    st.t = Some(at);
    let width = ui.available_width().min(760.0);
    let ppp = ui.ctx().pixels_per_point();
    let px = (width * ppp).round() as u32;
    // Re-render at about 12 frames a second while it plays.
    let key = (lang, (at * 12.0).round() as i64, px);
    if st.texture.as_ref().is_none_or(|(_, k)| *k != key) {
        let svg = fig.snapshot(Some(at), &figure_text);
        let view = roxmltree::Document::parse(&svg)
            .ok()
            .and_then(|d| d.root_element().attribute("width").and_then(|w| w.parse::<f32>().ok()))
            .unwrap_or(800.0);
        if let Some(img) = crate::figure::render(&svg, px as f32 / view) {
            let opts = egui::TextureOptions::LINEAR;
            match &mut st.texture {
                Some((tex, k)) => {
                    tex.set(img, opts);
                    *k = key;
                }
                None => st.texture = Some((ui.ctx().load_texture(format!("learn_fig_{name}"), img, opts), key)),
            }
        }
    }
    if let Some((tex, _)) = &st.texture {
        let size = tex.size_vec2() / ppp;
        ui.add(egui::Image::new((tex.id(), size)).corner_radius(8)).on_hover_text(alt);
    }
    ui.horizontal_wrapped(|ui| {
        if fig.period > 0.0 {
            let label = if st.playing { t("Pause") } else { t("Play") };
            if ui.small_button(label).on_hover_text(t("Play the figure's animation")).clicked() {
                st.playing = !st.playing;
            }
            let mut x = at;
            let l = ui.label(RichText::new(t("Time")).small().color(pal.text_weak));
            let r = ui
                .add(egui::Slider::new(&mut x, 0.0..=fig.period).show_value(false).clamping(egui::SliderClamping::Always))
                .labelled_by(l.id);
            if r.changed() {
                st.t = Some(x);
                st.playing = false;
            }
        }
        if ui.small_button(t("Open in browser")).on_hover_text(t("Show the animated figure in your web browser")).clicked() {
            let dir = std::env::temp_dir().join("ic-studio-learn");
            let file = dir.join(format!("{}{name}", if lang == i18n::Lang::Ko { "ko-" } else { "" }));
            let svg = fig.snapshot(None, &figure_text);
            if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, svg)).is_ok() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file://{}", file.display())));
            }
        }
    });
    ui.label(RichText::new(alt).small().italics().color(pal.text_weak));
    ui.add_space(8.0);
}

/// The commands of a "Try it" action, or why it cannot run yet.
fn action_commands(action: &Action, env: &Env) -> Result<Vec<Command>, String> {
    let cell = |name: &str| {
        (env.find_cell)(name).ok_or_else(|| {
            if env.project_open {
                tf("The open project has no cell {}. Open the demo project first.", &[name])
            } else {
                t("Open the demo project first.").to_string()
            }
        })
    };
    Ok(match action {
        Action::Demo => vec![Command::NewDemoProject],
        Action::Example(name) => vec![Command::OpenExample(name.clone())],
        Action::Open(name, ws) => vec![Command::OpenCell(cell(name)?, *ws)],
        Action::Simulate(name) => vec![Command::OpenCell(cell(name)?, Some(Workspace::Simulation)), Command::RunSimulation],
        Action::Drc(name) => vec![Command::OpenCell(cell(name)?, Some(Workspace::Layout)), Command::RunDrc],
        Action::Lvs(name) => vec![Command::OpenCell(cell(name)?, Some(Workspace::Layout)), Command::RunLvs],
        Action::Derive(name) => {
            let r = cell(name)?;
            vec![Command::OpenCell(r.clone(), Some(Workspace::Schematic)), Command::DeriveDialog(r)]
        }
        Action::NewCell => {
            if !env.project_open {
                return Err(t("Open or create a project first.").to_string());
            }
            vec![Command::NewCellDialog]
        }
        Action::Calculator => vec![Command::Calculator],
        Action::Tutorial => vec![Command::Tutorial],
        Action::Palette => vec![Command::CommandPalette],
    })
}

fn try_button(ui: &mut egui::Ui, label: &str, action: &Action, env: &Env, q: &mut Vec<Command>) {
    let pal = theme::palette(ui.ctx());
    let cmds = action_commands(action, env);
    egui::Frame::new()
        .fill(pal.accent.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, pal.accent.gamma_multiply(0.5)))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("Try it")).strong().color(pal.accent));
                let b = ui.add_enabled(cmds.is_ok(), egui::Button::new(label));
                match &cmds {
                    Ok(c) => {
                        if b.clicked() {
                            q.extend(c.iter().cloned());
                        }
                    }
                    Err(why) => {
                        ui.label(RichText::new(why).small().color(pal.warning));
                        if (!env.project_open || (env.find_cell)("cmos_inverter").is_none())
                            && ui.small_button(t("Open demo project")).clicked()
                        {
                            q.push(Command::NewDemoProject);
                        }
                    }
                }
            });
        });
    ui.add_space(6.0);
}

/// Where a check's result is found or made: the place to point at, and the workspace showing it.
fn check_anchor(c: &Check, state: &CheckState) -> (&'static str, Workspace) {
    use learn::CheckKind as K;
    let waiting = matches!(state, CheckState::Waiting(_));
    match &c.kind {
        K::Schematic { .. } => ("sch.library", Workspace::Schematic),
        K::Layout => ("ws.layout", Workspace::Layout),
        K::Simulated => ("sim.run", Workspace::Simulation),
        K::Measure { .. } if waiting => ("sim.run", Workspace::Simulation),
        K::Measure { .. } => ("sim.measurements", Workspace::Simulation),
        K::Region { .. } if waiting => ("sim.run", Workspace::Simulation),
        K::Region { .. } => ("sim.results", Workspace::Simulation),
        K::DrcClean => ("lay.run_drc", Workspace::Layout),
        K::LvsClean => ("lay.verify", Workspace::Layout),
    }
}

/// What to say about the place a check points at.
fn anchor_note(c: &Check, state: &CheckState) -> String {
    use learn::CheckKind as K;
    let waiting = matches!(state, CheckState::Waiting(_));
    match &c.kind {
        K::Schematic { .. } => t("Pick a device here, then click on the canvas to place it.").into(),
        K::Layout => t("The layout of the cell opens here.").into(),
        K::Simulated => t("Press this to simulate (F5).").into(),
        K::Measure { .. } | K::Region { .. } if waiting => t("Press this to simulate (F5).").into(),
        K::Measure { .. } => t("The measured values are in this table.").into(),
        K::Region { device, .. } => tf("The Operating Point tab lists every transistor with its region. Look at {}.", &[device]),
        K::DrcClean => t("Run the design rule check here.").into(),
        K::LvsClean => t("Run LVS here and read the differences below.").into(),
    }
}

/// A check on the user's design; returns a cell the user picked to check instead of
/// `course` (the cell name the chapter uses).
fn check_row(
    ui: &mut egui::Ui,
    c: &Check,
    course: &str,
    state: &CheckState,
    env: &Env,
    q: &mut Vec<Command>,
    chapter: &str,
) -> Option<String> {
    let pal = theme::palette(ui.ctx());
    let mut pick = None;
    let (col, mark, detail, hint) = match state {
        CheckState::Pass(d) => (pal.ok, true, d, None),
        CheckState::Fail(d, h) => (pal.danger, false, d, h.as_ref()),
        CheckState::Waiting(d) => (pal.text_weak, false, d, None),
    };
    egui::Frame::new()
        .stroke(Stroke::new(1.0, col.gamma_multiply(0.6)))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, p) = ui.allocate_painter(Vec2::splat(16.0), Sense::hover());
                let ctr = r.rect.center();
                if mark {
                    p.circle_filled(ctr, 7.0, pal.ok);
                    let s = Stroke::new(2.0, Color32::WHITE);
                    p.line_segment([ctr + Vec2::new(-3.5, 0.0), ctr + Vec2::new(-1.0, 2.5)], s);
                    p.line_segment([ctr + Vec2::new(-1.0, 2.5), ctr + Vec2::new(3.5, -2.5)], s);
                } else {
                    p.circle_stroke(ctr, 6.0, Stroke::new(1.5, col));
                }
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(t("Check")).small().strong().color(col));
                        ui.label(RichText::new(&c.label).strong());
                    });
                    ui.label(RichText::new(detail).small().color(pal.text_weak));
                    if let Some(h) = hint {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new(t("What to try:")).small().strong().color(pal.warning));
                            ui.add(egui::Label::new(RichText::new(h).small()).wrap());
                        });
                    }
                    let cell = (env.find_cell)(&c.cell);
                    // The course names its cells (my_inv); the user may have named theirs otherwise.
                    if cell.is_none() && env.project_open && !env.cells.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            let l = ui.label(RichText::new(t("Or check another cell:")).small().color(pal.text_weak));
                            let shown = if c.cell == course { t("choose…").to_string() } else { c.cell.clone() };
                            egui::ComboBox::from_id_salt(("learn_check_cell", course, &c.label))
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    for name in env.cells {
                                        if ui.selectable_label(*name == c.cell, name).clicked() {
                                            pick = Some(name.clone());
                                        }
                                    }
                                })
                                .response
                                .labelled_by(l.id);
                        });
                    } else if c.cell != course {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(tf("Checking {} (the course calls it {})", &[&c.cell, course]))
                                    .small()
                                    .color(pal.text_weak),
                            );
                            if ui.small_button(t("Use the course name")).clicked() {
                                pick = Some(course.to_string());
                            }
                        });
                    }
                    if !mark {
                        ui.horizontal_wrapped(|ui| {
                            let (key, ws) = check_anchor(c, state);
                            if ui
                                .small_button(t("Show me where"))
                                .on_hover_text(t("Point at the place in the window where this is done"))
                                .clicked()
                            {
                                match &cell {
                                    Some(r) => q.push(Command::OpenCell(r.clone(), Some(ws))),
                                    None => q.push(Command::SetWorkspace(ws)),
                                }
                                crate::spot::point(ui.ctx(), key, &anchor_note(c, state), Some(8.0));
                            }
                            if ui
                                .small_button(t("Ask the tutor"))
                                .on_hover_text(t("The copilot in Tutor mode guides you through it step by step"))
                                .clicked()
                            {
                                let mut ask = tf(
                                    "I am working on the Learn chapter \"{}\". The check \"{}\" on my cell {} says: {}.",
                                    &[chapter, &c.label, &c.cell, detail],
                                );
                                if let Some(h) = hint {
                                    ask += &format!(" {}", tf("The advice shown is: {}", &[h]));
                                }
                                ask += &format!(" {}", t("Guide me step by step so I can fix it myself, and explain why."));
                                q.push(Command::AskTutor(ask));
                            }
                        });
                    }
                });
            });
        });
    ui.add_space(6.0);
    pick
}

/// A quiz; returns whether a submitted answer was right.
fn quiz_card(
    ui: &mut egui::Ui,
    quiz: &learn::Quiz,
    s: &mut QuizState,
    before: Option<bool>,
    n: usize,
    q: &mut Vec<Command>,
) -> Option<bool> {
    let pal = theme::palette(ui.ctx());
    let mut submitted = None;
    egui::Frame::new()
        .fill(pal.simulation.gamma_multiply(0.06))
        .stroke(Stroke::new(1.0, pal.simulation.gamma_multiply(0.45)))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("Quiz")).small().strong().color(pal.simulation));
                if before == Some(true) && s.result.is_none() {
                    ui.label(RichText::new(t("answered right before")).small().color(pal.ok));
                }
            });
            inline(ui, &quiz.question, Some(pal.text_strong));
            ui.add_space(4.0);
            match &quiz.answer {
                Answer::Choice { options, correct } => {
                    for (i, o) in options.iter().enumerate() {
                        let picked = s.choice == Some(i);
                        let mut text = RichText::new(learn::strip_inline(o));
                        if s.result.is_some() && i == *correct {
                            text = text.color(pal.ok).strong();
                        } else if s.result == Some(false) && picked {
                            text = text.color(pal.danger);
                        }
                        if ui.radio(picked, text).clicked() {
                            s.choice = Some(i);
                            s.result = None;
                        }
                    }
                    ui.add_space(2.0);
                    if ui.add_enabled(s.choice.is_some() && s.result.is_none(), egui::Button::new(t("Check answer"))).clicked() {
                        let right = s.choice == Some(*correct);
                        s.result = Some(right);
                        submitted = Some(right);
                    }
                }
                Answer::Number { value, tol, unit } => {
                    ui.horizontal(|ui| {
                        let l = ui.label(t("Answer"));
                        let r = ui
                            .add(egui::TextEdit::singleline(&mut s.text).id_salt(("quiz", n)).desired_width(90.0))
                            .labelled_by(l.id);
                        if r.changed() {
                            s.result = None;
                        }
                        ui.label(unit);
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.add_enabled(!s.text.trim().is_empty(), egui::Button::new(t("Check answer"))).clicked() || enter {
                            let x = s.text.trim().replace(',', ".");
                            let x = x.parse::<f64>().ok().or_else(|| ics_core::units::parse_spice(&x));
                            let right = x.is_some_and(|x| (x - value).abs() <= *tol);
                            s.result = Some(right);
                            submitted = Some(right);
                        }
                    });
                }
            }
            if let Some(right) = s.result {
                ui.add_space(4.0);
                let (text, col) =
                    if right { (t("Right!"), pal.ok) } else { (t("Not quite. Try again, or read the explanation."), pal.danger) };
                ui.label(RichText::new(text).strong().color(col));
                if !quiz.explain.is_empty() {
                    inline(ui, &quiz.explain, Some(pal.text_weak));
                }
                if let Answer::Number { value, unit, .. } = &quiz.answer
                    && !right
                {
                    ui.label(RichText::new(tf("Answer: {}", &[&format!("{value} {unit}")])).small().color(pal.text_weak));
                }
                if !right
                    && ui
                        .small_button(t("Ask the tutor why"))
                        .on_hover_text(t("The copilot in Tutor mode explains the idea behind the question"))
                        .clicked()
                {
                    let mine = match &quiz.answer {
                        Answer::Choice { options, .. } => s.choice.map(|i| learn::strip_inline(&options[i])).unwrap_or_default(),
                        Answer::Number { unit, .. } => format!("{} {unit}", s.text.trim()),
                    };
                    q.push(Command::AskTutor(tf(
                        "In the Learn course I answered the quiz question \"{}\" with \"{}\", which is wrong. Help me \
                         understand why, step by step, with a question I can think about rather than just the answer.",
                        &[&learn::strip_inline(&quiz.question), &mine],
                    )));
                }
            }
        });
    ui.add_space(8.0);
    submitted
}

/// The state of a check on the user's design, from the cell and its last results.
pub fn evaluate(
    c: &Check,
    cell: Option<&ics_core::project::Cell>,
    results: Option<&ics_sim::SimResults>,
    drc: Option<&ics_core::drc::DrcReport>,
    lvs: Option<&ics_core::lvs::LvsReport>,
) -> CheckState {
    use ics_core::schematic::DeviceKind;
    use learn::CheckKind as K;
    let Some(cell) = cell else {
        return CheckState::Waiting(tf("Create the cell {} first.", &[&c.cell]));
    };
    let need_sim = || CheckState::Waiting(tf("Simulate {} (F5) to check this.", &[&c.cell]));
    match &c.kind {
        K::Schematic { devices } => {
            let n = cell
                .schematic
                .as_ref()
                .map_or(0, |s| s.instances.iter().filter(|i| !matches!(i.kind, DeviceKind::Ground | DeviceKind::Supply)).count());
            if n >= *devices {
                CheckState::Pass(tf("{} devices placed (at least {} needed)", &[&n.to_string(), &devices.to_string()]))
            } else {
                CheckState::Waiting(tf("{} of {} devices placed", &[&n.to_string(), &devices.to_string()]))
            }
        }
        K::Layout => match &cell.layout {
            Some(l) if !l.shapes.is_empty() || !l.instances.is_empty() => CheckState::Pass(t("The cell has a layout.").into()),
            _ => CheckState::Waiting(t("The cell has no layout yet.").into()),
        },
        K::Simulated => match results {
            Some(_) => CheckState::Pass(t("Simulated.").into()),
            None => need_sim(),
        },
        K::Measure { analysis, expr, min, max } => {
            let Some(res) = results else { return need_sim() };
            let spec = ics_core::sweep::Spec {
                name: c.label.clone(),
                analysis: analysis.clone(),
                expr: expr.clone(),
                min: min.clone(),
                max: max.clone(),
            };
            let limits = crate::guide_view::spec_limits(&spec);
            match ics_sim::optimize::measure(&spec, res) {
                Err(e) if res.dataset(analysis).is_none() => {
                    CheckState::Waiting(tf("Add a {} analysis and simulate again ({}).", &[analysis, &e]))
                }
                Err(e) => {
                    let hint = ics_core::advice::error_advice(&e).map(|a| i18n::t_text(a).to_string());
                    CheckState::Fail(i18n::t_text(&e).to_string(), hint)
                }
                Ok(v) => {
                    let text = tf("measured {} (target {})", &[&ics_core::units::format_eng(v, ""), &limits]);
                    if spec.check(v).unwrap_or(true) {
                        CheckState::Pass(text)
                    } else {
                        let hint = ics_core::advice::spec_miss(min, max, v)
                            .map(|m| i18n::t_text(&ics_core::advice::spec_advice(expr, m)).to_string());
                        CheckState::Fail(text, hint)
                    }
                }
            }
        }
        K::Region { device, region } => {
            let Some(res) = results else { return need_sim() };
            let Some(op) = res.dataset("op") else {
                return CheckState::Waiting(tf("Enable the operating point analysis and simulate {}.", &[&c.cell]));
            };
            match op.devices.iter().find(|d| d.name.eq_ignore_ascii_case(device)) {
                None => CheckState::Fail(
                    tf("No device {} in the operating point.", &[device]),
                    Some(t("Device names come from the schematic: select the transistor and read its name in the inspector.").into()),
                ),
                Some(d) => {
                    let got = d.region.clone().unwrap_or_else(|| "?".into());
                    let text = tf("{} is in {}", &[device, &got]);
                    if got.eq_ignore_ascii_case(region) {
                        CheckState::Pass(text)
                    } else {
                        let hint = ics_core::advice::mos_advice(&got, &d.values).map(|a| i18n::t_text(&a).to_string());
                        CheckState::Fail(text, hint)
                    }
                }
            }
        }
        K::DrcClean => match drc {
            None => CheckState::Waiting(t("Run DRC on the layout.").into()),
            Some(r) if r.violations.is_empty() => CheckState::Pass(t("DRC clean.").into()),
            Some(r) => CheckState::Fail(
                tf("{} DRC violation(s)", &[&r.violations.len().to_string()]),
                Some(t("Click a violation in the Verify panel to zoom to it. Ctrl+. offers quick fixes, and Fix All repairs the cell in one step.").into()),
            ),
        },
        K::LvsClean => match lvs {
            None => CheckState::Waiting(t("Run LVS on the layout.").into()),
            Some(r) if r.matched => CheckState::Pass(t("Layout matches the schematic.").into()),
            Some(r) => CheckState::Fail(
                tf("LVS: {} difference(s)", &[&r.errors.len().to_string()]),
                Some(t("The Verify panel lists each difference: a device missing or extra in the layout, different sizes, or a net that is open (not connected) or shorted to another.").into()),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tutor requests carry quotes, which the translation scanner skips: check them here.
    #[test]
    fn tutor_requests_are_translated() {
        for k in [
            "I am working on the Learn chapter \"{}\". The check \"{}\" on my cell {} says: {}.",
            "In the Learn course I answered the quiz question \"{}\" with \"{}\", which is wrong. Help me \
             understand why, step by step, with a question I can think about rather than just the answer.",
        ] {
            assert!(crate::i18n_ko::all().any(|(e, _)| *e == k), "no Korean for {k}");
        }
    }

    #[test]
    fn every_figure_is_built_in() {
        for lang in [i18n::Lang::En, i18n::Lang::Ko] {
            for (i, chapter) in CHAPTERS.iter().enumerate() {
                for b in learn::blocks(lang, i) {
                    if let Block::Image { path, .. } = b {
                        assert!(figure(path).is_some(), "{}: figure {path} is not in FIGURES", chapter.id);
                    }
                }
            }
        }
    }

    /// Every figure draws at its poster time; with `ICS_SHOT_DIR` set, as PNGs to look at.
    #[test]
    fn figures_render() {
        let dir = std::env::var("ICS_SHOT_DIR").ok();
        for (name, bytes) in FIGURES {
            let src = std::str::from_utf8(bytes).unwrap();
            let fig = crate::figure::Figure::new(src);
            assert!(fig.period > 0.0, "{name} is animated");
            let ko = |en: &str| crate::i18n_ko::figures::FIGURES.iter().find(|(k, _)| *k == en).map(|(_, v)| *v);
            let none = |_: &str| None;
            let langs: [(&str, f64, crate::figure::Translate); 3] =
                [("poster", fig.poster, &none), ("start", 0.0, &none), ("ko", fig.poster, &ko)];
            for (tag, t, tr) in langs {
                let svg = fig.snapshot(Some(t), tr);
                let img = crate::figure::render(&svg, 1.0).unwrap_or_else(|| panic!("{name} does not render"));
                assert!(img.pixels.iter().any(|p| p.a() > 0), "{name} is empty");
                if let Some(d) = &dir {
                    std::fs::create_dir_all(d).unwrap();
                    let opt = resvg::usvg::Options { fontdb: crate::figure::fontdb(), ..Default::default() };
                    let tree = resvg::usvg::Tree::from_str(&svg, &opt).unwrap();
                    let s = tree.size().to_int_size();
                    let mut pm = resvg::tiny_skia::Pixmap::new(s.width(), s.height()).unwrap();
                    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
                    pm.save_png(format!("{d}/fig_{}_{tag}.png", name.trim_end_matches(".svg"))).unwrap();
                }
            }
        }
    }

    /// Every text in a figure a reader sees has a Korean entry, or is data kept as is.
    #[test]
    fn figure_text_is_translated() {
        use crate::i18n_ko::figures::{FIGURES as KO, KEEP};
        let mut missing = vec![];
        for (name, bytes) in FIGURES {
            for text in crate::figure::texts(std::str::from_utf8(bytes).unwrap()) {
                let words = text
                    .split(|c: char| !c.is_ascii_alphabetic())
                    .any(|w| w.len() >= 2 && w.chars().any(|c| c.is_ascii_lowercase()));
                if words && !KEEP.contains(&text.as_str()) && !KO.iter().any(|(k, _)| *k == text) {
                    missing.push(format!("{name}: {text:?}"));
                }
            }
        }
        assert!(missing.is_empty(), "figure text without Korean:\n{}", missing.join("\n"));
    }

    #[test]
    fn links_out_of_the_course_go_to_the_guide() {
        assert!(matches!(resolve_link("mosfet.md#the-three-regions"), Link::Chapter(2, a) if a == "the-three-regions"));
        assert!(
            matches!(resolve_link("../tools.md#waveform-calculator"), Link::Url(u) if u.ends_with("tools.html#waveform-calculator"))
        );
    }
}
