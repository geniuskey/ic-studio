//! Derive New Design: the guided way to start a new design from a finished one. Pick the
//! targets (specs) the new design must meet and the parameters IC Studio may change (suggested
//! from the circuit, matched devices kept equal), then one click copies the cell, sizes it with
//! the optimizer, builds its layout like the original and checks it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, RichText};
use ics_core::derive::{Plan, Suggestion};
use ics_core::sweep::Spec;
use ics_core::{CellRef, Project};
use ics_sim::SimOptions;
use ics_sim::derive::Outcome;
use ics_sim::optimize::{OptEval, OptReport};

use crate::i18n::{t, tf};
use crate::theme;

enum Msg {
    Eval(usize),
    Done(Result<OptReport, String>),
}

struct Target {
    on: bool,
    spec: Spec,
    /// The reference's limits, shown next to the new ones.
    was: String,
}

struct Knob {
    on: bool,
    s: Suggestion,
}

/// `W of M1, M2 (input pair)` in the UI language.
fn knob_title(s: &Suggestion) -> String {
    tf("{} of {} ({})", &[&s.param.to_ascii_uppercase(), &s.devices.join(", "), t(s.role.label())])
}

pub enum DeriveAction {
    /// Create the copy and start sizing it.
    Start(CellRef, Plan, bool),
}

#[derive(Default)]
pub struct DeriveWizard {
    pub open: bool,
    from: Option<CellRef>,
    lib: String,
    name: String,
    targets: Vec<Target>,
    presets: Vec<Spec>,
    knobs: Vec<Knob>,
    has_layout: bool,
    layout: bool,
    max_evals: usize,
    error: String,
    /// The copy being sized.
    pub target: Option<CellRef>,
    rx: Option<Receiver<Msg>>,
    stop: Arc<AtomicBool>,
    evals: usize,
    outcome: Option<(CellRef, Outcome)>,
}

