//! Silicon Correlation window: measured curves (CSV) against the cell's simulation, and a
//! Levenberg-Marquardt fit of model, instance or design parameters to them
//! (`ics_sim::silicon`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, LineStyle, Plot, PlotPoints, Points};
use ics_core::silicon::{FitParam, Measured, SiliconSetup, Trace};
use ics_core::units::format_eng;
use ics_core::{CellRef, Project};
use ics_sim::SimOptions;
use ics_sim::silicon::{Correlation, Evaluation};

/// The outcome of a compare or fit run.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub start_values: Vec<f64>,
    pub start: Evaluation,
    pub best: Option<Evaluation>,
    pub iterations: usize,
    /// Fitted model cards.
    pub cards: String,
    /// The parameters of the run, in the order of the values.
    pub params: Vec<FitParam>,
}

pub enum SiliconAction {
    /// The draft setup changed; store it with the cell.
    Changed(SiliconSetup),
    LoadData,
    Compare,
    Fit,
    /// Write the fitted instance parameters into the cell's schematic.
    Apply(CellRef, Vec<(String, f64)>),
}

#[derive(Default)]
pub struct SiliconView {
    pub open: bool,
    pub cell: Option<CellRef>,
    pub setup: SiliconSetup,
    pub data: Option<Measured>,
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    rx: Option<Receiver<Result<Outcome, String>>>,
    cancel: Arc<AtomicBool>,
    iteration: Arc<AtomicUsize>,
}

impl SiliconView {
    pub fn show(&mut self, cell: &CellRef, setup: &SiliconSetup, root: Option<&Path>) {
        if self.cell.as_ref() != Some(cell) {
            self.outcome = None;
            self.error = None;
            self.cell = Some(cell.clone());
            self.setup = setup.clone();
            self.data = None;
            if !self.setup.data.trim().is_empty()
                && let Err(e) = self.load_data(root)
            {
                self.error = Some(e);
            }
        }
        self.open = true;
    }

    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Read the setup's data file (relative to `root`).
    pub fn load_data(&mut self, root: Option<&Path>) -> Result<(), String> {
        let rel = Path::new(self.setup.data.trim());
        let path = match root {
            Some(r) if rel.is_relative() => r.join(rel),
            _ => rel.to_path_buf(),
        };
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m = Measured::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if self.setup.x.trim().is_empty() {
            self.setup.x = m.columns[0].clone();
        }
        if self.setup.analysis.trim().is_empty() {
            self.setup.analysis = "dc".into();
        }
        self.data = Some(m);
        self.outcome = None;
        self.error = None;
        Ok(())
    }

