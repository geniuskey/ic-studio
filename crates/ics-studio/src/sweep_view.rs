//! Corner / Sweep Manager (ADE Explorer / Assembler style): run the active cell over model corners,
//! temperatures and variable values, check specs on every point and overlay the waveforms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use ics_core::sweep::{Spec, SweepPoint, SweepSetup, SweepVar};
use ics_core::units::format_eng;
use ics_core::{Analysis, CellRef, Project};
use ics_sim::calc::{self, CalcValue};
use ics_sim::{SimOptions, SimResults};

use crate::theme::{self, Palette};

pub struct PointResult {
    pub point: SweepPoint,
    pub results: Result<SimResults, String>,
    /// Value of every spec, in the order of the setup's specs.
    pub specs: Vec<Result<f64, String>>,
}

struct Job {
    rx: Receiver<(usize, PointResult)>,
    total: usize,
    done: usize,
    cancel: Arc<AtomicBool>,
    started: Instant,
}

pub enum SweepAction {
    /// The draft setup changed; store it with the cell.
    Changed(SweepSetup),
    Run,
}

#[derive(Default)]
pub struct SweepManager {
    pub open: bool,
    /// Draft setup of `cell`.
    pub setup: SweepSetup,
    pub cell: Option<CellRef>,
    /// Results of the last run in point order (`None` while a point is still running).
    pub results: Vec<Option<PointResult>>,
    /// Specs of the last run.
    pub run_specs: Vec<Spec>,
    job: Option<Job>,
    status: Option<(bool, String)>,
    plot_analysis: String,
    plot_expr: String,
    selected: Option<usize>,
}

impl SweepManager {
    pub fn show(&mut self, cell: &CellRef, setup: &SweepSetup) {
        if self.cell.as_ref() != Some(cell) {
            self.cell = Some(cell.clone());
            self.setup = setup.clone();
            self.results.clear();
        }
        self.open = true;
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    pub fn status(&self) -> Option<&(bool, String)> {
        self.status.as_ref()
    }

    /// Run every point's deck on a pool of worker threads.
    pub fn start(&mut self, ctx: &egui::Context, decks: Vec<(SweepPoint, String)>, analyses: Vec<Analysis>, opts: SimOptions) {
        let total = decks.len();
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let next = Arc::new(AtomicUsize::new(0));
        let decks = Arc::new(decks);
        let specs = Arc::new(self.setup.specs.clone());
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).min(total).max(1);
        for _ in 0..workers {
            let (tx, cancel, next, decks, specs, analyses, opts, ctx) = (
                tx.clone(),
                cancel.clone(),
                next.clone(),
                decks.clone(),
                specs.clone(),
                analyses.clone(),
                opts.clone(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= decks.len() || cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let (point, deck) = &decks[i];
                    let results = ics_sim::simulate(deck, &analyses, &opts).map_err(|e| e.to_string());
                    let specs = specs.iter().map(|s| spec_value(s, &results)).collect();
                    if tx.send((i, PointResult { point: point.clone(), results, specs })).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        self.results = (0..total).map(|_| None).collect();
        self.run_specs = self.setup.specs.clone();
        self.selected = None;
        self.status = Some((true, format!("Running {total} point(s) on {workers} thread(s)…")));
        self.job = Some(Job { rx, total, done: 0, cancel, started: Instant::now() });
    }

    /// Collect finished points; returns a summary when the run completes.
    pub fn poll(&mut self) -> Option<String> {
        let job = self.job.as_mut()?;
        while let Ok((i, r)) = job.rx.try_recv() {
            if let Some(slot) = self.results.get_mut(i) {
                *slot = Some(r);
                job.done += 1;
            }
        }
        let cancelled = job.cancel.load(Ordering::SeqCst);
        if job.done < job.total && !cancelled {
            return None;
        }
        let secs = job.started.elapsed().as_secs_f64();
        let done = job.done;
        self.job = None;
        let failed = self.results.iter().flatten().filter(|r| r.results.is_err()).count();
        let (pass, checked) = self.pass_count();
        let mut msg = format!("{done} point(s) in {secs:.1} s");
        if failed > 0 {
            msg += &format!(", {failed} failed");
        }
        if checked > 0 {
            msg += &format!(", {pass}/{checked} pass every spec");
        }
        if cancelled {
            msg += " (stopped)";
        }
        self.status = Some((failed == 0, msg.clone()));
        Some(msg)
    }

    /// (points meeting every spec, points with specs checked).
    pub fn pass_count(&self) -> (usize, usize) {
        let mut pass = 0;
        let mut checked = 0;
        for r in self.results.iter().flatten() {
            let verdicts: Vec<Option<bool>> = self
                .run_specs
                .iter()
                .zip(&r.specs)
                .map(|(s, v)| v.as_ref().ok().and_then(|v| s.check(*v)).or(v.is_err().then_some(false)))
                .collect();
            if verdicts.iter().any(Option::is_some) {
                checked += 1;
                if verdicts.iter().all(|v| *v != Some(false)) {
                    pass += 1;
                }
            }
        }
        (pass, checked)
    }

    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>) -> Option<SweepAction> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        let builder = egui::ViewportBuilder::default()
            .with_title("Corner / Sweep Manager")
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([760.0, 480.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("sweep_manager"), builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                self.open = false;
            }
            self.ui(ui, project)
        })
    }

