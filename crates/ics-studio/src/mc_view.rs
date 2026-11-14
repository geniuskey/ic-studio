//! Monte Carlo window: process and mismatch variation of the built-in engine's device models
//! (`ics_sim::montecarlo`), with statistics and a histogram per measurement.

use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, RichText};
use egui_plot::{Bar, BarChart, Plot};
use ics_core::sim_setup::{McMeasure, McSetup};
use ics_core::units::format_eng;
use ics_core::{Analysis, CellRef};
use ics_sim::montecarlo::{self, McResult, McSpec, Measure};

const ANALYSES: [&str; 5] = ["op", "dc", "ac", "tran", "noise"];

#[derive(Default)]
pub struct MonteCarlo {
    pub open: bool,
    pub cell: Option<CellRef>,
    pub setup: McSetup,
    pub result: Option<McResult>,
    /// The measures of the last run, in the order of `result`.
    pub measured: Vec<McMeasure>,
    pub error: Option<String>,
    /// Measure whose histogram is shown.
    pub selected: usize,
    rx: Option<Receiver<Result<McResult, String>>>,
}

impl MonteCarlo {
    pub fn show(&mut self, cell: &CellRef, setup: &McSetup) {
        if self.cell.as_ref() != Some(cell) {
            self.result = None;
            self.measured.clear();
            self.error = None;
            self.selected = 0;
        }
        self.cell = Some(cell.clone());
        self.setup = setup.clone();
        self.open = true;
    }

    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    pub fn spec(&self) -> Result<McSpec, String> {
        let (vto_sigma, kp_sigma, avt, abeta) = self.setup.sigmas()?;
        Ok(McSpec { runs: self.setup.runs, seed: self.setup.seed, vto_sigma, kp_sigma, avt, abeta })
    }

    /// Run the setup's measures on `deck` in a background thread.
    pub fn start(&mut self, ctx: &egui::Context, deck: String, analyses: Vec<Analysis>) -> Result<(), String> {
        if self.is_running() {
            return Err("a Monte Carlo run is already in progress".into());
        }
        let spec = self.spec()?;
        let measured: Vec<McMeasure> = self.setup.measures.iter().filter(|m| !m.expr.trim().is_empty()).cloned().collect();
        if measured.is_empty() {
            return Err("add a measurement".into());
        }
        let measures: Vec<Measure> = measured
            .iter()
            .map(|m| Measure { analysis: m.analysis.trim().to_ascii_lowercase(), expr: m.expr.trim().to_string() })
            .collect();
        self.measured = measured;
        self.result = None;
        self.error = None;
        self.selected = 0;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = montecarlo::monte_carlo(&deck, &analyses, &measures, &spec).map_err(|e| e.to_string());
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        Ok(())
    }

    /// Pick up a finished run; returns (ok, console line).
    pub fn poll(&mut self) -> Option<(bool, String)> {
        let r = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        Some(match r {
            Ok(res) => {
                let runs = self.setup.runs;
                let failed = res.failed.len();
                let summary: Vec<String> = self
                    .measured
                    .iter()
                    .zip(&res.stats)
                    .map(|(m, s)| format!("{} mean {} sigma {}", m.expr, format_eng(s.mean, ""), format_eng(s.sigma, "")))
                    .collect();
                self.result = Some(res);
                (failed == 0, format!("Monte Carlo: {runs} run(s), {failed} failed; {}", summary.join("; ")))
            }
            Err(e) => {
                self.error = Some(e.clone());
                (false, format!("Monte Carlo: {e}"))
            }
        })
    }

