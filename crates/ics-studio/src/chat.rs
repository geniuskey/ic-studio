//! Chat rendering for the AI windows: light Markdown (paragraphs, lists, **bold**, `code`,
//! headings and fenced code blocks with a copy button), copy buttons, and a scroll area that
//! follows a text selection dragged past its top or bottom edge.

use eframe::egui;
use egui::text::{LayoutJob, TextFormat};

use crate::theme::Palette;

/// A piece of a message: running text, or a fenced code block.
#[derive(Debug, PartialEq)]
pub enum Block<'a> {
    Text(String),
    Code { lang: &'a str, code: String },
}

/// Split Markdown into text and fenced code blocks. An unclosed fence (a message still
/// streaming) runs to the end.
pub fn blocks(md: &str) -> Vec<Block<'_>> {
    let mut out = vec![];
    let mut text: Vec<&str> = vec![];
    let mut code: Option<(&str, Vec<&str>, usize)> = None;
    for line in md.lines() {
        let t = line.trim_start();
        match &mut code {
            Some((lang, lines, indent)) => {
                if t.starts_with("```") && t.trim_end().trim_start_matches('`').is_empty() {
                    out.push(Block::Code { lang, code: lines.join("\n") });
                    code = None;
                } else {
                    let strip = line.len() - line.trim_start().len();
                    lines.push(&line[strip.min(*indent)..]);
                }
            }
            None => {
                if let Some(rest) = t.strip_prefix("```") {
                    flush_text(&mut out, &mut text);
                    code = Some((rest.trim(), vec![], line.len() - t.len()));
                } else {
                    text.push(line);
                }
            }
        }
    }
    flush_text(&mut out, &mut text);
    if let Some((lang, lines, _)) = code {
        out.push(Block::Code { lang, code: lines.join("\n") });
    }
    out
}

fn flush_text(out: &mut Vec<Block>, text: &mut Vec<&str>) {
    let t = text.join("\n");
    text.clear();
    let t = t.trim_matches('\n');
    if !t.trim().is_empty() {
        out.push(Block::Text(t.to_string()));
    }
}

/// Text with inline Markdown as one layout job: `**bold**`, `` `code` ``, and `#` headings.
pub fn inline_job(ui: &egui::Ui, text: &str, pal: &Palette, color: egui::Color32) -> LayoutJob {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let mut job = LayoutJob::default();
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            job.append("\n", 0.0, TextFormat::simple(body.clone(), color));
        }
        let t = line.trim_start();
        let hashes = t.chars().take_while(|c| *c == '#').count();
        let (line, heading) = if (1..=4).contains(&hashes) && t[hashes..].starts_with(' ') {
            (t[hashes..].trim_start(), true)
        } else {
            (line, false)
        };
        let (mut bold, mut code) = (heading, false);
        let mut rest = line;
        while !rest.is_empty() {
            let next = if code { rest.find('`').map(|i| (i, 1)) } else { find_marker(rest) };
            let (seg, marker) = match next {
                Some((i, len)) => (&rest[..i], Some(&rest[i..i + len])),
                None => (rest, None),
            };
            if !seg.is_empty() {
                let fmt = if code {
                    TextFormat {
                        font_id: egui::FontId::new(mono.size * 0.95, mono.family.clone()),
                        color: pal.text_strong,
                        background: pal.bg_widget,
                        ..Default::default()
                    }
                } else {
                    TextFormat::simple(body.clone(), if bold { pal.text_strong } else { color })
                };
                job.append(seg, 0.0, fmt);
            }
            match marker {
                Some("`") => code = !code,
                Some(_) => bold = !bold,
                None => break,
            }
            rest = &rest[seg.len() + marker.map_or(0, str::len)..];
        }
    }
    job
}

/// The next `**` or `` ` `` in `s`: its byte offset and length.
fn find_marker(s: &str) -> Option<(usize, usize)> {
    let b = s.find("**").map(|i| (i, 2));
    let c = s.find('`').map(|i| (i, 1));
    match (b, c) {
        (Some(b), Some(c)) => Some(if c.0 < b.0 { c } else { b }),
        (b, c) => b.or(c),
    }
}

