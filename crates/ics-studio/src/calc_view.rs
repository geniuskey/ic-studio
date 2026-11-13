//! Waveform Calculator (Virtuoso ViVA Calculator): evaluate expressions on simulation results,
//! read scalar measurements and plot derived waveforms.

use eframe::egui::{self, RichText};
use ics_core::CellRef;
use ics_core::units::format_eng;
use ics_sim::calc::{self, CalcValue};

use crate::sim_view::SimState;
use crate::theme::{self, Palette};

#[derive(Default)]
pub struct Calculator {
    pub open: bool,
    pub expr: String,
    /// Dataset the expression runs on (`tran`, `ac`, `dc`, `op`); empty = the results tab.
    pub analysis: String,
    /// Evaluated expressions, newest last: (analysis, expression, result or error, ok).
    pub history: Vec<(String, String, String, bool)>,
    filter: String,
    focus: bool,
}

impl Calculator {
    pub fn show(&mut self) {
        self.open = true;
        self.focus = true;
    }

    /// Evaluate `expr` on the `analysis` dataset of `sim`, plotting waveform results when `plot`
    /// is set. Records the outcome in the history and returns it.
    pub fn evaluate(&mut self, sim: &mut SimState, analysis: &str, expr: &str, plot: bool) -> Result<String, String> {
        let r = sim.eval_calc(analysis, expr, plot).map(|v| match v {
            CalcValue::Scalar(x) => format_eng(x, ""),
            CalcValue::Wave(w) if plot => format!("plotted ({} points)", w.re.len()),
            CalcValue::Wave(w) => format!("waveform ({} points{})", w.re.len(), if w.im.is_some() { ", complex" } else { "" }),
        });
        let (text, ok) = match &r {
            Ok(s) => (s.clone(), true),
            Err(e) => (e.clone(), false),
        };
        self.history.push((analysis.to_string(), expr.trim().to_string(), text, ok));
        if self.history.len() > 200 {
            self.history.remove(0);
        }
        r
    }

    pub fn window(&mut self, ctx: &egui::Context, cell: Option<&CellRef>, sim: Option<&mut SimState>) {
        if !self.open {
            return;
        }
        let mut open = true;
        crate::float::tool_window(ctx, "calculator", "Waveform Calculator", [900.0, 600.0], &mut open, |ui| {
            self.ui(ui, cell, sim);
        });
        self.open &= open;
    }

    fn ui(&mut self, ui: &mut egui::Ui, cell: Option<&CellRef>, sim: Option<&mut SimState>) {
        let pal = theme::palette(ui.ctx());
        let analyses: Vec<String> = sim
            .as_ref()
            .and_then(|s| s.results.as_ref())
            .map(|r| r.datasets.iter().map(|d| d.analysis.clone()).collect())
            .unwrap_or_default();
        if !analyses.contains(&self.analysis) {
            let tab = sim.as_ref().map(|s| s.tab.clone()).unwrap_or_default();
            self.analysis = if analyses.contains(&tab) { tab } else { analyses.first().cloned().unwrap_or_default() };
        }
        egui::Panel::left("calc_funcs")
            .resizable(true)
            .default_size(300.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| self.palette(ui, sim.as_deref(), &pal));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Waveform Calculator").strong().color(pal.text_strong));
                ui.label(RichText::new(cell.map_or("· no cell".into(), |c| format!("· {c}"))).color(pal.text_weak));
            });
            ui.separator();
            if analyses.is_empty() {
                ui.label(RichText::new("No results for the active cell: run a simulation (F5) first.").color(pal.text_weak));
            }
            ui.horizontal(|ui| {
                let l = ui.label("Dataset");
                egui::ComboBox::from_id_salt("calc_ds")
                    .selected_text(self.analysis.as_str())
                    .show_ui(ui, |ui| {
                        for a in &analyses {
                            ui.selectable_value(&mut self.analysis, a.clone(), a.as_str());
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            let l = ui.label("Expression");
            let te = egui::TextEdit::singleline(&mut self.expr)
                .code_editor()
                .desired_width(f32::INFINITY)
                .hint_text("bandwidth(v(out)/v(in))   risetime(v(out))   deriv(v(out))");
            let resp = ui.add(te).labelled_by(l.id);
            if std::mem::take(&mut self.focus) {
                resp.request_focus();
            }
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let mut run = None;
            ui.horizontal(|ui| {
                let ready = sim.is_some() && !analyses.is_empty() && !self.expr.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new("Evaluate")).on_hover_text("Enter").clicked() || (enter && ready) {
                    run = Some(false);
                }
                if ui
                    .add_enabled(ready, egui::Button::new("Plot"))
                    .on_hover_text("Add the waveform to the results plot")
                    .clicked()
                {
                    run = Some(true);
                }
                if ui.button("Clear history").clicked() {
                    self.history.clear();
                }
            });
            if let (Some(plot), Some(s)) = (run, sim) {
                let (a, e) = (self.analysis.clone(), self.expr.clone());
                let _ = self.evaluate(s, &a, &e, plot);
            }
            ui.add_space(6.0);
            theme::section(ui, "Results");
            let mut reuse = None;
            egui::ScrollArea::vertical().id_salt("calc_hist").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                egui::Grid::new("calc_hist_grid").striped(true).num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
                    for (a, e, r, ok) in &self.history {
                        ui.label(RichText::new(a).small().color(pal.text_weak));
                        if ui
                            .add(egui::Label::new(RichText::new(e).monospace()).sense(egui::Sense::click()))
                            .on_hover_text("Edit again")
                            .clicked()
                        {
                            reuse = Some((a.clone(), e.clone()));
                        }
                        ui.label(RichText::new(r).monospace().color(if *ok { pal.text_strong } else { pal.danger }));
                        ui.end_row();
                    }
                });
            });
            if let Some((a, e)) = reuse {
                self.analysis = a;
                self.expr = e;
            }
        });
    }

    fn palette(&mut self, ui: &mut egui::Ui, sim: Option<&SimState>, pal: &Palette) {
        let fl = ui.label(RichText::new("Filter").small().color(pal.text_weak));
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("rise, db, v(out)…").desired_width(f32::INFINITY))
            .labelled_by(fl.id);
        let f = self.filter.to_ascii_lowercase();
        egui::ScrollArea::vertical().id_salt("calc_pal").show(ui, |ui| {
            theme::section(ui, "Signals");
            let ds = sim.and_then(|s| s.results.as_ref()).and_then(|r| r.dataset(&self.analysis));
            for v in ds.into_iter().flat_map(|d| d.vectors.iter()) {
                if (!f.is_empty() && !v.name.to_ascii_lowercase().contains(&f)) || v.name.contains(' ') {
                    continue;
                }
                if ui.add(egui::Button::new(RichText::new(&v.name).monospace()).frame(false)).on_hover_text("Insert").clicked() {
                    self.expr.push_str(&v.name);
                    self.focus = true;
                }
            }
            theme::section(ui, "Functions");
            for (sig, help) in calc::functions() {
                if !f.is_empty() && !sig.to_ascii_lowercase().contains(&f) && !help.to_ascii_lowercase().contains(&f) {
                    continue;
                }
                let name = sig.split('(').next().unwrap_or(sig);
                let b = ui.add(egui::Button::new(RichText::new(*sig).monospace()).frame(false)).on_hover_text(*help);
                if b.clicked() {
                    self.expr =
                        if self.expr.trim().is_empty() { format!("{name}(") } else { format!("{name}({})", self.expr.trim()) };
                    self.focus = true;
                }
            }
        });
    }
}
