//! Beginner guidance shown next to a design: the cell's design notes (what it is for, why it
//! is sized as it is, what to change for a new target) and its specs, with the way to start a
//! new design from it.

use eframe::egui::{self, RichText};
use ics_core::CellRef;
use ics_core::sweep::Spec;

use crate::app::Command;
use crate::i18n::{t, tf};
use crate::theme;

/// `min <= x <= max`, `>= min`, `<= max`, or "no limit" for a spec.
pub fn spec_limits(s: &Spec) -> String {
    match (s.min.trim(), s.max.trim()) {
        ("", "") => t("measured only").into(),
        (lo, "") => format!(">= {lo}"),
        ("", hi) => format!("<= {hi}"),
        (lo, hi) => format!("{lo} .. {hi}"),
    }
}

/// The Design section of the schematic inspector (nothing selected). Returns whether the notes
/// were edited.
pub fn design_panel(ui: &mut egui::Ui, cell: &CellRef, notes: &mut String, specs: &[Spec], q: &mut Vec<Command>) -> bool {
    let pal = theme::palette(ui.ctx());
    let mut changed = false;
    theme::section(ui, t("Specs (targets)"));
    if specs.is_empty() {
        ui.label(
            RichText::new(t(
                "No specs yet. Specs say what the design must achieve, e.g. gain >= 40 dB; they are checked on every run.",
            ))
            .small()
            .color(pal.text_weak),
        );
    } else {
        egui::Grid::new("design_specs").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
            for s in specs {
                ui.label(RichText::new(&s.name).strong()).on_hover_text(tf("{} on the {} analysis", &[&s.expr, &s.analysis]));
                ui.label(RichText::new(spec_limits(s)).monospace());
                ui.end_row();
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        if ui.small_button(t("Edit specs…")).on_hover_text(t("Corner / Sweep Manager")).clicked() {
            q.push(Command::SweepManager);
        }
        if ui.small_button(t("Check specs")).on_hover_text(t("Run every spec over its corners (Verification)")).clicked() {
            q.push(Command::Verification);
        }
    });
    ui.add_space(6.0);
    if ui
        .button(t("Derive a new design from this…"))
        .on_hover_text(t("Copy this cell with its specs and notes, set new targets and let IC Studio size and lay it out"))
        .clicked()
    {
        q.push(Command::DeriveDialog(cell.clone()));
    }
    ui.add_space(8.0);
    theme::section(ui, t("Design notes"));
    if notes.is_empty() {
        ui.label(
            RichText::new(t("What is this cell for, why is it sized this way, what would you change for another target?"))
                .small()
                .color(pal.text_weak),
        );
    }
    let l = ui.label(RichText::new(t("Notes")).small().color(pal.text_weak));
    changed |= ui.add(egui::TextEdit::multiline(notes).desired_rows(4).desired_width(f32::INFINITY)).labelled_by(l.id).changed();
    ui.add_space(8.0);
    changed
}