    fn ui(&mut self, ui: &mut egui::Ui, project: &Project) -> Option<SweepAction> {
        let pal = theme::palette(ui.ctx());
        if self.job.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let before = self.setup.clone();
        let base = self.cell.as_ref().and_then(|r| project.cell(r)).map(|c| c.sim.clone()).unwrap_or_default();
        let points = self.setup.points(&base);
        let mut action = None;
        egui::Panel::top("sw_toolbar")
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Corner / Sweep Manager").strong().color(pal.text_strong));
                    if let Some(c) = &self.cell {
                        ui.label(RichText::new(format!("· {c}")).color(pal.text_weak));
                    }
                    ui.separator();
                    match &points {
                        Ok(p) => ui.label(format!("{} point(s)", p.len())),
                        Err(e) => ui.label(RichText::new(e).color(pal.danger)),
                    };
                    let running = self.job.is_some();
                    if running {
                        if ui.button("Stop").clicked()
                            && let Some(j) = &self.job
                        {
                            j.cancel.store(true, Ordering::SeqCst);
                        }
                        if let Some(j) = &self.job {
                            ui.spinner();
                            ui.label(format!("{}/{}", j.done, j.total));
                        }
                    } else {
                        let b = egui::Button::new(RichText::new("▶  Run Sweep").strong().color(theme::on_fill(pal.simulation)))
                            .fill(pal.simulation);
                        if ui.add_enabled(points.is_ok() && self.cell.is_some(), b).clicked() {
                            action = Some(SweepAction::Run);
                        }
                    }
                    if let Some((ok, s)) = &self.status {
                        ui.label(RichText::new(s).small().color(if *ok { pal.ok } else { pal.warning }));
                    }
                });
            });
        egui::Panel::left("sw_setup")
            .resizable(true)
            .default_size(430.0)
            .size_range(320.0..=640.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("sw_setup_scroll").show(ui, |ui| self.setup_ui(ui, project, &base, &pal));
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(10)).show(ui, |ui| {
            if self.results.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("Pick corners, temperatures and variables on the left, add specs, then press ▶ Run Sweep.")
                            .color(pal.text_weak),
                    );
                });
                return;
            }
            egui::Panel::bottom("sw_plot").resizable(true).default_size(320.0).show(ui, |ui| self.overlay(ui, &pal));
            egui::CentralPanel::default().show(ui, |ui| self.table(ui, &pal));
        });
        if action.is_none() && self.setup != before {
            action = Some(SweepAction::Changed(self.setup.clone()));
        }
        action
    }

    fn setup_ui(&mut self, ui: &mut egui::Ui, project: &Project, base: &ics_core::SimSetup, pal: &Palette) {
        theme::section(ui, "Model corners");
        match project.tech.models.as_ref().filter(|m| !m.corners.is_empty()) {
            Some(m) => {
                ui.horizontal_wrapped(|ui| {
                    for c in &m.corners {
                        let mut on = self.setup.corners.contains(c);
                        if ui.checkbox(&mut on, c.as_str()).changed() {
                            if on {
                                self.setup.corners.push(c.clone());
                            } else {
                                self.setup.corners.retain(|x| x != c);
                            }
                        }
                    }
                });
                if self.setup.corners.is_empty() {
                    let shown = if base.corner.is_empty() { "default" } else { base.corner.as_str() };
                    ui.label(
                        RichText::new(format!("None checked: runs the setup's corner ({shown})")).small().color(pal.text_weak),
                    );
                }
            }
            None => {
                ui.label(RichText::new("The technology has no PDK model corners").small().color(pal.text_weak));
            }
        }
        theme::section(ui, "Temperatures");
        let l = ui.label(RichText::new("°C, e.g. -40 27 125 or -40:55:125 (empty = setup)").small().color(pal.text_weak));
        ui.add(
            egui::TextEdit::singleline(&mut self.setup.temps).hint_text(format!("{}", base.temp)).desired_width(f32::INFINITY),
        )
        .labelled_by(l.id);
        theme::section(ui, "Variables");
        ui.label(
            RichText::new("INST.param sets an instance parameter (R1.r, M1.w); other names replace {name} in the deck")
                .small()
                .color(pal.text_weak),
        );
        let mut remove = None;
        egui::Grid::new("sw_vars").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
            let hn = ui.label(RichText::new("NAME").small().strong().color(pal.text_weak));
            let hv = ui.label(RichText::new("VALUES").small().strong().color(pal.text_weak));
            ui.label("");
            ui.end_row();
            for (i, v) in self.setup.vars.iter_mut().enumerate() {
                ui.add_sized([110.0, 20.0], egui::TextEdit::singleline(&mut v.name).hint_text("R1.r")).labelled_by(hn.id);
                let bad = !v.values.trim().is_empty() && ics_core::sweep::parse_values(&v.values).is_err();
                let mut te = egui::TextEdit::singleline(&mut v.values).hint_text("1k 2k 5k");
                if bad {
                    te = te.text_color(pal.danger);
                }
                ui.add_sized([200.0, 20.0], te).labelled_by(hv.id);
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.setup.vars.remove(i);
        }
        if ui.button("+ Variable").clicked() {
            self.setup.vars.push(SweepVar::default());
        }
        theme::section(ui, "Specs");
        ui.label(RichText::new("Waveform calculator expressions measured on each point").small().color(pal.text_weak));
        let analyses: Vec<&str> = base.enabled().map(|a| a.kind_name()).collect();
        let mut remove = None;
        for (i, s) in self.setup.specs.iter_mut().enumerate() {
            egui::Frame::new().fill(pal.bg_widget).corner_radius(6).inner_margin(6).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new(("sw_spec", i)).num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
                    let l = ui.label("Name");
                    ui.add_sized([140.0, 20.0], egui::TextEdit::singleline(&mut s.name).hint_text("gain")).labelled_by(l.id);
                    let l = ui.label("Dataset");
                    if s.analysis.is_empty() {
                        s.analysis =
                            analyses.iter().find(|a| **a != "op").or(analyses.first()).map_or(String::new(), |a| a.to_string());
                    }
                    egui::ComboBox::from_id_salt(("sw_spec_an", i))
                        .selected_text(s.analysis.as_str())
                        .show_ui(ui, |ui| {
                            for a in &analyses {
                                ui.selectable_value(&mut s.analysis, a.to_string(), *a);
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    ui.end_row();
                    let l = ui.label("Expr");
                    ui.add_sized([140.0, 20.0], egui::TextEdit::singleline(&mut s.expr).code_editor().hint_text("max(v(out))"))
                        .labelled_by(l.id);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                    let l = ui.label("Min");
                    ui.add_sized([140.0, 20.0], egui::TextEdit::singleline(&mut s.min)).labelled_by(l.id);
                    let l = ui.label("Max");
                    ui.add_sized([100.0, 20.0], egui::TextEdit::singleline(&mut s.max)).labelled_by(l.id);
                    ui.end_row();
                });
            });
            ui.add_space(4.0);
        }
        if let Some(i) = remove {
            self.setup.specs.remove(i);
        }
        if ui.button("+ Spec").clicked() {
            self.setup.specs.push(Spec { name: format!("spec{}", self.setup.specs.len() + 1), ..Default::default() });
        }
    }

    fn table(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let (pass, checked) = self.pass_count();
        if checked > 0 {
            let all = pass == checked;
            ui.label(RichText::new(format!("{pass} of {checked} point(s) meet every spec")).strong().color(if all {
                pal.ok
            } else {
                pal.warning
            }));
        }
        egui::ScrollArea::both().id_salt("sw_table").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("sw_grid").striped(true).num_columns(self.run_specs.len() + 3).spacing([18.0, 4.0]).show(ui, |ui| {
                let head = |ui: &mut egui::Ui, t: &str| ui.label(RichText::new(t).small().strong().color(pal.text_weak));
                head(ui, "#");
                head(ui, "POINT");
                head(ui, "STATUS");
                for s in &self.run_specs {
                    let lim = match (s.min.trim(), s.max.trim()) {
                        ("", "") => String::new(),
                        (lo, "") => format!(" ≥ {lo}"),
                        ("", hi) => format!(" ≤ {hi}"),
                        (lo, hi) => format!(" {lo}…{hi}"),
                    };
                    head(ui, &format!("{}{lim}", s.name.to_uppercase()));
                }
                ui.end_row();
                for (i, r) in self.results.iter().enumerate() {
                    let sel = self.selected == Some(i);
                    if ui.selectable_label(sel, format!("{}", i + 1)).clicked() {
                        self.selected = if sel { None } else { Some(i) };
                    }
                    let Some(r) = r else {
                        ui.label(RichText::new("…").color(pal.text_weak));
                        ui.label(RichText::new("running").small().color(pal.text_weak));
                        ui.end_row();
                        continue;
                    };
                    ui.label(RichText::new(r.point.label()).monospace());
                    match &r.results {
                        Ok(res) => ui.label(RichText::new(format!("ok · {:.0} ms", res.elapsed_ms)).small().color(pal.ok)),
                        Err(e) => ui.label(RichText::new("failed").small().color(pal.danger)).on_hover_text(e.as_str()),
                    };
                    for (s, v) in self.run_specs.iter().zip(&r.specs) {
                        match v {
                            Ok(x) => {
                                let c = match s.check(*x) {
                                    Some(true) => pal.ok,
                                    Some(false) => pal.danger,
                                    None => pal.text,
                                };
                                ui.label(RichText::new(format_eng(*x, "")).monospace().color(c));
                            }
                            Err(e) => {
                                ui.label(RichText::new("error").small().color(pal.danger)).on_hover_text(e.as_str());
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn overlay(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let analyses: Vec<String> = self
            .results
            .iter()
            .flatten()
            .find_map(|r| r.results.as_ref().ok())
            .map(|r| r.datasets.iter().filter(|d| d.analysis != "op").map(|d| d.analysis.clone()).collect())
            .unwrap_or_default();
        if !analyses.contains(&self.plot_analysis) {
            self.plot_analysis = analyses.first().cloned().unwrap_or_default();
        }
        if self.plot_expr.is_empty()
            && let Some(ds) = self.results.iter().flatten().find_map(|r| r.results.as_ref().ok()?.dataset(&self.plot_analysis))
        {
            let v = ds.get("v(out)").or_else(|| ds.signals().find(|v| v.kind == ics_sim::VecKind::Voltage));
            self.plot_expr = v.map_or(String::new(), |v| v.name.clone());
        }
        ui.horizontal(|ui| {
            theme::section(ui, "Overlay");
            let l = ui.label("Dataset");
            egui::ComboBox::from_id_salt("sw_plot_an")
                .selected_text(self.plot_analysis.as_str())
                .show_ui(ui, |ui| {
                    for a in &analyses {
                        ui.selectable_value(&mut self.plot_analysis, a.clone(), a.as_str());
                    }
                })
                .response
                .labelled_by(l.id);
            let l = ui.label("Signal");
            ui.add(
                egui::TextEdit::singleline(&mut self.plot_expr)
                    .code_editor()
                    .desired_width(260.0)
                    .hint_text("v(out) or any expression"),
            )
            .labelled_by(l.id);
        });
        if self.plot_expr.trim().is_empty() {
            ui.label(RichText::new("Enter a signal or expression to overlay it across the points.").color(pal.text_weak));
            return;
        }
        let log = self.plot_analysis == "ac";
        let trace = if pal.dark { DARK } else { LIGHT };
        let mut lines = vec![];
        let mut error = None;
        for (i, r) in self.results.iter().enumerate() {
            let Some(Ok(res)) = r.as_ref().map(|r| r.results.as_ref()) else { continue };
            let Some(ds) = res.dataset(&self.plot_analysis) else { continue };
            let Some(scale) = ds.scale_vector() else { continue };
            match calc::eval(&self.plot_expr, ds) {
                Ok(CalcValue::Wave(w)) => {
                    let ys = if w.im.is_some() { w.db() } else { w.re.clone() };
                    let pts: Vec<[f64; 2]> =
                        scale.re.iter().zip(ys).map(|(x, y)| [if log { x.max(1e-30).log10() } else { *x }, y]).collect();
                    let name = r.as_ref().map_or(String::new(), |r| format!("{} {}", i + 1, r.point.label()));
                    let mut c = trace[i % trace.len()];
                    let mut width = 1.5;
                    if let Some(s) = self.selected {
                        if s == i {
                            width = 3.0;
                        } else {
                            c = c.gamma_multiply(0.35);
                        }
                    }
                    lines.push((name, pts, c, width));
                }
                Ok(CalcValue::Scalar(_)) => error = Some("the expression gives a number, not a waveform".to_string()),
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error.filter(|_| lines.is_empty()) {
            ui.label(RichText::new(e).color(pal.danger));
            return;
        }
        Plot::new("sw_overlay")
            .legend(Legend::default())
            .x_axis_formatter(move |m, _| format_eng(if log { 10f64.powf(m.value) } else { m.value }, ""))
            .y_axis_formatter(|m, _| format_eng(m.value, ""))
            .show(ui, |pu| {
                for (name, pts, c, w) in lines {
                    pu.line(Line::new(name, PlotPoints::from(pts)).color(c).width(w));
                }
            });
    }
}

fn spec_value(s: &Spec, results: &Result<SimResults, String>) -> Result<f64, String> {
    let res = results.as_ref().map_err(|_| "simulation failed".to_string())?;
    let ds = res.dataset(&s.analysis).ok_or_else(|| format!("no {} results", s.analysis))?;
    match calc::eval(&s.expr, ds)? {
        CalcValue::Scalar(x) => Ok(x),
        CalcValue::Wave(w) if w.re.len() == 1 => Ok(w.re[0]),
        CalcValue::Wave(_) => Err("the expression gives a waveform; use a measurement such as max(…)".into()),
    }
}

const DARK: [Color32; 8] = [
    Color32::from_rgb(0x4c, 0x9b, 0xff),
    Color32::from_rgb(0xf2, 0xa9, 0x3b),
    Color32::from_rgb(0x4c, 0xc3, 0x8a),
    Color32::from_rgb(0xf0, 0x5d, 0x5e),
    Color32::from_rgb(0xb5, 0x7b, 0xff),
    Color32::from_rgb(0x3d, 0xd6, 0xd0),
    Color32::from_rgb(0xff, 0x7e, 0xc8),
    Color32::from_rgb(0xc8, 0xd6, 0x4c),
];

const LIGHT: [Color32; 8] = [
    Color32::from_rgb(0x1f, 0x6f, 0xeb),
    Color32::from_rgb(0xc7, 0x6a, 0x00),
    Color32::from_rgb(0x1a, 0x8a, 0x4f),
    Color32::from_rgb(0xd0, 0x2f, 0x2f),
    Color32::from_rgb(0x7c, 0x3a, 0xd8),
    Color32::from_rgb(0x00, 0x8b, 0x8b),
    Color32::from_rgb(0xc2, 0x2a, 0x84),
    Color32::from_rgb(0x6b, 0x7a, 0x00),
];
