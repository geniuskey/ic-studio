//! Optimizer window: size device parameters to meet the cell's specs (from the Sweep Manager),
//! optionally re-generating, routing and extracting the layout of every candidate, so the
//! specs are met after layout rather than only in the schematic.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, Points};
use ics_core::optimize::{Goal, OptSetup, OptVar};
use ics_core::units::format_eng;
use ics_core::{CellRef, Project};
use ics_sim::SimOptions;
use ics_sim::optimize::{self, OptEval, OptReport};

pub enum OptAction {
    /// The setup was edited.
    Changed(OptSetup),
    /// Write these parameter values into the cell's schematic.
    Apply(CellRef, Vec<f64>),
}

enum Msg {
    Eval(OptEval),
    Done(Result<OptReport, String>),
}

#[derive(Default)]
pub struct Optimizer {
    pub open: bool,
    pub cell: Option<CellRef>,
    pub setup: OptSetup,
    pub evals: Vec<OptEval>,
    pub error: Option<String>,
    rx: Option<Receiver<Msg>>,
    stop: Arc<AtomicBool>,
}

impl Optimizer {
    pub fn show(&mut self, cell: &CellRef, setup: &OptSetup) {
        if self.cell.as_ref() != Some(cell) {
            self.evals.clear();
            self.error = None;
        }
        self.cell = Some(cell.clone());
        self.setup = setup.clone();
        self.open = true;
    }

    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Start a run on a copy of the project (the setup must already be in it).
    pub fn start(&mut self, ctx: &egui::Context, project: &Project, opts: SimOptions) {
        let Some(cell) = self.cell.clone() else { return };
        if self.is_running() {
            return;
        }
        self.evals.clear();
        self.error = None;
        let mut p = project.clone();
        if let Some(c) = p.cell_mut(&cell) {
            c.sim.optimize = self.setup.clone();
        }
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = stop.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut progress = |e: &OptEval, _: usize| {
                let _ = tx.send(Msg::Eval(e.clone()));
                ctx.request_repaint();
                !stop.load(Ordering::Relaxed)
            };
            let res = optimize::run(&p, &cell, &opts, &mut progress);
            let _ = tx.send(Msg::Done(res));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Pick up progress; returns a console line when the run finished.
    pub fn poll(&mut self) -> Option<(bool, String)> {
        let rx = self.rx.as_ref()?;
        let mut done = None;
        for m in rx.try_iter() {
            match m {
                Msg::Eval(e) => self.evals.push(e),
                Msg::Done(r) => done = Some(r),
            }
        }
        let r = done?;
        self.rx = None;
        Some(match r {
            Ok(rep) => match rep.best() {
                Some(b) if b.passed => (true, format!("Optimizer: all specs met after {} evaluation(s)", rep.evals.len())),
                Some(_) => (false, format!("Optimizer: no point meets every spec in {} evaluation(s)", rep.evals.len())),
                None => (false, "Optimizer: nothing evaluated".into()),
            },
            Err(e) => {
                self.error = Some(e.clone());
                (false, format!("Optimizer: {e}"))
            }
        })
    }

    /// Index of the best evaluation so far.
    pub fn best(&self) -> Option<usize> {
        self.evals.iter().enumerate().min_by(|a, b| a.1.objective.total_cmp(&b.1.objective)).map(|(k, _)| k)
    }

    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>) -> (Option<OptAction>, bool) {
        if !self.open {
            return (None, false);
        }
        let (mut action, mut run) = (None, false);
        let mut open = true;
        let title = match &self.cell {
            Some(c) => format!("Optimizer: {c}"),
            None => "Optimizer".into(),
        };
        let specs = self.cell.as_ref().and_then(|c| project?.cell(c)).map(|c| c.sim.sweep.specs.clone()).unwrap_or_default();
        egui::Window::new(title).id(egui::Id::unique("optimizer")).open(&mut open).default_size([640.0, 600.0]).show(ctx, |ui| {
            let mut changed = false;
            let running = self.is_running();
            ui.add_enabled_ui(!running, |ui| {
                ui.label(egui::RichText::new("Variables").strong());
                let mut remove = None;
                egui::Grid::new("opt_vars").num_columns(6).spacing([6.0, 4.0]).show(ui, |ui| {
                    ui.label("Target");
                    ui.label("Min");
                    ui.label("Max");
                    ui.label("Step");
                    ui.label("Log");
                    ui.label("");
                    ui.end_row();
                    for (k, v) in self.setup.vars.iter_mut().enumerate() {
                        for (text, w, name) in [
                            (&mut v.target, 110.0, "target"),
                            (&mut v.min, 70.0, "min"),
                            (&mut v.max, 70.0, "max"),
                            (&mut v.step, 60.0, "step"),
                        ] {
                            let r = ui.add(egui::TextEdit::singleline(text).desired_width(w));
                            changed |= r.on_hover_text(format!("Variable {} {name}", k + 1)).changed();
                        }
                        changed |= ui.checkbox(&mut v.log, "").on_hover_text("Search on a log scale").changed();
                        if ui.small_button("x").clicked() {
                            remove = Some(k);
                        }
                        ui.end_row();
                    }
                });
                if let Some(k) = remove {
                    self.setup.vars.remove(k);
                    changed = true;
                }
                if ui.button("+ Variable").on_hover_text("INST.param, e.g. M1.w, or a design variable name").clicked() {
                    self.setup.vars.push(OptVar {
                        target: "M1.w".into(),
                        min: "1u".into(),
                        max: "20u".into(),
                        ..Default::default()
                    });
                    changed = true;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let l = ui.label("Evaluations");
                    let mut n = if self.setup.max_evals == 0 { 60 } else { self.setup.max_evals };
                    if ui.add(egui::DragValue::new(&mut n).range(4..=2000)).labelled_by(l.id).changed() {
                        self.setup.max_evals = n;
                        changed = true;
                    }
                    let l = ui.label("Goal");
                    egui::ComboBox::from_id_salt("opt_goal")
                        .selected_text(if self.setup.goal == Goal::Margin { "Best margin" } else { "Least area" })
                        .show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut self.setup.goal, Goal::Margin, "Best margin").changed();
                            changed |= ui.selectable_value(&mut self.setup.goal, Goal::MinArea, "Least area").changed();
                        })
                        .response
                        .labelled_by(l.id);
                    changed |= ui
                        .checkbox(&mut self.setup.post_layout, "Post-layout")
                        .on_hover_text("Place, route and extract every candidate and simulate with its parasitics")
                        .changed();
                });
            });
            if changed {
                action = Some(OptAction::Changed(self.setup.clone()));
            }
            ui.add_space(4.0);
            let limited = specs.iter().filter(|s| ics_core::optimize::has_limit(s)).count();
            ui.label(
                egui::RichText::new(format!(
                    "{} spec(s), {limited} with limits (edit them in the Corner / Sweep Manager)",
                    specs.len()
                ))
                .small()
                .weak(),
            );
            ui.horizontal(|ui| {
                if running {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("Evaluation {}", self.evals.len()));
                    if ui.button("Stop").clicked() {
                        self.stop.store(true, Ordering::Relaxed);
                    }
                } else if ui.add_enabled(!self.setup.vars.is_empty() && limited > 0, egui::Button::new("Run optimizer")).clicked()
                {
                    run = true;
                }
                if let (Some(k), Some(c)) = (self.best(), self.cell.clone())
                    && !running
                    && ui.button("Apply best to schematic").clicked()
                {
                    action = Some(OptAction::Apply(c, self.evals[k].values.clone()));
                }
            });
            if let Some(e) = &self.error {
                ui.colored_label(crate::theme::palette(ui.ctx()).danger, e);
            }
            if self.evals.is_empty() {
                return;
            }
            let pal = crate::theme::palette(ui.ctx());
            let obj: Vec<[f64; 2]> =
                self.evals.iter().enumerate().map(|(k, e)| [k as f64 + 1.0, e.objective.clamp(-1.0, 10.0)]).collect();
            let mut best = vec![];
            let mut b = f64::INFINITY;
            for (k, e) in self.evals.iter().enumerate() {
                b = b.min(e.objective);
                best.push([k as f64 + 1.0, b.clamp(-1.0, 10.0)]);
            }
            let pass: Vec<[f64; 2]> = obj.iter().zip(&self.evals).filter(|(_, e)| e.passed).map(|(p, _)| *p).collect();
            Plot::new("opt_plot").height(150.0).show(ui, |pu| {
                pu.points(Points::new("evaluations", PlotPoints::from(obj)).radius(2.0).color(pal.text_weak));
                pu.points(Points::new("passing", PlotPoints::from(pass)).radius(3.0).color(pal.ok));
                pu.line(Line::new("best", PlotPoints::from(best)).color(pal.accent));
            });
            let bk = self.best();
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                egui::Grid::new("opt_evals").striped(true).num_columns(3 + specs.len()).show(ui, |ui| {
                    ui.label("#");
                    for v in &self.setup.vars {
                        ui.label(egui::RichText::new(&v.target).strong());
                    }
                    for s in &specs {
                        ui.label(egui::RichText::new(&s.name).strong());
                    }
                    ui.label("objective");
                    ui.end_row();
                    for (k, e) in self.evals.iter().enumerate().rev().take(200) {
                        let mark = if Some(k) == bk { "*" } else { "" };
                        let c = if e.passed { pal.ok } else { pal.text };
                        ui.label(egui::RichText::new(format!("{}{mark}", k + 1)).color(c));
                        for v in &e.values {
                            ui.label(format_eng(*v, ""));
                        }
                        for (s, r) in specs.iter().zip(&e.specs) {
                            match r {
                                Ok(v) => {
                                    let ok = s.check(*v).unwrap_or(true);
                                    ui.label(egui::RichText::new(format_eng(*v, "")).color(if ok {
                                        pal.text
                                    } else {
                                        pal.danger
                                    }));
                                }
                                Err(err) => {
                                    ui.label(egui::RichText::new("error").color(pal.danger)).on_hover_text(err);
                                }
                            }
                        }
                        ui.label(format!("{:.4}", e.objective)).on_hover_text(&e.note);
                        ui.end_row();
                    }
                });
            });
        });
        self.open = open;
        (action, run)
    }
}