    /// Returns (setup changed, run requested).
    pub fn window(&mut self, ctx: &egui::Context) -> (Option<McSetup>, bool) {
        if !self.open {
            return (None, false);
        }
        let (mut changed, mut run) = (false, false);
        let mut open = self.open;
        let title = match &self.cell {
            Some(c) => format!("Monte Carlo: {c}"),
            None => "Monte Carlo".into(),
        };
        crate::float::tool_window(ctx, "monte_carlo", &title, [620.0, 620.0], &mut open, |ui| {
            let running = self.is_running();
            ui.add_enabled_ui(!running, |ui| {
                egui::Grid::new("mc_spec").num_columns(4).min_col_width(80.0).spacing([8.0, 4.0]).show(ui, |ui| {
                    let l = ui.label("Runs");
                    changed |= ui.add(egui::DragValue::new(&mut self.setup.runs).range(1..=100_000)).labelled_by(l.id).changed();
                    let l = ui.label("Seed");
                    changed |= ui.add(egui::DragValue::new(&mut self.setup.seed)).labelled_by(l.id).changed();
                    ui.end_row();
                    for (k, (label, text, hint)) in [
                        ("Vto sigma", &mut self.setup.vto_sigma, "Global threshold shift per polarity (V, one sigma)"),
                        ("kp sigma", &mut self.setup.kp_sigma, "Global relative transconductance shift per polarity"),
                        ("Avt", &mut self.setup.avt, "Threshold mismatch coefficient (V*m): 5n = 5 mV*um"),
                        ("Abeta", &mut self.setup.abeta, "Current factor mismatch coefficient (m): 10n = 1 %*um"),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let l = ui.label(label);
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(80.0)).labelled_by(l.id);
                        changed |= r.on_hover_text(hint).changed();
                        if k % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new("Measurements").strong());
                let mut remove = None;
                egui::Grid::new("mc_measures").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
                    for (k, m) in self.setup.measures.iter_mut().enumerate() {
                        let l = ui.label(format!("{}", k + 1));
                        egui::ComboBox::from_id_salt(("mc_an", k))
                            .selected_text(m.analysis.as_str())
                            .width(60.0)
                            .show_ui(ui, |ui| {
                                for a in ANALYSES {
                                    changed |= ui.selectable_value(&mut m.analysis, a.to_string(), a).changed();
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        let r = ui.add(egui::TextEdit::singleline(&mut m.expr).desired_width(300.0)).labelled_by(l.id);
                        changed |= r.on_hover_text("Waveform calculator expression with a scalar result").changed();
                        if ui.small_button("x").on_hover_text("Remove").clicked() {
                            remove = Some(k);
                        }
                        ui.end_row();
                    }
                });
                if let Some(k) = remove {
                    self.setup.measures.remove(k);
                    changed = true;
                }
                if ui.button("+ Measurement").on_hover_text("e.g. dc: cross(v(out),0.9) or op: v(out)").clicked() {
                    self.setup.measures.push(McMeasure { analysis: "op".into(), expr: "v(out)".into() });
                    changed = true;
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = self.setup.measures.iter().any(|m| !m.expr.trim().is_empty());
                if ui.add_enabled(!running && ready, egui::Button::new("Run")).clicked() {
                    run = true;
                }
                if running {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("Running {} sample(s)", self.setup.runs));
                }
            });
            let pal = crate::theme::palette(ui.ctx());
            if let Some(e) = &self.error {
                ui.colored_label(pal.danger, e);
            }
            let Some(res) = &self.result else { return };
            ui.separator();
            egui::Grid::new("mc_stats").striped(true).num_columns(7).show(ui, |ui| {
                for h in ["measure", "n", "mean", "sigma", "min", "max", "failed"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (k, (m, s)) in self.measured.iter().zip(&res.stats).enumerate() {
                    if ui.selectable_label(self.selected == k, format!("{}: {}", m.analysis, m.expr)).clicked() {
                        self.selected = k;
                    }
                    ui.label(s.n.to_string());
                    for v in [s.mean, s.sigma, s.min, s.max] {
                        ui.label(if s.n > 0 { format_eng(v, "") } else { "-".into() });
                    }
                    ui.label(res.failed.len().to_string());
                    ui.end_row();
                }
            });
            if let Some((run, e)) = res.failed.first() {
                ui.colored_label(pal.danger, format!("run {run}: {e}")).on_hover_text("First failed run");
            }
            let values: Vec<f64> = res.values.get(self.selected).map(|v| v.iter().map(|p| p.1).collect()).unwrap_or_default();
            let Some(m) = self.measured.get(self.selected) else { return };
            if values.is_empty() {
                ui.label(RichText::new("No values for this measurement").weak());
                return;
            }
            let (bars, width) = histogram(&values, 20);
            let bars: Vec<Bar> = bars.into_iter().map(|(x, c)| Bar::new(x, c).width(width * 0.9).fill(pal.simulation)).collect();
            Plot::new("mc_hist").height(220.0).x_axis_formatter(|g, _| format_eng(g.value, "")).allow_scroll(false).show(
                ui,
                |pu| {
                    pu.bar_chart(BarChart::new(m.expr.clone(), bars));
                },
            );
        });
        self.open = open;
        (changed.then(|| self.setup.clone()), run)
    }
}

/// Bin centres with counts, and the bin width.
fn histogram(values: &[f64], bins: usize) -> (Vec<(f64, f64)>, f64) {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;
    let width = if span > 0.0 { span / bins as f64 } else { lo.abs().max(1e-12) * 0.01 };
    let n = if span > 0.0 { bins } else { 1 };
    let mut counts = vec![0.0; n];
    for v in values {
        let k = if span > 0.0 { (((v - lo) / width) as usize).min(n - 1) } else { 0 };
        counts[k] += 1.0;
    }
    let start = if span > 0.0 { lo } else { lo - width / 2.0 };
    (counts.into_iter().enumerate().map(|(k, c)| (start + (k as f64 + 0.5) * width, c)).collect(), width)
}