    /// Compare (or fit) on a copy of the project in a background thread.
    pub fn start(&mut self, ctx: &egui::Context, project: Project, fit: bool, opts: SimOptions) -> Result<(), String> {
        if self.is_running() {
            return Err("a correlation is already running".into());
        }
        let cell = self.cell.clone().ok_or("no cell")?;
        let data = self.data.clone().ok_or("load the measured data first")?;
        let setup = self.setup.clone();
        // Check the setup before starting.
        Correlation::with_data(&project, &cell, setup.clone(), data.clone(), &opts)?;
        let (tx, rx) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.iteration = Arc::new(AtomicUsize::new(0));
        let (cancel, iteration, ctx) = (self.cancel.clone(), self.iteration.clone(), ctx.clone());
        std::thread::spawn(move || {
            let run = || -> Result<Outcome, String> {
                let cor = Correlation::with_data(&project, &cell, setup, data, &opts)?;
                let start_values = cor.start_values();
                let params = cor.params().to_vec();
                if !fit {
                    let start = cor.evaluate(&start_values);
                    return Ok(Outcome { start_values, start, best: None, iterations: 0, cards: String::new(), params });
                }
                let rep = cor.fit(60, &mut |k, _| {
                    iteration.store(k, Ordering::Relaxed);
                    ctx.request_repaint();
                    !cancel.load(Ordering::Relaxed)
                })?;
                let cards = cor.fitted_cards(&rep.best.values)?;
                Ok(Outcome { start_values, start: rep.start, best: Some(rep.best), iterations: rep.iterations, cards, params })
            };
            let _ = tx.send(run());
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.outcome = None;
        self.error = None;
        Ok(())
    }

    /// Pick up a finished run; returns (ok, console line).
    pub fn poll(&mut self) -> Option<(bool, String)> {
        let r = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        let cell = self.cell.as_ref().map_or(String::new(), |c| c.to_string());
        Some(match r {
            Ok(o) => {
                let pct = |v: f64| format!("{:.2}%", v * 100.0);
                let msg = match &o.best {
                    Some(b) => format!(
                        "Silicon correlation {cell}: RMS error {} -> {} in {} iteration(s)",
                        pct(o.start.error()),
                        pct(b.error()),
                        o.iterations
                    ),
                    None => format!("Silicon correlation {cell}: RMS error {}", pct(o.start.error())),
                };
                let ok = o.start.traces.iter().all(|t| t.is_ok());
                self.outcome = Some(o);
                (ok, msg)
            }
            Err(e) => {
                self.error = Some(e.clone());
                (false, format!("Silicon correlation {cell}: {e}"))
            }
        })
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<SiliconAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let mut changed = false;
        let mut open = self.open;
        let title = match &self.cell {
            Some(c) => format!("Silicon Correlation: {c}"),
            None => "Silicon Correlation".into(),
        };
        if self.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        crate::float::tool_window(ctx, "silicon_correlation", &title, [780.0, 820.0], &mut open, |ui| {
            let pal = crate::theme::palette(ui.ctx());
            let running = self.is_running();
            let columns: Vec<String> = self.data.as_ref().map(|d| d.columns.clone()).unwrap_or_default();
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    let l = ui.label("Measured data");
                    let shown = if self.setup.data.is_empty() { "none".to_string() } else { self.setup.data.clone() };
                    ui.label(RichText::new(shown).monospace()).labelled_by(l.id);
                    if ui.button("Load CSV…").clicked() {
                        action = Some(SiliconAction::LoadData);
                    }
                    if let Some(d) = &self.data {
                        ui.label(
                            RichText::new(format!("{} rows, {} columns", d.rows.len(), d.columns.len())).color(pal.text_weak),
                        );
                    }
                });
                ui.horizontal(|ui| {
                    let l = ui.label("X column");
                    egui::ComboBox::from_id_salt("si_x")
                        .selected_text(self.setup.x.as_str())
                        .show_ui(ui, |ui| {
                            for c in &columns {
                                changed |= ui.selectable_value(&mut self.setup.x, c.clone(), c).changed();
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    let l = ui.label("Analysis");
                    egui::ComboBox::from_id_salt("si_an")
                        .selected_text(self.setup.analysis.as_str())
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            for a in ["dc", "ac", "tran"] {
                                changed |= ui.selectable_value(&mut self.setup.analysis, a.to_string(), a).changed();
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    ui.label(RichText::new("its sweep is the x axis").small().color(pal.text_weak));
                });
                ui.add_space(4.0);
                crate::theme::section(ui, "Traces");
                let mut remove = None;
                egui::Grid::new("si_traces").num_columns(6).spacing([6.0, 4.0]).show(ui, |ui| {
                    for h in ["#", "COLUMN", "SIMULATED EXPRESSION", "BIAS", "LOG", ""] {
                        ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                    }
                    ui.end_row();
                    for (k, t) in self.setup.traces.iter_mut().enumerate() {
                        let l = ui.label((k + 1).to_string());
                        egui::ComboBox::from_id_salt(("si_col", k))
                            .selected_text(t.column.as_str())
                            .width(110.0)
                            .show_ui(ui, |ui| {
                                for c in &columns {
                                    changed |= ui.selectable_value(&mut t.column, c.clone(), c).changed();
                                }
                            })
                            .response
                            .labelled_by(l.id)
                            .on_hover_text("Measured column");
                        let r = ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut t.expr)).labelled_by(l.id);
                        changed |= r.on_hover_text("Waveform calculator expression, e.g. -i(vd)").changed();
                        let r = ui.add_sized([170.0, 20.0], egui::TextEdit::singleline(&mut t.bias)).labelled_by(l.id);
                        changed |=
                            r.on_hover_text("Bias of this curve: VG.dc=0.9 (instance parameters or design variables)").changed();
                        changed |= ui.checkbox(&mut t.log, "").on_hover_text("Compare on a log scale").changed();
                        if ui.small_button("Remove").clicked() {
                            remove = Some(k);
                        }
                        ui.end_row();
                    }
                });
                if let Some(k) = remove {
                    self.setup.traces.remove(k);
                    changed = true;
                }
                if ui.button("+ Trace").clicked() {
                    let used: Vec<&String> = self.setup.traces.iter().map(|t| &t.column).collect();
                    let next = columns.iter().find(|c| **c != self.setup.x && !used.contains(c)).cloned().unwrap_or_default();
                    let like = self.setup.traces.last().cloned().unwrap_or_default();
                    self.setup.traces.push(Trace { column: next, expr: like.expr, bias: like.bias, log: like.log });
                    changed = true;
                }
                ui.add_space(4.0);
                crate::theme::section(ui, "Parameters");
                let mut remove = None;
                let fitted = self.outcome.as_ref().map(|o| (o.start_values.clone(), o.best.as_ref().map(|b| b.values.clone())));
                egui::Grid::new("si_params").num_columns(7).spacing([6.0, 4.0]).show(ui, |ui| {
                    for h in ["#", "TARGET", "MIN", "MAX", "LOG", "START → FITTED", ""] {
                        ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                    }
                    ui.end_row();
                    for (k, p) in self.setup.params.iter_mut().enumerate() {
                        let l = ui.label((k + 1).to_string());
                        let r = ui.add_sized([170.0, 20.0], egui::TextEdit::singleline(&mut p.target)).labelled_by(l.id);
                        changed |= r.on_hover_text("model:nmos.vto, M1.w or a design variable").changed();
                        changed |= ui.add_sized([64.0, 20.0], egui::TextEdit::singleline(&mut p.min)).labelled_by(l.id).changed();
                        changed |= ui.add_sized([64.0, 20.0], egui::TextEdit::singleline(&mut p.max)).labelled_by(l.id).changed();
                        changed |= ui.checkbox(&mut p.log, "").on_hover_text("Search on a log scale").changed();
                        match &fitted {
                            Some((s, f)) if k < s.len() => {
                                let t = match f {
                                    Some(f) => format!("{} → {}", format_eng(s[k], ""), format_eng(f[k], "")),
                                    None => format_eng(s[k], ""),
                                };
                                ui.label(RichText::new(t).monospace());
                            }
                            _ => {
                                ui.label("");
                            }
                        }
                        if ui.small_button("Remove").clicked() {
                            remove = Some(k);
                        }
                        ui.end_row();
                    }
                });
                if let Some(k) = remove {
                    self.setup.params.remove(k);
                    changed = true;
                }
                if ui.button("+ Parameter").clicked() {
                    self.setup.params.push(FitParam {
                        target: "model:nmos.vto".into(),
                        min: "0.2".into(),
                        max: "0.8".into(),
                        log: false,
                    });
                    changed = true;
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = self.data.is_some() && !self.setup.traces.is_empty();
                if ui.add_enabled(!running && ready, egui::Button::new("Compare")).on_hover_text("Simulate and compare").clicked()
                {
                    action = Some(SiliconAction::Compare);
                }
                let can_fit = ready && !self.setup.params.is_empty();
                if ui.add_enabled(!running && can_fit, egui::Button::new("Fit")).on_hover_text("Fit the parameters").clicked() {
                    action = Some(SiliconAction::Fit);
                }
                if running {
                    if ui.button("Stop").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                    ui.spinner();
                    ui.label(format!("iteration {}", self.iteration.load(Ordering::Relaxed)));
                }
                if let Some(o) = &self.outcome
                    && let Some(b) = &o.best
                {
                    let inst: Vec<(String, f64)> = o
                        .params
                        .iter()
                        .zip(&b.values)
                        .filter(|(p, _)| p.model_param().is_none() && p.target.contains('.'))
                        .map(|(p, v)| (p.target.clone(), *v))
                        .collect();
                    if !inst.is_empty()
                        && let Some(c) = &self.cell
                        && ui.button("Apply to schematic").on_hover_text("Write the fitted instance parameters").clicked()
                    {
                        action = Some(SiliconAction::Apply(c.clone(), inst));
                    }
                    if !o.cards.is_empty() && ui.button("Copy model cards").clicked() {
                        ui.ctx().copy_text(o.cards.clone());
                    }
                }
            });
            if let Some(e) = &self.error {
                ui.colored_label(pal.danger, e);
            }
            let Some(o) = &self.outcome else { return };
            ui.separator();
            let pct = |v: f64| format!("{:.2}%", v * 100.0);
            egui::Grid::new("si_errors").striped(true).num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["TRACE", "RMS ERROR", "FITTED"] {
                    ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                }
                ui.end_row();
                for (k, t) in o.start.traces.iter().enumerate() {
                    let name = self.setup.traces.get(k).map_or(String::new(), |t| t.column.clone());
                    ui.label(name);
                    match t {
                        Ok(t) => ui.label(RichText::new(pct(t.error)).monospace()),
                        Err(e) => ui.colored_label(pal.danger, e),
                    };
                    match o.best.as_ref().and_then(|b| b.traces.get(k)) {
                        Some(Ok(t)) => ui.label(RichText::new(pct(t.error)).monospace().strong().color(pal.ok)),
                        Some(Err(e)) => ui.colored_label(pal.danger, e),
                        None => ui.label("—"),
                    };
                    ui.end_row();
                }
            });
            let colors = [pal.accent, pal.ok, pal.warning, pal.danger, pal.schematic, pal.layout];
            let (start, best) = (&o.start, o.best.as_ref());
            Plot::new("si_plot")
                .height(ui.available_height().max(260.0))
                .legend(Legend::default().position(egui_plot::Corner::LeftTop))
                .x_axis_formatter(|m, _| format_eng(m.value, ""))
                .y_axis_formatter(|m, _| format_eng(m.value, ""))
                .allow_scroll(false)
                .show(ui, |pu| {
                    for (k, t) in start.traces.iter().enumerate() {
                        let Ok(t) = t else { continue };
                        let c: Color32 = colors[k % colors.len()];
                        let meas: Vec<[f64; 2]> = t.x.iter().zip(&t.measured).map(|(x, y)| [*x, *y]).collect();
                        pu.points(Points::new(format!("{} measured", t.column), meas).color(c).radius(2.5));
                        let line = |e: &Evaluation| -> Option<Vec<[f64; 2]>> {
                            let tf = e.traces.get(k)?.as_ref().ok()?;
                            Some(tf.x.iter().zip(&tf.simulated).filter(|(_, y)| y.is_finite()).map(|(x, y)| [*x, *y]).collect())
                        };
                        if let Some(pts) = line(start) {
                            let style = if best.is_some() { LineStyle::dashed_dense() } else { LineStyle::Solid };
                            pu.line(
                                Line::new(format!("{} model", t.column), PlotPoints::from(pts))
                                    .color(c.gamma_multiply(0.6))
                                    .style(style),
                            );
                        }
                        if let Some(pts) = best.and_then(line) {
                            pu.line(Line::new(format!("{} fitted", t.column), PlotPoints::from(pts)).color(c).width(1.8));
                        }
                    }
                });
        });
        self.open = open;
        if changed && action.is_none() {
            action = Some(SiliconAction::Changed(self.setup.clone()));
        }
        action
    }
}
