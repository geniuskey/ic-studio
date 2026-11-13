//! Corner / Sweep Manager (ADE Explorer / Assembler style): run the active cell over model corners,
//! temperatures and variable values, check specs on every point and overlay the waveforms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints, VLine};
use ics_core::sweep::{Dist, McVar, Spec, SweepPoint, SweepSetup, SweepVar, stats};
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
    /// Spec shown in the Monte Carlo histogram.
    hist_spec: usize,
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
        let mut open = true;
        let r = crate::float::tool_window(ctx, "sweep_manager", "Corner / Sweep Manager", [1240.0, 780.0], &mut open, |ui| {
            self.ui(ui, project)
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(&mut self, ui: &mut egui::Ui, project: &Project) -> Option<SweepAction> {
        let pal = theme::palette(ui.ctx());
        if self.job.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let before = self.setup.clone();
        let base = self.cell.as_ref().and_then(|r| project.cell(r)).map(|c| c.sim.clone()).unwrap_or_default();
        let runs = self.setup.monte_carlo.runs.max(1) as usize;
        let points = self.setup.points(&base).map(|p| p.len() * runs);
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
                        Ok(n) => ui.label(format!("{n} point(s)")),
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
                    if !running
                        && !self.results.is_empty()
                        && ui.button("Export datasheet…").on_hover_text("Specs, statistics and every point as HTML").clicked()
                    {
                        let name = self.cell.as_ref().map_or("sweep".into(), |c| c.cell.clone());
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name(format!("{name}_datasheet.html"))
                            .add_filter("HTML", &["html"])
                            .save_file()
                        {
                            self.status = Some(match std::fs::write(&path, self.datasheet()) {
                                Ok(()) => (true, format!("Wrote {}", path.display())),
                                Err(e) => (false, format!("Cannot write {}: {e}", path.display())),
                            });
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
            egui::Panel::bottom("sw_plot").resizable(true).default_size(300.0).show(ui, |ui| self.overlay(ui, &pal));
            if self.is_monte_carlo() && !self.run_specs.is_empty() {
                egui::Panel::top("sw_stats").resizable(true).default_size(230.0).show(ui, |ui| self.statistics(ui, &pal));
            }
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
        self.monte_carlo_ui(ui, pal);
    }

    fn monte_carlo_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        theme::section(ui, "Monte Carlo");
        let mc = &mut self.setup.monte_carlo;
        ui.horizontal(|ui| {
            let l = ui.label("Runs");
            ui.add(egui::DragValue::new(&mut mc.runs).range(0..=1000)).labelled_by(l.id);
            let l = ui.label("Seed");
            ui.add(egui::DragValue::new(&mut mc.seed)).labelled_by(l.id);
        });
        ui.label(
            RichText::new(
                "0 runs = off. Targets: INST.param with wildcards (M*.w, R?.r) or a design variable. Sigma: 5% or a value.",
            )
            .small()
            .color(pal.text_weak),
        );
        let mut remove = None;
        for (i, v) in mc.vars.iter_mut().enumerate() {
            egui::Frame::new().fill(pal.bg_widget).corner_radius(6).inner_margin(6).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new(("sw_mc", i)).num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
                    let l = ui.label("Target");
                    ui.add_sized([140.0, 20.0], egui::TextEdit::singleline(&mut v.target).hint_text("R*.r")).labelled_by(l.id);
                    let l = ui.label("Sigma");
                    ui.add_sized([90.0, 20.0], egui::TextEdit::singleline(&mut v.sigma).hint_text("5%")).labelled_by(l.id);
                    ui.end_row();
                    let l = ui.label("Nominal");
                    ui.add_enabled(
                        !v.target.contains('.'),
                        egui::TextEdit::singleline(&mut v.nominal).desired_width(140.0).hint_text("design variables only"),
                    )
                    .labelled_by(l.id);
                    let l = ui.label("Dist");
                    egui::ComboBox::from_id_salt(("sw_mc_dist", i))
                        .selected_text(if v.dist == Dist::Gauss { "Gauss" } else { "Uniform" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut v.dist, Dist::Gauss, "Gauss");
                            ui.selectable_value(&mut v.dist, Dist::Uniform, "Uniform");
                        })
                        .response
                        .labelled_by(l.id);
                    ui.end_row();
                    ui.checkbox(&mut v.mismatch, "Mismatch")
                        .on_hover_text("Independent draw per instance (else shared: process)");
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                });
            });
            ui.add_space(4.0);
        }
        if let Some(i) = remove {
            mc.vars.remove(i);
        }
        if ui.button("+ Varied parameter").clicked() {
            mc.vars.push(McVar { sigma: "5%".into(), ..Default::default() });
            if mc.runs == 0 {
                mc.runs = 50;
            }
        }
    }

    pub fn is_monte_carlo(&self) -> bool {
        self.results.iter().flatten().any(|r| r.point.run.is_some())
    }

    /// Values of spec `k` over the finished points.
    pub fn spec_values(&self, k: usize) -> Vec<f64> {
        self.results.iter().flatten().filter_map(|r| r.specs.get(k)?.as_ref().ok().copied()).collect()
    }

    /// (passing, checked) points of spec `k`; failed evaluations count as failing.
    pub fn spec_yield(&self, k: usize) -> (usize, usize) {
        let Some(s) = self.run_specs.get(k) else { return (0, 0) };
        let mut out = (0, 0);
        for r in self.results.iter().flatten() {
            let v = match r.specs.get(k) {
                Some(Ok(x)) => s.check(*x),
                Some(Err(_)) => Some(false),
                None => None,
            };
            if let Some(ok) = v {
                out.1 += 1;
                out.0 += usize::from(ok);
            }
        }
        out
    }

    fn statistics(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            theme::section(ui, "Statistics");
            egui::Grid::new("sw_stats_grid").striped(true).num_columns(7).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["SPEC", "MEAN", "σ", "MIN", "MAX", "N", "YIELD"] {
                    ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                }
                ui.end_row();
                for k in 0..self.run_specs.len() {
                    let st = stats(&self.spec_values(k));
                    if ui.selectable_label(self.hist_spec == k, &self.run_specs[k].name).clicked() {
                        self.hist_spec = k;
                    }
                    for v in [st.mean, st.std, st.min, st.max] {
                        ui.label(RichText::new(format_eng(v, "")).monospace());
                    }
                    ui.label(st.n.to_string());
                    let (pass, n) = self.spec_yield(k);
                    if n > 0 {
                        let y = pass as f64 / n as f64 * 100.0;
                        ui.label(RichText::new(format!("{y:.1}%")).strong().color(if pass == n { pal.ok } else { pal.warning }));
                    } else {
                        ui.label("—");
                    }
                    ui.end_row();
                }
            });
            let ui = &mut cols[1];
            let k = self.hist_spec.min(self.run_specs.len() - 1);
            let spec = &self.run_specs[k];
            theme::section(ui, &format!("Histogram · {}", spec.name));
            let vals = self.spec_values(k);
            let st = stats(&vals);
            let bins = 24usize;
            let width = if st.max > st.min { (st.max - st.min) / bins as f64 } else { st.mean.abs().max(1e-30) * 0.01 };
            let mut counts = vec![0u32; bins];
            for v in &vals {
                let i = (((v - st.min) / width) as usize).min(bins - 1);
                counts[i] += 1;
            }
            let bars: Vec<Bar> = counts
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let x = st.min + (i as f64 + 0.5) * width;
                    let fill = match spec.check(x) {
                        Some(false) => pal.danger,
                        _ => pal.simulation,
                    };
                    Bar::new(x, *c as f64).width(width * 0.9).fill(fill)
                })
                .collect();
            let limits: Vec<f64> = [&spec.min, &spec.max].iter().filter_map(|v| ics_core::units::parse_spice(v)).collect();
            Plot::new("sw_hist").x_axis_formatter(|m, _| format_eng(m.value, "")).allow_scroll(false).show(ui, |pu| {
                pu.bar_chart(BarChart::new(spec.name.clone(), bars));
                for l in limits {
                    pu.vline(VLine::new("limit", l).color(pal.danger).width(1.5));
                }
            });
        });
    }

    /// An HTML datasheet of the last run: setup, spec summary and every point.
    pub fn datasheet(&self) -> String {
        let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let cell = self.cell.as_ref().map_or(String::new(), |c| c.to_string());
        let mut h = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{0} datasheet</title><style>\
             body{{font-family:system-ui,sans-serif;margin:32px;color:#1d222b}}table{{border-collapse:collapse;margin:12px 0}}\
             td,th{{border:1px solid #d3d7de;padding:4px 10px;text-align:right}}th{{background:#e9ebef}}td.l,th.l{{text-align:left}}\
             .pass{{color:#0f7442}}.fail{{color:#c02a2a;font-weight:600}}</style></head><body><h1>{0}</h1>",
            esc(&cell)
        );
        let (pass, checked) = self.pass_count();
        h += &format!("<p>{} point(s); {pass} of {checked} meet every spec.</p>", self.results.iter().flatten().count());
        h += "<h2>Specs</h2><table><tr><th class=l>Spec</th><th class=l>Expression</th><th>Min limit</th><th>Max limit</th>\
              <th>Min</th><th>Mean</th><th>Max</th><th>σ</th><th>Yield</th></tr>";
        for (k, s) in self.run_specs.iter().enumerate() {
            let st = stats(&self.spec_values(k));
            let (p, n) = self.spec_yield(k);
            let y = if n > 0 { format!("{:.1}%", p as f64 / n as f64 * 100.0) } else { "—".into() };
            let e = |v: f64| format_eng(v, "");
            h += &format!(
                "<tr><td class=l>{}</td><td class=l><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{y}</td></tr>",
                esc(&s.name),
                esc(&s.expr),
                esc(&s.min),
                esc(&s.max),
                e(st.min),
                e(st.mean),
                e(st.max),
                e(st.std)
            );
        }
        h += "</table><h2>Points</h2><table><tr><th>#</th><th class=l>Point</th><th class=l>Status</th>";
        for s in &self.run_specs {
            h += &format!("<th>{}</th>", esc(&s.name));
        }
        h += "</tr>";
        for (i, r) in self.results.iter().enumerate() {
            let Some(r) = r else { continue };
            let status = match &r.results {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("failed: {}", esc(e)),
            };
            h += &format!("<tr><td>{}</td><td class=l>{}</td><td class=l>{status}</td>", i + 1, esc(&r.point.label()));
            for (s, v) in self.run_specs.iter().zip(&r.specs) {
                h += &match v {
                    Ok(x) => {
                        let class = match s.check(*x) {
                            Some(true) => " class=pass",
                            Some(false) => " class=fail",
                            None => "",
                        };
                        format!("<td{class}>{}</td>", format_eng(*x, ""))
                    }
                    Err(e) => format!("<td class=fail title=\"{}\">error</td>", esc(e)),
                };
            }
            h += "</tr>";
        }
        h += "</table></body></html>\n";
        h
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
