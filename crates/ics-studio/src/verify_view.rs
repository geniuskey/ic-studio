//! Verification window (ADE Assembler style): the project's tests (cells whose corner / sweep
//! / Monte Carlo setup is run together), a summary of every spec over its test's points, and
//! a comparison with an earlier run from the project's history.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText};
use ics_core::verify::{self, Change, Status, TestDef, VerifyPlan, VerifyRun};
use ics_core::{CellRef, Project};
use ics_sim::SimOptions;

use crate::i18n;
use crate::theme::{self, Palette};

pub enum VerifyAction {
    /// The plan changed; store it in the project.
    Plan(VerifyPlan),
    Run,
}

/// (test, points done, points total) of the running test.
type Progress = Arc<Mutex<(String, usize, usize)>>;

#[derive(Default)]
pub struct Verification {
    pub open: bool,
    /// The last run.
    pub run: Option<VerifyRun>,
    /// The saved run compared with, and its file name.
    pub before: Option<(String, VerifyRun)>,
    history: Vec<PathBuf>,
    history_root: Option<PathBuf>,
    rx: Option<Receiver<VerifyRun>>,
    progress: Progress,
    /// Show only failures, errors and regressions.
    pub hide_passing: bool,
    /// The run compared with was picked by hand (otherwise it follows the latest saved run).
    pinned: bool,
}

impl Verification {
    pub fn show(&mut self, root: Option<&Path>) {
        self.open = true;
        self.refresh_history(root);
    }

    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    fn refresh_history(&mut self, root: Option<&Path>) {
        self.history_root = root.map(Path::to_path_buf);
        self.history = root.map(verify::list_history).unwrap_or_default();
        if self.before.is_none()
            && let Some(last) = self.history.last()
            && let Ok(r) = verify::load_run(last)
        {
            self.before = Some((file_name(last), r));
        }
    }

    /// Run `tests` of a snapshot of the project in a background thread.
    pub fn start(&mut self, ctx: &egui::Context, project: Project, tests: Vec<(String, CellRef)>, opts: SimOptions) {
        let (tx, rx) = channel();
        let progress = self.progress.clone();
        *progress.lock().unwrap() = (String::new(), 0, 0);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let report = |name: &str, done: usize, total: usize| {
                *progress.lock().unwrap() = (name.to_string(), done, total);
                ctx.request_repaint();
            };
            let run = ics_sim::verify::run(&project, &tests, &opts, &report);
            let _ = tx.send(run);
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Pick up a finished run, save it in the history and compare it with the previous one;
    /// returns (ok, console line).
    pub fn poll(&mut self) -> Option<(bool, String)> {
        let run = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        if !self.pinned
            && let Some(last) = self.history.last()
            && let Ok(r) = verify::load_run(last)
        {
            self.before = Some((file_name(last), r));
        }
        let n = run.counts();
        let mut msg = i18n::tf(
            "Verification {}: {} pass, {} fail, {} error, {} info",
            &[i18n::t(run.status().label()), &n.pass.to_string(), &n.fail.to_string(), &n.error.to_string(), &n.info.to_string()],
        );
        if let Some((name, b)) = &self.before {
            let reg = run.compare(b).iter().filter(|c| c.is_regression()).count();
            if reg > 0 {
                msg += &i18n::tf("; {} regression(s) since {}", &[&reg.to_string(), name]);
            }
        }
        if let Some(root) = self.history_root.clone() {
            match verify::save_history(&root, &run) {
                Ok(_) => self.history = verify::list_history(&root),
                Err(e) => msg += &format!(" {}", i18n::tf("(not saved: {})", &[&e.to_string()])),
            }
        }
        let ok = matches!(run.status(), Status::Pass | Status::Info);
        self.run = Some(run);
        Some((ok, msg))
    }

    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>, active: Option<&CellRef>) -> Option<VerifyAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let mut open = self.open;
        if self.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        let pal = theme::palette(ctx);
        crate::float::tool_window(ctx, "verification", i18n::t("Verification"), [760.0, 560.0], &mut open, |ui| {
            let Some(p) = project else {
                ui.label(i18n::t("Open a project to verify it."));
                return;
            };
            if let Some(a) = self.tests_ui(ui, p, active) {
                action = Some(a);
            }
            ui.separator();
            ui.horizontal(|ui| {
                let running = self.is_running();
                if ui.add_enabled(!running, egui::Button::new(i18n::t("Run all tests"))).clicked() {
                    action = Some(VerifyAction::Run);
                }
                if running {
                    ui.spinner();
                    let (t, d, n) = self.progress.lock().unwrap().clone();
                    ui.label(if t.is_empty() {
                        i18n::t("starting...").to_string()
                    } else {
                        i18n::tf("{}: {}/{} point(s)", &[&t, &d.to_string(), &n.to_string()])
                    });
                }
                ui.separator();
                let l = ui.label(i18n::t("Compare with"));
                let current = self.before.as_ref().map_or(i18n::t("(none)").to_string(), |b| b.0.clone());
                let mut pick: Option<Option<PathBuf>> = None;
                egui::ComboBox::from_id_salt("verify_before")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.before.is_none(), i18n::t("(none)")).clicked() {
                            pick = Some(None);
                        }
                        for f in self.history.iter().rev() {
                            let n = file_name(f);
                            let sel = self.before.as_ref().is_some_and(|b| b.0 == n);
                            if ui.selectable_label(sel, n).clicked() {
                                pick = Some(Some(f.clone()));
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                match pick {
                    Some(None) => {
                        self.before = None;
                        self.pinned = true;
                    }
                    Some(Some(f)) => {
                        self.before = verify::load_run(&f).ok().map(|r| (file_name(&f), r));
                        self.pinned = self.history.last() != Some(&f);
                    }
                    None => {}
                }
                ui.checkbox(&mut self.hide_passing, i18n::t("Failures only"));
            });
            ui.separator();
            match &self.run {
                Some(run) => results_ui(ui, run, self.before.as_ref().map(|b| &b.1), self.hide_passing, &pal),
                None => {
                    ui.label(
                        RichText::new(i18n::t("No run yet. Run all tests to check every spec over its corners."))
                            .color(pal.text_weak),
                    );
                }
            }
        });
        self.open = open;
        action
    }