/// A button that copies `text` and says "Copied" for a moment afterwards.
pub fn copy_button(ui: &mut egui::Ui, id: egui::Id, label: &str, text: &str) -> egui::Response {
    copy_button_with(ui, id, label, || text.to_string())
}

/// [`copy_button`] that only builds the text when clicked.
pub fn copy_button_with(ui: &mut egui::Ui, id: egui::Id, label: &str, text: impl FnOnce() -> String) -> egui::Response {
    let now = ui.input(|i| i.time);
    let at: Option<f64> = ui.data(|d| d.get_temp(id));
    let copied = at.is_some_and(|t| now - t < 1.5);
    let r = ui.add(egui::Button::new(egui::RichText::new(if copied { "Copied" } else { label }).small()).small());
    if r.clicked() {
        ui.ctx().copy_text(text());
        ui.data_mut(|d| d.insert_temp(id, now));
    }
    if copied {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
    }
    r.on_hover_text("Copy to the clipboard")
}

/// Show a Markdown message. `id` keeps the code blocks' widgets apart.
pub fn markdown(ui: &mut egui::Ui, id: egui::Id, md: &str, pal: &Palette) {
    for (k, b) in blocks(md).into_iter().enumerate() {
        match b {
            Block::Text(t) => {
                let job = inline_job(ui, &t, pal, pal.text);
                ui.add(egui::Label::new(job).wrap());
            }
            Block::Code { lang, code } => code_block(ui, id.with(k), lang, &code, pal),
        }
    }
}

/// A fenced code block: monospace, scrolls sideways, with the language and a copy button.
pub fn code_block(ui: &mut egui::Ui, id: egui::Id, lang: &str, code: &str, pal: &Palette) {
    egui::Frame::new()
        .fill(pal.bg_canvas)
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(if lang.is_empty() { "code" } else { lang }).small().color(pal.text_weak));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    copy_button(ui, id.with("copy"), "Copy", code);
                });
            });
            egui::ScrollArea::horizontal().id_salt(id.with("scroll")).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(code).monospace().color(pal.text_strong)).extend());
            });
        });
}

/// Call at the end of a scroll area's contents: while a drag that started inside the view (a text
/// selection) is held above or below it, scroll that way, faster the farther the pointer is.
pub fn drag_autoscroll(ui: &egui::Ui) {
    let view = ui.clip_rect();
    let bar = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin + 4.0;
    let (down, origin, pos, dt) =
        ui.input(|i| (i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.latest_pos(), i.stable_dt));
    let (Some(o), Some(p)) = (origin, pos) else { return };
    if !down || !view.contains(o) || o.x > view.right() - bar {
        return;
    }
    let past = if p.y < view.top() {
        view.top() - p.y
    } else if p.y > view.bottom() {
        view.bottom() - p.y
    } else {
        return;
    };
    let speed = past.signum() * (past.abs() * 12.0).clamp(120.0, 2400.0);
    ui.scroll_with_delta(egui::vec2(0.0, speed * dt.min(0.05)));
    ui.ctx().request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_fenced_code() {
        let b = blocks("Here:\n\n```spice\nR1 a b 1k\n  C1 b 0 1n\n```\nDone.");
        assert_eq!(b.len(), 3);
        assert_eq!(b[0], Block::Text("Here:".into()));
        assert_eq!(b[1], Block::Code { lang: "spice", code: "R1 a b 1k\n  C1 b 0 1n".into() });
        assert_eq!(b[2], Block::Text("Done.".into()));
    }

    #[test]
    fn unclosed_fence_runs_to_the_end() {
        let b = blocks("x\n```\nline 1\nline");
        assert_eq!(b[1], Block::Code { lang: "", code: "line 1\nline".into() });
    }

    #[test]
    fn indented_fence_strips_its_indent() {
        let b = blocks("- item\n  ```\n  a\n    b\n  ```");
        assert_eq!(b[1], Block::Code { lang: "", code: "a\n  b".into() });
    }

    #[test]
    fn inline_markup_is_removed() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let pal = crate::theme::palette(ui.ctx());
            text = inline_job(ui, "## Gain is **60 dB** on `out`", &pal, pal.text).text;
        });
        out.textures_delta.clear();
        assert_eq!(text, "Gain is 60 dB on out");
    }
}
