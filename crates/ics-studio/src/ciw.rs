//! Command Interpreter Window: an interactive SKILL prompt whose variables and procedures
//! persist between commands (and across `.il` files loaded through File > Import).

use eframe::egui::{self, Key, RichText};
use ics_core::skill::{self, RunResult, Session, SkillError};

use crate::theme;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CiwKind {
    Input,
    Output,
    Value,
    Error,
}

pub struct Ciw {
    pub session: Session,
    pub lines: Vec<(CiwKind, String)>,
    input: String,
    /// Lines of a command still missing a closing parenthesis, string or comment.
    pending: String,
    history: Vec<String>,
    /// Position while browsing `history` with the arrow keys.
    hist_pos: Option<usize>,
    focus: bool,
}

impl Default for Ciw {
    fn default() -> Self {
        Ciw {
            session: Session::new(),
            lines: vec![],
            input: String::new(),
            pending: String::new(),
            history: vec![],
            hist_pos: None,
            focus: false,
        }
    }
}

const MAX_LINES: usize = 5000;

impl Ciw {
    pub fn push(&mut self, kind: CiwKind, text: impl Into<String>) {
        self.lines.push((kind, text.into()));
        if self.lines.len() > MAX_LINES {
            self.lines.drain(0..MAX_LINES / 4);
        }
    }

    /// Forget all SKILL state (a different project was opened); the transcript stays.
    pub fn reset(&mut self) {
        self.session = Session::new();
        self.pending.clear();
    }

    pub fn request_focus(&mut self) {
        self.focus = true;
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Show what a command printed and returned.
    pub fn show(&mut self, res: &Result<RunResult, SkillError>) {
        let output = match res {
            Ok(r) => &r.output,
            Err(e) => &e.output,
        };
        for l in output.lines() {
            self.push(CiwKind::Output, l);
        }
        match res {
            Ok(r) => self.push(CiwKind::Value, r.value.clone()),
            Err(e) => {
                let msg = match &e.file {
                    Some(f) => format!("*Error* {f}:{}: {}", e.line, e.message),
                    None => format!("*Error* {}", e.message),
                };
                self.push(CiwKind::Error, msg);
            }
        }
    }

    /// Draw the transcript and the prompt. Returns a complete command to evaluate.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let pal = theme::palette(ui.ctx());
        let input_h = ui.spacing().interact_size.y + 6.0;
        egui::ScrollArea::vertical()
            .id_salt("ciw_transcript")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .max_height((ui.available_height() - input_h).max(20.0))
            .show(ui, |ui| {
                for (kind, text) in &self.lines {
                    let (prefix, color) = match kind {
                        CiwKind::Input => ("> ", pal.text_strong),
                        CiwKind::Output => ("", pal.text),
                        CiwKind::Value => ("", pal.text_weak),
                        CiwKind::Error => ("", pal.danger),
                    };
                    ui.add(
                        egui::Label::new(RichText::new(format!("{prefix}{text}")).monospace().size(12.0).color(color))
                            .wrap()
                            .selectable(true),
                    );
                }
            });
        let mut submit = None;
        ui.horizontal(|ui| {
            let prompt = ui.label(RichText::new(if self.pending.is_empty() { ">" } else { "..." }).monospace().color(pal.accent));
            let id = egui::Id::unique("ciw_input");
            if ui.memory(|m| m.has_focus(id)) {
                let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                if up || down {
                    self.browse(up);
                }
            }
            let edit = egui::TextEdit::singleline(&mut self.input)
                .id(id)
                .font(egui::TextStyle::Monospace)
                .hint_text("SKILL, e.g. dbCreateRect(geGetEditCellView() \"metal1\" list(0:0 1:1))")
                .desired_width(f32::INFINITY);
            let resp = ui.add(edit).labelled_by(prompt.id);
            if self.focus {
                resp.request_focus();
                self.focus = false;
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = self.enter();
                resp.request_focus();
            }
        });
        submit
    }

    fn browse(&mut self, up: bool) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        self.hist_pos = match (self.hist_pos, up) {
            (None, true) => Some(last),
            (None, false) => None,
            (Some(p), true) => Some(p.saturating_sub(1)),
            (Some(p), false) if p < last => Some(p + 1),
            (Some(_), false) => None,
        };
        self.input = self.hist_pos.map(|p| self.history[p].clone()).unwrap_or_default();
    }

    /// Take the typed line; a command is returned once its parentheses, strings and comments close.
    fn enter(&mut self) -> Option<String> {
        let line = std::mem::take(&mut self.input);
        self.hist_pos = None;
        if line.trim().is_empty() && self.pending.is_empty() {
            return None;
        }
        if !line.trim().is_empty() && self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        self.push(CiwKind::Input, line.clone());
        if !self.pending.is_empty() {
            self.pending.push('\n');
        }
        self.pending.push_str(&line);
        if skill::is_complete(&self.pending) { Some(std::mem::take(&mut self.pending)) } else { None }
    }
}