    /// The test list; returns a changed plan.
    fn tests_ui(&mut self, ui: &mut egui::Ui, p: &Project, active: Option<&CellRef>) -> Option<VerifyAction> {
        let mut plan = p.settings.verification.clone();
        let mut changed = false;
        ui.label(RichText::new(i18n::t("Tests")).strong());
        if plan.is_empty() {
            let defaults = verify::default_tests(p);
            ui.label(
                RichText::new(i18n::tf(
                    "No plan: every cell with specs is a test ({}). Add a cell to start a plan.",
                    &[&if defaults.is_empty() {
                        i18n::t("none yet: give a cell specs in its Corner / Sweep Manager").to_string()
                    } else {
                        defaults.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
                    }],
                ))
                .small(),
            );
        }
        let mut remove = None;
        egui::Grid::new("verify_tests").num_columns(4).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            for (k, t) in plan.tests.iter_mut().enumerate() {
                let name = t.display_name();
                changed |= ui.checkbox(&mut t.enabled, name).changed();
                let cell = verify::find_cell(p, &t.cell);
                ui.label(&t.cell);
                match cell.as_ref().and_then(|r| p.cell(r)) {
                    Some(c) => {
                        let sw = &c.sim.sweep;
                        ui.label(i18n::tf(
                            "{} spec(s), {} corner(s), {} MC run(s)",
                            &[
                                &sw.specs.len().to_string(),
                                &sw.corners.len().max(1).to_string(),
                                &sw.monte_carlo.runs.to_string(),
                            ],
                        ))
                    }
                    None => ui.label(RichText::new(i18n::t("no such cell")).color(Color32::from_rgb(220, 80, 80))),
                };
                if ui.small_button(i18n::t("Remove")).clicked() {
                    remove = Some(k);
                }
                ui.end_row();
            }
        });
        if let Some(k) = remove {
            plan.tests.remove(k);
            changed = true;
        }
        if let Some(r) = active {
            let there = plan.tests.iter().any(|t| verify::find_cell(p, &t.cell).as_ref() == Some(r));
            if ui.add_enabled(!there, egui::Button::new(i18n::tf("Add {}", &[&r.to_string()]))).clicked() {
                if plan.tests.is_empty() {
                    plan.tests = verify::default_tests(p).iter().map(|(_, r)| TestDef::new(&r.to_string())).collect();
                }
                if !plan.tests.iter().any(|t| t.cell == r.to_string()) {
                    plan.tests.push(TestDef::new(&r.to_string()));
                }
                changed = true;
            }
        }
        changed.then_some(VerifyAction::Plan(plan))
    }
}

fn file_name(p: &Path) -> String {
    p.file_stem().map_or(String::new(), |n| n.to_string_lossy().to_string())
}

pub fn status_color(s: Status, pal: &Palette) -> Color32 {
    match s {
        Status::Pass => pal.ok,
        Status::Fail => pal.danger,
        Status::Error => pal.warning,
        Status::Info => pal.text_weak,
    }
}

