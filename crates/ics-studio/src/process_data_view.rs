//! Process Data window: WAT / PCM results against the process specification, with process
//! capability, control charts of the wafer means and the model corners
//! (`ics_core::process_data`).

use eframe::egui::{self, RichText};
use egui_plot::{Bar, BarChart, HLine, Legend, Line, LineStyle, Plot, PlotPoints, Points, VLine};
use ics_core::process_data::{GENERIC180_SPEC, ParamSummary, ProcessSpec, WatData, summarize};
use ics_core::units::format_eng;

pub enum ProcessDataAction {
    LoadData,
    LoadSpec,
    /// Fit corner models to the data (needs the project's technology).
    ExtractCorners,
    /// Save the corner library text.
    SaveLibrary(String),
    /// Store the process variation in the project's technology for Monte Carlo.
    ApplyVariation(ics_core::process_data::ProcessVariation),
}

/// The result of a corner extraction.
#[derive(Debug, Clone)]
pub struct CornerResult {
    pub knobs: Vec<String>,
    pub fits: Vec<ics_sim::corners::CornerFit>,
    pub library: String,
    /// The global process variation of the knobs, or why there is none.
    pub variation: Result<ics_sim::corners::VariationFit, String>,
}

#[derive(Default)]
pub struct ProcessDataView {
    pub open: bool,
    pub data: Option<WatData>,
    pub spec: ProcessSpec,
    pub summary: Vec<ParamSummary>,
    /// Shown file names.
    pub data_name: String,
    pub spec_name: String,
    pub selected: usize,
    pub error: Option<String>,
    pub corners: Option<CornerResult>,
    rx: Option<std::sync::mpsc::Receiver<Result<CornerResult, String>>>,
}

impl ProcessDataView {
    pub fn show(&mut self) {
        if self.spec_name.is_empty() {
            self.set_spec_text(GENERIC180_SPEC, "generic180 WAT (built in)");
        }
        self.open = true;
    }

    pub fn set_spec_text(&mut self, text: &str, name: &str) {
        match ProcessSpec::from_toml(text) {
            Ok(s) => {
                self.spec = s;
                self.spec_name = name.to_string();
                self.error = None;
                self.refresh();
            }
            Err(e) => self.error = Some(format!("{name}: {e}")),
        }
    }

    pub fn set_data_text(&mut self, text: &str, name: &str) {
        match WatData::parse(text) {
            Ok(d) => {
                self.data = Some(d);
                self.data_name = name.to_string();
                self.selected = 0;
                self.error = None;
                self.refresh();
            }
            Err(e) => self.error = Some(format!("{name}: {e}")),
        }
    }

    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Fit the corner models on a copy of the project in a background thread.
    pub fn start_corners(&mut self, ctx: &egui::Context, project: ics_core::Project) -> Result<(), String> {
        let data = self.data.clone().ok_or("load the WAT data first")?;
        let spec = self.spec.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let run = || -> Result<CornerResult, String> {
                let ex = ics_sim::corners::Extraction::new(&project, &spec, &data, &ics_sim::SimOptions::default())?;
                let fits = ex.run(40, &mut |_| {});
                let variation =
                    fits.first().ok_or("no corners".to_string()).and_then(|tt| ex.variation(tt, &format!("{} WAT", spec.name)));
                let library =
                    ex.library(&fits, variation.as_ref().ok().map(|v| &v.variation), &format!("{} corners", spec.name))?;
                Ok(CornerResult { knobs: ex.knobs.iter().map(|k| k.target.clone()).collect(), fits, library, variation })
            };
            let _ = tx.send(run());
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.corners = None;
        Ok(())
    }

    /// Pick up a finished extraction; returns (ok, console line).
    pub fn poll(&mut self) -> Option<(bool, String)> {
        let r = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        Some(match r {
            Ok(c) => {
                let worst = c.fits.iter().map(|f| f.error_sigma).fold(0.0, f64::max);
                let msg = format!("Corner models: {} corner(s) fitted, worst error {worst:.3}σ", c.fits.len());
                self.corners = Some(c);
                (true, msg)
            }
            Err(e) => {
                self.error = Some(e.clone());
                (false, format!("Corner models: {e}"))
            }
        })
    }

