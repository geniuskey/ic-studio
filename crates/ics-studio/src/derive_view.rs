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
    /// Create the copy and start sizing it: (reference, plan, build the layout, keep the
    /// supply current near the reference's).
    Start(CellRef, Plan, bool, bool),
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
    /// The reference schematic and technology, to suggest the knobs again when the targets change.
    sch: ics_core::Schematic,
    tech: Option<ics_core::Tech>,
    /// The targets the knobs were suggested for.
    knobs_for: Vec<String>,
    /// Derivations finished in this session.
    pub derived: usize,
    has_layout: bool,
    layout: bool,
    keep_power: bool,
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
            knobs_for: c.sim.sweep.specs.iter().map(|s| s.expr.clone()).collect(),
            sch: sch.clone(),
            tech: Some(p.tech.clone()),
            derived: self.derived,
            has_layout: c.layout.is_some(),
            layout: c.layout.is_some(),
            keep_power: true,
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

    /// Suggest the knobs again for the enabled targets (a current target frees mirror ratios),
    /// keeping the user's choices for knobs that stay.
    fn resuggest(&mut self) {
        let specs: Vec<Spec> = self.targets.iter().filter(|t| t.on).map(|t| t.spec.clone()).collect();
        let key: Vec<String> = specs.iter().map(|s| s.expr.clone()).collect();
        if key == self.knobs_for {
            return;
        }
        let Some(tech) = &self.tech else { return };
        // A knob stays off when one the user turned off changed the same parameter of one of its
        // devices (groups split or merge when mirror ratios are freed or tied).
        let off: Vec<(&'static str, Vec<String>)> =
            self.knobs.iter().filter(|k| !k.on).map(|k| (k.s.param, k.s.devices.clone())).collect();
        self.knobs = ics_core::derive::suggest(&self.sch, tech, &specs)
            .into_iter()
            .map(|s| {
                let on = !off.iter().any(|(p, ds)| *p == s.param && ds.iter().any(|d| s.devices.contains(d)));
                Knob { on, s }
            })
            .collect();
        self.knobs_for = key;
    }

    pub fn finished(&mut self, cell: CellRef, res: Result<Outcome, String>) {
        match res {
            Ok(o) => {
                self.derived += 1;
                self.outcome = Some((cell, o));
            }
            Err(e) => self.error = e,
        }
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<DeriveAction> {
        let mut open = self.open;
        let mut action = None;
        let title = t("Derive New Design");
        crate::float::tool_window(ctx, "derive", title, [560.0, 640.0], &mut open, |ui| {
            if let Some(from) = self.from.clone().filter(|_| !self.is_running() && self.outcome.is_none()) {
                ui.horizontal_wrapped(|ui| {
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
                        action = Some(DeriveAction::Start(from, plan, self.layout, self.keep_power));
                    }
                    if !self.error.is_empty() {
                        ui.colored_label(theme::palette(ui.ctx()).danger, crate::i18n::t_text(&self.error));
                    }
                });
                ui.separator();
            }
            egui::ScrollArea::vertical().show(ui, |ui| self.ui(ui));
        });
        self.open &= open;
        action
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let Some(from) = self.from.clone() else { return };
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
            return;
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
                    ui.add(egui::TextEdit::singleline(v).min_size(egui::vec2(88.0, 0.0)).desired_width(88.0))
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
        self.resuggest();
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
        let has_idd = self.targets.iter().any(|tg| tg.on && tg.spec.name == "idd");
        ui.add_enabled(!has_idd, egui::Checkbox::new(&mut self.keep_power, t("Keep the supply current near the original (0.5x to 2x)")))
            .on_hover_text(t(
                "Without it the optimizer may meet a target by starving the circuit of current (slow) or flooding it (power). An idd target replaces it.",
            ));
        ui.horizontal(|ui| {
            let l = ui.label(t("Simulations at most"));
            ui.add(egui::DragValue::new(&mut self.max_evals).range(10..=1000)).labelled_by(l.id);
        });
    }

    pub fn set_error(&mut self, e: String) {
        self.error = e;
    }

    fn result_ui(&mut self, ui: &mut egui::Ui, from: &CellRef, cell: CellRef, o: Outcome) {
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
        if !o.health.is_empty() {
            ui.add_space(8.0);
            theme::section(ui, t("What else changed"));
            for f in &o.health {
                let (mark, color) = if f.warn { ("!", pal.warning) } else { ("i", pal.text_weak) };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(mark).strong().color(color));
                    ui.add(egui::Label::new(RichText::new(crate::i18n::t_text(&f.text)).color(color)).wrap());
                });
            }
        }
        if !o.passed {
            ui.add_space(8.0);
            ui.label(
                RichText::new(t(
                    "Try a larger simulation budget, let more parameters change, or relax a target. The Verification \
                     window explains each failing spec.",
                ))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_follow_the_targets() {
        let p = ics_core::examples::demo_project();
        let mut w = DeriveWizard::default();
        w.show(&p, &CellRef::new("work", "current_mirror"));
        let ratio = |w: &DeriveWizard| w.knobs.iter().any(|k| k.s.role == ics_core::derive::Role::MirrorOutput);
        assert!(ratio(&w), "the iout spec frees the mirror ratio");
        w.knobs.iter_mut().filter(|k| k.s.param == "l").for_each(|k| k.on = false);
        w.targets[0].on = false;
        w.resuggest();
        assert!(!ratio(&w), "without a current target the mirror stays matched");
        assert!(w.knobs.iter().filter(|k| k.s.param == "l").all(|k| !k.on), "choices are kept");
    }
}