fn results_ui(ui: &mut egui::Ui, run: &VerifyRun, before: Option<&VerifyRun>, hide_passing: bool, pal: &Palette) {
    let n = run.counts();
    ui.horizontal(|ui| {
        ui.label(RichText::new(i18n::t(run.status().label()).to_uppercase()).strong().color(status_color(run.status(), pal)));
        ui.label(i18n::tf(
            "{} pass, {} fail, {} error, {} info   ({})",
            &[&n.pass.to_string(), &n.fail.to_string(), &n.error.to_string(), &n.info.to_string(), &run.time],
        ));
    });
    let changes: Vec<Change> = before.map(|b| run.compare(b)).unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("verify_results").num_columns(8).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            for h in ["Test", "Spec", "Limits", "Worst", "Margin", "Pass", "Status", "Before"] {
                ui.label(RichText::new(i18n::t(h)).strong());
            }
            ui.end_row();
            for t in &run.tests {
                if let Some(e) = &t.error {
                    ui.label(&t.name);
                    ui.label("-");
                    ui.label("");
                    ui.label(RichText::new(e).color(pal.warning));
                    for _ in 0..4 {
                        ui.label("");
                    }
                    ui.end_row();
                    continue;
                }
                for s in &t.specs {
                    let st = s.status();
                    let ch = changes.iter().find(|c| c.test == t.name && c.spec == s.name);
                    if hide_passing && matches!(st, Status::Pass | Status::Info) && !ch.is_some_and(Change::is_regression) {
                        continue;
                    }
                    ui.label(&t.name);
                    ui.label(&s.name).on_hover_text(format!("{}: {}", s.analysis, s.expr));
                    ui.label(s.limits());
                    let at = s.worst_point.and_then(|k| t.points.get(k)).cloned().unwrap_or_default();
                    let worst = match (st, s.worst, s.range) {
                        (Status::Info, _, Some((a, b))) => format!("{} .. {}", verify::fmt(Some(a)), verify::fmt(Some(b))),
                        (_, Some(_), _) => verify::fmt(s.worst),
                        _ => "-".into(),
                    };
                    let r = ui.label(worst);
                    if !at.is_empty() {
                        r.on_hover_text(i18n::tf("at {}", &[&at]));
                    }
                    ui.label(verify::fmt(s.margin));
                    ui.label(format!("{}/{}", s.passed, s.values.len()));
                    let r = ui.label(RichText::new(i18n::t(st.label())).strong().color(status_color(st, pal)));
                    if !s.first_error.is_empty() {
                        r.on_hover_text(i18n::tf("{} point(s) not measured: {}", &[&s.errors.to_string(), &s.first_error]));
                    } else if let Some(a) = s.advice() {
                        r.on_hover_text(crate::i18n::t_text(&a));
                    }
                    match ch {
                        Some(c) if c.is_regression() => ui.label(
                            RichText::new(i18n::tf("was {}", &[i18n::t(c.before.map_or("new", |b| b.label()))]))
                                .color(pal.danger),
                        ),
                        Some(c) => ui
                            .label(match c.before {
                                None => i18n::t("new").to_string(),
                                Some(b) if b != c.after => i18n::tf("was {}", &[i18n::t(b.label())]),
                                Some(_) => i18n::tf("was {}", &[&verify::fmt(c.worst_before)]),
                            })
                            .on_hover_text(i18n::tf(
                                "worst {} -> {}",
                                &[&verify::fmt(c.worst_before), &verify::fmt(c.worst_after)],
                            )),
                        None if before.is_some() => ui.label(RichText::new(i18n::t("same")).color(pal.text_weak)),
                        None => ui.label(""),
                    };
                    ui.end_row();
                }
            }
        });
        let tips: Vec<(String, String)> = run
            .tests
            .iter()
            .flat_map(|t| t.specs.iter().map(move |s| (t, s)))
            .filter_map(|(t, s)| {
                let tip = s.advice()?;
                let at = s.worst_point.and_then(|k| t.points.get(k)).filter(|p| !p.is_empty());
                let head = match at {
                    Some(p) => crate::i18n::tf("{} / {} (worst at {})", &[&t.name, &s.name, p]),
                    None => format!("{} / {}", t.name, s.name),
                };
                Some((head, tip))
            })
            .collect();
        if !tips.is_empty() {
            ui.add_space(10.0);
            theme::section(ui, crate::i18n::t("What to try"));
            for (head, tip) in tips {
                ui.label(RichText::new(head).strong());
                ui.add(egui::Label::new(RichText::new(crate::i18n::t_text(&tip)).color(pal.text_weak)).wrap());
                ui.add_space(4.0);
            }
        }
    });
}