    fn refresh(&mut self) {
        self.summary = self.data.as_ref().map(|d| summarize(d, &self.spec)).unwrap_or_default();
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<ProcessDataAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let mut open = self.open;
        crate::float::tool_window(ctx, "process_data", "Process Data", [900.0, 780.0], &mut open, |ui| {
            egui::ScrollArea::vertical().id_salt("pd_scroll").show(ui, |ui| {
                let pal = crate::theme::palette(ui.ctx());
                ui.horizontal(|ui| {
                    let l = ui.label("Data");
                    let shown = if self.data_name.is_empty() { "none" } else { self.data_name.as_str() };
                    ui.label(RichText::new(shown).monospace()).labelled_by(l.id);
                    if ui.button("Load CSV…").clicked() {
                        action = Some(ProcessDataAction::LoadData);
                    }
                    let l = ui.label("Spec");
                    ui.label(RichText::new(&self.spec_name).monospace()).labelled_by(l.id);
                    if ui.button("Load spec…").clicked() {
                        action = Some(ProcessDataAction::LoadSpec);
                    }
                    if ui.button("Built-in spec").on_hover_text("The generic 180 nm WAT specification").clicked() {
                        self.set_spec_text(GENERIC180_SPEC, "generic180 WAT (built in)");
                    }
                    let can = self.data.is_some() && self.spec.extraction.is_some() && !self.is_running();
                    if ui
                        .add_enabled(can, egui::Button::new("Extract corners"))
                        .on_hover_text("Fit tt / ff / ss / fs / sf model cards to the data (the spec's [extraction])")
                        .clicked()
                    {
                        action = Some(ProcessDataAction::ExtractCorners);
                    }
                    if self.is_running() {
                        ui.spinner();
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(pal.danger, e);
                }
                let Some(d) = &self.data else {
                    ui.label(
                        RichText::new("Load WAT / PCM data: CSV with lot, wafer and site columns and one column per parameter.")
                            .color(pal.text_weak),
                    );
                    return;
                };
                let wafers = d.wafers();
                ui.label(RichText::new(format!("{} sites on {} wafers", d.sites.len(), wafers.len())).color(pal.text_weak));
                crate::theme::section(ui, "Parameters");
                egui::Grid::new("pd_grid").striped(true).num_columns(10).spacing([12.0, 3.0]).show(ui, |ui| {
                    for h in ["PARAMETER", "UNIT", "MEAN", "σ", "LSL", "USL", "Cpk", "OOS", "SPC", "CORNERS"] {
                        ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                    }
                    ui.end_row();
                    for (k, s) in self.summary.iter().enumerate() {
                        let ps = self.spec.param(&s.name);
                        if ui
                            .selectable_label(self.selected == k, &s.name)
                            .on_hover_text(ps.map_or("", |p| p.description.as_str()))
                            .clicked()
                        {
                            self.selected = k;
                        }
                        ui.label(ps.map_or("", |p| p.unit.as_str()));
                        ui.label(RichText::new(format_eng(s.mean, "")).monospace());
                        ui.label(RichText::new(format_eng(s.std, "")).monospace());
                        let lim = |v: Option<f64>| v.map_or("—".to_string(), |x| format_eng(x, ""));
                        ui.label(RichText::new(lim(ps.and_then(|p| p.lsl))).monospace().color(pal.text_weak));
                        ui.label(RichText::new(lim(ps.and_then(|p| p.usl))).monospace().color(pal.text_weak));
                        match s.cpk {
                            Some(c) => {
                                let col = if c >= 1.33 {
                                    pal.ok
                                } else if c >= 1.0 {
                                    pal.warning
                                } else {
                                    pal.danger
                                };
                                ui.label(RichText::new(format!("{c:.2}")).monospace().strong().color(col))
                            }
                            None => ui.label("—"),
                        };
                        ui.label(RichText::new(s.out_of_spec.to_string()).color(if s.out_of_spec > 0 {
                            pal.danger
                        } else {
                            pal.text
                        }));
                        if s.violations.is_empty() {
                            ui.label(RichText::new("in control").color(pal.ok));
                        } else {
                            let (lot, wafer, _) = &wafers[s.violations[0].wafer];
                            ui.label(RichText::new(format!("from {lot}/{wafer}")).color(pal.danger))
                                .on_hover_text(s.violations[0].message.clone());
                        }
                        match s.corners_cover {
                            Some(true) => ui.label(RichText::new("cover ±3σ").color(pal.ok)),
                            Some(false) => ui.label(RichText::new("narrower than ±3σ").color(pal.warning)),
                            None => ui.label("—"),
                        };
                        ui.end_row();
                    }
                });
                if let Some(c) = &self.corners {
                    crate::theme::section(ui, "Corner models");
                    egui::Grid::new("pd_corners").striped(true).num_columns(c.knobs.len() + 2).spacing([12.0, 3.0]).show(
                        ui,
                        |ui| {
                            ui.label(RichText::new("CORNER").small().strong().color(pal.text_weak));
                            for k in &c.knobs {
                                ui.label(RichText::new(k.trim_start_matches("model:")).small().strong().color(pal.text_weak));
                            }
                            ui.label(RichText::new("ERROR").small().strong().color(pal.text_weak));
                            ui.end_row();
                            for f in &c.fits {
                                ui.label(RichText::new(&f.name).strong());
                                for v in &f.values {
                                    ui.label(RichText::new(format_eng(*v, "")).monospace());
                                }
                                let col = if f.error_sigma < 0.1 { pal.ok } else { pal.warning };
                                ui.label(RichText::new(format!("{:.2}σ", f.error_sigma)).monospace().color(col)).on_hover_text(
                                    f.params
                                        .iter()
                                        .map(|(n, t, g)| {
                                            format!(
                                                "{n}: {} (target {})",
                                                g.as_ref().map_or("failed".into(), |v| format_eng(*v, "")),
                                                format_eng(*t, "")
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                );
                                ui.end_row();
                            }
                        },
                    );
                    match &c.variation {
                        Ok(v) => {
                            let pv = &v.variation;
                            crate::theme::section(ui, "Process variation (global, wafer to wafer)");
                            let n = pv.params.len();
                            egui::Grid::new("pd_variation").striped(true).num_columns(n + 3).spacing([12.0, 3.0]).show(ui, |ui| {
                                for h in ["PARAMETER", "NOMINAL", "SIGMA"] {
                                    ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                                }
                                for p in &pv.params {
                                    ui.label(RichText::new(format!("r {p}")).small().strong().color(pal.text_weak));
                                }
                                ui.end_row();
                                for i in 0..n {
                                    ui.label(RichText::new(&pv.params[i]).strong());
                                    ui.label(RichText::new(format_eng(pv.mean[i], "")).monospace());
                                    let rel = 100.0 * pv.sigma(i) / pv.mean[i].abs().max(1e-300);
                                    ui.label(RichText::new(format!("{} ({rel:.1} %)", format_eng(pv.sigma(i), ""))).monospace());
                                    for j in 0..n {
                                        let r = pv.correlation(i, j);
                                        let t = RichText::new(format!("{r:+.2}")).monospace();
                                        ui.label(if i == j { t.color(pal.text_weak) } else { t });
                                    }
                                    ui.end_row();
                                }
                            });
                            let worst = v.params.iter().map(|(_, d, g)| (g / d - 1.0).abs()).fold(0.0, f64::max);
                            ui.label(RichText::new(format!(
                                "Reproduces the measured σ of {} within {:.1} %.",
                                v.params.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(", "),
                                100.0 * worst
                            ))
                            .small()
                            .color(pal.text_weak))
                            .on_hover_text(
                                v.params
                                    .iter()
                                    .map(|(n, d, g)| format!("{n}: σ {} (data {})", format_eng(*g, ""), format_eng(*d, "")))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            );
                        }
                        Err(e) => {
                            ui.colored_label(pal.warning, format!("No process variation: {e}"));
                        }
                    }
                    ui.horizontal(|ui| {
                        if let Ok(v) = &c.variation
                            && ui
                                .button("Use for Monte Carlo")
                                .on_hover_text("Store the variation in the project's technology: Monte Carlo then draws it in place of the generic sigmas")
                                .clicked()
                        {
                            action = Some(ProcessDataAction::ApplyVariation(v.variation.clone()));
                        }
                        if ui.button("Save library…").clicked() {
                            action = Some(ProcessDataAction::SaveLibrary(c.library.clone()));
                        }
                        if ui.button("Copy library").clicked() {
                            ui.ctx().copy_text(c.library.clone());
                        }
                    });
                }
                let Some(s) = self.summary.get(self.selected) else { return };
                let ps = self.spec.param(&s.name);
                ui.add_space(4.0);
                crate::theme::section(ui, &format!("Control chart · {} (wafer means)", s.name));
                let h = 230.0;
                let flagged: Vec<usize> = s.violations.iter().map(|v| v.wafer).collect();
                Plot::new("pd_chart")
                    .height(h)
                    .legend(Legend::default().position(egui_plot::Corner::LeftBottom))
                    .y_axis_formatter(|m, _| format_eng(m.value, ""))
                    .x_axis_formatter(move |m, _| format!("{}", m.value.round() as i64 + 1))
                    .allow_scroll(false)
                    .show(ui, |pu| {
                        let pts: Vec<[f64; 2]> = s
                            .wafer_means
                            .iter()
                            .enumerate()
                            .filter(|(_, v)| v.is_finite())
                            .map(|(i, v)| [i as f64, *v])
                            .collect();
                        pu.line(Line::new("wafer mean", PlotPoints::from(pts.clone())).color(pal.accent));
                        pu.points(Points::new("wafers", pts).color(pal.accent).radius(2.5));
                        let bad: Vec<[f64; 2]> = flagged.iter().map(|&i| [i as f64, s.wafer_means[i]]).collect();
                        if !bad.is_empty() {
                            pu.points(Points::new("control rule", bad).color(pal.danger).radius(4.5));
                        }
                        pu.hline(HLine::new("centre", s.center).color(pal.text_weak));
                        pu.hline(HLine::new("UCL / LCL", s.ucl).color(pal.warning).style(LineStyle::dashed_dense()));
                        pu.hline(HLine::new("UCL / LCL", s.lcl).color(pal.warning).style(LineStyle::dashed_dense()));
                        for l in [ps.and_then(|p| p.lsl), ps.and_then(|p| p.usl)].into_iter().flatten() {
                            pu.hline(HLine::new("spec limit", l).color(pal.danger).width(1.5));
                        }
                        // Lot boundaries.
                        for i in 1..wafers.len() {
                            if wafers[i].0 != wafers[i - 1].0 {
                                pu.vline(VLine::new("lot", i as f64 - 0.5).color(pal.border).style(LineStyle::dotted_dense()));
                            }
                        }
                    });
                crate::theme::section(ui, &format!("Distribution · {} (sites)", s.name));
                let j = d.params.iter().position(|p| *p == s.name).unwrap_or(0);
                let vals: Vec<f64> = d.values[j].iter().copied().filter(|v| v.is_finite()).collect();
                let bins = 30usize;
                let mut lo = s.min;
                let mut hi = s.max;
                for v in ps.into_iter().flat_map(|p| [p.lsl, p.usl].into_iter().flatten().chain(p.corners.values().copied())) {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                let w = if hi > lo { (hi - lo) / bins as f64 } else { 1.0 };
                let mut counts = vec![0u32; bins];
                for v in &vals {
                    counts[(((v - lo) / w) as usize).min(bins - 1)] += 1;
                }
                let bars: Vec<Bar> = counts
                    .iter()
                    .enumerate()
                    .map(|(i, c)| Bar::new(lo + (i as f64 + 0.5) * w, *c as f64).width(w * 0.9).fill(pal.accent))
                    .collect();
                Plot::new("pd_hist")
                    .height(h)
                    .legend(Legend::default().position(egui_plot::Corner::LeftTop))
                    .x_axis_formatter(|m, _| format_eng(m.value, ""))
                    .allow_scroll(false)
                    .show(ui, |pu| {
                        pu.bar_chart(BarChart::new("sites", bars).color(pal.accent));
                        for l in [ps.and_then(|p| p.lsl), ps.and_then(|p| p.usl)].into_iter().flatten() {
                            pu.vline(VLine::new("spec limit", l).color(pal.danger).width(1.5));
                        }
                        for (name, v, _) in &s.corners {
                            pu.vline(VLine::new(format!("corner {name}"), *v).color(pal.ok).style(LineStyle::dashed_dense()));
                        }
                    });
            });
        });
        self.open = open;
        action
    }
}