impl DeriveWizard {
    /// Open the wizard on `from`.
    pub fn show(&mut self, p: &Project, from: &CellRef) {
        let Some(c) = p.cell(from) else { return };
        if self.is_running() {
            self.open = true;
            return;
        }
        let lib = if from.lib == ics_core::pcell::PCELL_LIB { "work".to_string() } else { from.lib.clone() };
        let sch = c.schematic.clone().unwrap_or_default();
        *self = DeriveWizard {
            open: true,
            from: Some(from.clone()),
            name: (2..)
                .map(|k| format!("{}_v{k}", from.cell))
                .find(|n| p.cell(&CellRef::new(&lib, n)).is_none())
                .unwrap_or_default(),
            lib,
            targets: c
                .sim
                .sweep
                .specs
                .iter()
                .map(|s| Target { on: true, spec: s.clone(), was: crate::guide_view::spec_limits(s) })
                .collect(),
            presets: ics_core::derive::spec_presets(&sch, &c.sim, &c.sim.sweep.specs),
            knobs: ics_core::derive::suggest(&sch, &p.tech, &c.sim.sweep.specs)
                .into_iter()
                .map(|s| Knob { on: true, s })
                .collect(),
            has_layout: c.layout.is_some(),
            layout: c.layout.is_some(),
            max_evals: 80,
            ..Default::default()
        };
    }

    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Size the new cell `target` (already created in `project`) on a background thread.
    pub fn start(&mut self, ctx: &egui::Context, project: &Project, target: CellRef, opts: SimOptions) {
        let p = project.clone();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = stop.clone();
        self.rx = Some(rx);
        self.evals = 0;
        self.outcome = None;
        self.target = Some(target.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut progress = |_: &OptEval, n: usize| {
                let _ = tx.send(Msg::Eval(n));
                ctx.request_repaint();
                !stop.load(Ordering::Relaxed)
            };
            let res = ics_sim::optimize::run(&p, &target, &opts, &mut progress);
            let _ = tx.send(Msg::Done(res));
            ctx.request_repaint();
        });
    }

    /// The finished sizing run, once: (reference, new cell, report, build the layout).
    pub fn poll(&mut self) -> Option<(CellRef, CellRef, Result<OptReport, String>, bool)> {
        let rx = self.rx.as_ref()?;
        let mut done = None;
        for m in rx.try_iter() {
            match m {
                Msg::Eval(n) => self.evals = n,
                Msg::Done(r) => done = Some(r),
            }
        }
        let res = done?;
        self.rx = None;
        Some((self.from.clone()?, self.target.clone()?, res, self.layout))
    }

    pub fn finished(&mut self, cell: CellRef, res: Result<Outcome, String>) {
        match res {
            Ok(o) => self.outcome = Some((cell, o)),
            Err(e) => self.error = e,
        }
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<DeriveAction> {
        let mut open = self.open;
        let mut action = None;
        let title = t("Derive New Design");
        crate::float::tool_window(ctx, "derive", title, [560.0, 640.0], &mut open, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| action = self.ui(ui));
        });
        self.open = open;
        action
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<DeriveAction> {
        let pal = theme::palette(ui.ctx());
        let from = self.from.clone()?;
        if let Some((cell, o)) = &self.outcome {
            return self.result_ui(ui, &from, cell.clone(), o.clone());
        }
        if self.is_running() {
            ui.heading(tf("Sizing {}…", &[&self.target.as_ref().map_or(String::new(), |c| c.to_string())]));
            ui.add(
                egui::ProgressBar::new(self.evals as f32 / self.max_evals.max(1) as f32)
                    .text(tf("simulation {} of up to {}", &[&self.evals.to_string(), &self.max_evals.to_string()])),
            );
            ui.label(
                RichText::new(t("The optimizer tries sizes and simulates each, keeping the matched devices equal."))
                    .color(pal.text_weak),
            );
            if ui.button(t("Stop")).clicked() {
                self.stop.store(true, Ordering::Relaxed);
            }
            return None;
        }
        ui.label(
            RichText::new(tf(
                "Start a new design from {}. The original stays as it is; the copy gets your targets, is sized to \
                 meet them and laid out like the original.",
                &[&from.to_string()],
            ))
            .color(pal.text_weak),
        );
        ui.add_space(8.0);
        theme::section(ui, t("1. Name"));
        ui.horizontal(|ui| {
            let l = ui.label(t("New cell"));
            ui.text_edit_singleline(&mut self.name).labelled_by(l.id);
            ui.label(RichText::new(tf("in {}", &[&self.lib])).color(pal.text_weak));
        });
        ui.add_space(8.0);
        theme::section(ui, t("2. Targets"));
        ui.label(
            RichText::new(t("What the new design must achieve. Edit the limits (SPICE numbers: 40, 100meg, 50p); leave both empty to only measure."))
                .small()
                .color(pal.text_weak),
        );
        egui::Grid::new("derive_targets").num_columns(5).spacing([8.0, 4.0]).show(ui, |ui| {
            for h in ["", "Spec", "Min", "Max", "Original"] {
                ui.label(RichText::new(t(h)).strong());
            }
            ui.end_row();
            for (k, tg) in self.targets.iter_mut().enumerate() {
                ui.checkbox(&mut tg.on, "").on_hover_text(t("Use this spec"));
                ui.label(&tg.spec.name).on_hover_text(tf("{} on the {} analysis", &[&tg.spec.expr, &tg.spec.analysis]));
                let name = tg.spec.name.clone();
                for (v, what) in [(&mut tg.spec.min, "minimum"), (&mut tg.spec.max, "maximum")] {
                    ui.add(egui::TextEdit::singleline(v).desired_width(80.0))
                        .on_hover_text(format!("{name} {}", t(what)))
                        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, format!("{name} {what} {k}")));
                }
                ui.label(RichText::new(t(&tg.was)).color(pal.text_weak));
                ui.end_row();
            }
        });
        if self.targets.iter().all(|t| !t.on || crate::guide_view::spec_limits(&t.spec) == t.was) {
            ui.label(
                RichText::new(t("These are the original's targets: the copy would only be re-sized for more margin. Change a limit to design for a new target."))
                    .small()
                    .color(pal.warning),
            );
        }
        if !self.presets.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("Add:")).small());
                let mut add = None;
                for (k, s) in self.presets.iter().enumerate() {
                    if ui.small_button(format!("+ {}", s.name)).on_hover_text(&s.expr).clicked() {
                        add = Some(k);
                    }
                }
                if let Some(k) = add {
                    let s = self.presets.remove(k);
                    self.targets.push(Target { on: true, spec: s, was: "new".into() });
                }
            });
        }
        ui.add_space(8.0);
        theme::section(ui, t("3. What IC Studio may change"));
        ui.label(
            RichText::new(t("Suggested from the circuit. Matched transistors share one value so they stay equal."))
                .small()
                .color(pal.text_weak),
        );
        for k in &mut self.knobs {
            ui.horizontal(|ui| {
                ui.checkbox(&mut k.on, RichText::new(knob_title(&k.s)).strong());
                ui.label(RichText::new(format!("{} .. {}", k.s.var.min, k.s.var.max)).monospace().small());
            });
            ui.indent(("why", &k.s.var.target), |ui| {
                ui.add(egui::Label::new(RichText::new(t(k.s.why)).small().color(pal.text_weak)).wrap());
            });
        }
        if self.knobs.is_empty() {
            ui.label(RichText::new(t("No transistor sizes found to change.")).color(pal.warning));
        }
        ui.add_space(8.0);
        theme::section(ui, t("4. Options"));
        ui.add_enabled(self.has_layout, egui::Checkbox::new(&mut self.layout, t("Build the layout like the original")))
            .on_hover_text(t(
                "Place like the original (or generate from the schematic if the new sizes do not fit), route, fix DRC, run LVS",
            ));
        ui.horizontal(|ui| {
            let l = ui.label(t("Simulations at most"));
            ui.add(egui::DragValue::new(&mut self.max_evals).range(10..=1000)).labelled_by(l.id);
        });
        if !self.error.is_empty() {
            ui.colored_label(pal.danger, &self.error);
        }
        ui.add_space(8.0);
        let ready = self.knobs.iter().any(|k| k.on);
        if ui.add_enabled(ready, egui::Button::new(RichText::new(t("Create and size")).strong())).clicked() {
            let plan = Plan {
                lib: self.lib.clone(),
                name: self.name.trim().to_string(),
                specs: self.targets.iter().filter(|t| t.on).map(|t| t.spec.clone()).collect(),
                vars: self.knobs.iter().filter(|k| k.on).map(|k| k.s.var.clone()).collect(),
                max_evals: self.max_evals,
            };
            self.error.clear();
            return Some(DeriveAction::Start(from, plan, self.layout));
        }
        None
    }

    pub fn set_error(&mut self, e: String) {
        self.error = e;
    }

    fn result_ui(&mut self, ui: &mut egui::Ui, from: &CellRef, cell: CellRef, o: Outcome) -> Option<DeriveAction> {
        let pal = theme::palette(ui.ctx());
        let (text, color) = if o.passed {
            (tf("{} meets every target.", &[&cell.to_string()]), pal.ok)
        } else {
            (tf("{} does not meet every target yet.", &[&cell.to_string()]), pal.warning)
        };
        ui.label(RichText::new(text).strong().size(16.0).color(color));
        ui.label(
            RichText::new(tf("{} simulation(s); the original {} is unchanged.", &[&o.evals.to_string(), &from.to_string()]))
                .color(pal.text_weak),
        );
        ui.add_space(8.0);
        let fmt = |r: Option<&Result<f64, String>>| match r {
            Some(Ok(v)) => ics_core::units::format_eng(*v, ""),
            Some(Err(_)) => "error".into(),
            None => "-".into(),
        };
        egui::Grid::new("derive_result").num_columns(4).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
            for h in ["Spec", "Target", "Original sizes", "New sizes"] {
                ui.label(RichText::new(t(h)).strong());
            }
            ui.end_row();
            for (k, (name, limits)) in o.specs.iter().enumerate() {
                ui.label(name);
                ui.label(limits);
                ui.label(fmt(o.before.get(k)));
                ui.label(RichText::new(fmt(o.after.get(k))).strong());
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        theme::section(ui, t("New sizes"));
        for (t, v) in &o.values {
            ui.label(RichText::new(format!("{t} = {}", ics_core::units::format_eng(*v, ""))).monospace());
        }
        if !o.layout.is_empty() {
            ui.add_space(8.0);
            theme::section(ui, t("Layout"));
            for l in &o.layout {
                ui.label(crate::i18n::t_text(l));
            }
        }
        if !o.passed {
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Try a larger simulation budget, let more parameters change, or relax a target. The Verification \
                     window explains each failing spec.",
                )
                .color(pal.text_weak),
            );
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(t("Derive another")).clicked() {
                self.outcome = None;
            }
            if ui.button(t("Close")).clicked() {
                self.open = false;
            }
        });
        None
    }
}
