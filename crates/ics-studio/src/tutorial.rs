//! Interactive tutorial for first-time users: a guide window whose steps complete on their
//! own as the user does them in the real app (open a schematic, simulate, run DRC and LVS).

use eframe::egui::{self, Align2, RichText, Sense, Stroke, Vec2};
use ics_core::CellRef;

use crate::app::{Command, Workspace};
use crate::theme;

/// What the user has done so far, sampled from the app every frame.
#[derive(Default)]
pub struct Progress {
    /// The demo inverter (schematic and layout), when the open project has it.
    pub inverter: Option<CellRef>,
    pub workspace: Option<Workspace>,
    /// The inverter is the active cell.
    pub on_inverter: bool,
    pub selected: bool,
    pub simulated: bool,
    pub calculator: bool,
    pub drc: bool,
    pub lvs: bool,
    pub palette: bool,
}

impl Progress {
    fn at(&self, w: Workspace) -> bool {
        self.on_inverter && self.workspace == Some(w)
    }
}

struct Step {
    title: &'static str,
    body: &'static str,
    hint: &'static str,
    spot: Option<Workspace>,
    done: fn(&Progress) -> bool,
    show: fn(&CellRef) -> Vec<Command>,
}

const STEPS: &[Step] = &[
    Step {
        title: "Open the demo project",
        body: "IC Studio ships with example designs: inverters, logic gates, op-amps, SRAM. \
               Open them to have a real circuit to explore.",
        hint: "Home › Open demo project, or File › New Demo Project",
        spot: Some(Workspace::Home),
        done: |p| p.inverter.is_some(),
        show: |_| vec![],
    },
    Step {
        title: "Open a schematic",
        body: "Cells are listed in the Explorer on the left. Open the schematic of cmos_inverter: \
               one PMOS and one NMOS transistor between vdd and ground.",
        hint: "Explorer › work › cmos_inverter › Schematic",
        spot: Some(Workspace::Schematic),
        done: |p| p.at(Workspace::Schematic),
        show: |r| vec![Command::OpenCell(r.clone(), Some(Workspace::Schematic))],
    },
    Step {
        title: "Select a device",
        body: "Click a transistor on the canvas. Its parameters (W, L, model) appear in the panel \
               on the right, where you can edit them. Scroll to pan, Ctrl+wheel to zoom, F to fit.",
        hint: "Click a transistor",
        spot: None,
        done: |p| p.on_inverter && p.selected,
        show: |r| vec![Command::OpenCell(r.clone(), Some(Workspace::Schematic))],
    },
    Step {
        title: "Run a simulation",
        body: "The schematic is netlisted and simulated with the built-in SPICE engine, or with \
               ngspice when it is installed. The analyses are set up in the cell's simulation settings.",
        hint: "F5 or ⌘↩, or Simulation › Run",
        spot: Some(Workspace::Simulation),
        done: |p| p.on_inverter && p.simulated,
        show: |r| vec![Command::OpenCell(r.clone(), None), Command::RunSimulation],
    },
    Step {
        title: "Measure a waveform",
        body: "Results open in the Simulation workspace. Click signal names to show or hide them, \
               then open the Waveform Calculator to measure delays, rise times or averages.",
        hint: "Simulation › Waveform Calculator…",
        spot: Some(Workspace::Simulation),
        done: |p| p.calculator,
        show: |r| vec![Command::OpenCell(r.clone(), Some(Workspace::Simulation)), Command::Calculator],
    },
    Step {
        title: "Open the layout",
        body: "The same cell has a layout: the shapes that go on silicon, drawn on the layers of \
               the generic 180 nm technology. Layout coordinates are in nanometres.",
        hint: "⌘4, or the Layout icon on the left",
        spot: Some(Workspace::Layout),
        done: |p| p.at(Workspace::Layout),
        show: |r| vec![Command::OpenCell(r.clone(), Some(Workspace::Layout))],
    },
    Step {
        title: "Check the design rules",
        body: "DRC finds shapes that are too narrow or too close together. Violations are listed \
               in the Verify panel and marked on the canvas.",
        hint: "Verify › Run DRC",
        spot: Some(Workspace::Layout),
        done: |p| p.on_inverter && p.drc,
        show: |r| vec![Command::OpenCell(r.clone(), Some(Workspace::Layout)), Command::RunDrc],
    },
    Step {
        title: "Compare layout and schematic",
        body: "LVS extracts the devices and nets from the layout and compares them with the \
               schematic. Here it reports C1: the 20 fF load is a test load that exists only in \
               the schematic, so the layout has no counterpart. Other differences would mean a wiring error.",
        hint: "Verify › Run LVS",
        spot: Some(Workspace::Layout),
        done: |p| p.on_inverter && p.lvs,
        show: |r| vec![Command::OpenCell(r.clone(), Some(Workspace::Layout)), Command::RunLvs],
    },
    Step {
        title: "Find any command",
        body: "Every action is in the command palette. Type a few letters, for example \
               \"export gds\", and press Enter.",
        hint: "⌘K, or the search box at the top right",
        spot: None,
        done: |p| p.palette,
        show: |_| vec![Command::CommandPalette],
    },
];

#[derive(Default)]
pub struct Tutorial {
    pub open: bool,
    /// Current step; `STEPS.len()` once finished.
    pub step: usize,
    done: Vec<bool>,
}

impl Tutorial {
    pub fn start(&mut self) {
        self.open = true;
        self.step = 0;
        self.done = vec![false; STEPS.len()];
    }

    pub fn finished(&self) -> bool {
        self.step >= STEPS.len()
    }

    pub fn completed(&self) -> usize {
        self.done.iter().filter(|d| **d).count()
    }

    /// The workspace button the current step points at.
    pub fn spotlight(&self) -> Option<Workspace> {
        self.open.then(|| STEPS.get(self.step)?.spot).flatten()
    }

    /// Completes the current step once the user has done it, and moves on.
    pub fn track(&mut self, p: &Progress) {
        while self.open && !self.finished() && !self.done[self.step] && (STEPS[self.step].done)(p) {
            self.done[self.step] = true;
            self.step += 1;
        }
    }

    /// The guide window; returns the commands its buttons ran.
    pub fn window(&mut self, ctx: &egui::Context, p: &Progress) -> Vec<Command> {
        let mut q = vec![];
        if !self.open {
            return q;
        }
        let mut open = true;
        egui::Window::new("Tutorial")
            .id(egui::Id::unique("tutorial"))
            .open(&mut open)
            .collapsible(true)
            .resizable(false)
            .pivot(Align2::RIGHT_BOTTOM)
            .default_pos(ctx.content_rect().right_bottom() + Vec2::new(-24.0, -220.0))
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                self.contents(ui, p, &mut q);
            });
        self.open &= open;
        q
    }

    fn contents(&mut self, ui: &mut egui::Ui, p: &Progress, q: &mut Vec<Command>) {
        let pal = theme::palette(ui.ctx());
        let n = STEPS.len();
        let frac = self.completed() as f32 / n as f32;
        let bar = if self.finished() { "Complete".to_string() } else { format!("Step {} of {n}", self.step + 1) };
        ui.add(egui::ProgressBar::new(frac).desired_height(6.0)).on_hover_text(format!("{} of {n} done", self.completed()));
        ui.label(RichText::new(bar).small().color(pal.text_weak));
        ui.add_space(4.0);

        let Some(step) = STEPS.get(self.step) else {
            ui.label(RichText::new("You're ready to design").size(16.0).strong());
            ui.label(
                RichText::new(
                    "That's the whole flow: schematic, simulation, layout and verification. \
                     Next, create your own cell with File › New Cell…, or browse the other demo designs.",
                )
                .color(pal.text_weak),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Keyboard shortcuts").clicked() {
                    q.push(Command::Shortcuts);
                }
                if ui.button("User guide").clicked() {
                    q.push(Command::UserGuide);
                }
                if ui.button("Start over").clicked() {
                    self.start();
                }
                if ui.button("Close").clicked() {
                    self.open = false;
                }
            });
            return;
        };

        ui.label(RichText::new(step.title).size(16.0).strong());
        ui.label(RichText::new(step.body).color(pal.text_weak));
        ui.add_space(6.0);
        egui::Frame::new().fill(pal.bg_widget).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Try").strong().color(pal.accent));
                ui.label(theme::kbd(step.hint));
            });
        });
        ui.add_space(4.0);
        let done = self.done[self.step];
        ui.horizontal(|ui| {
            if done {
                ui.label(RichText::new("Done").color(pal.ok));
            } else {
                ui.add(egui::Spinner::new().size(12.0).color(pal.text_weak));
                ui.label(RichText::new("Waiting for you to do it").small().color(pal.text_weak));
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.step > 0, egui::Button::new("Back")).clicked() {
                self.step -= 1;
            }
            let shown = match &p.inverter {
                Some(r) => (step.show)(r),
                None if self.step == 0 => vec![Command::NewDemoProject],
                None => vec![],
            };
            if ui
                .add_enabled(!shown.is_empty() && !done, egui::Button::new("Show me"))
                .on_hover_text("Do this step for me")
                .on_disabled_hover_text("Do this step yourself, or skip it")
                .clicked()
            {
                q.extend(shown);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let next = if done { "Next" } else { "Skip" };
                if ui.button(next).clicked() {
                    self.step += 1;
                }
            });
        });
        if p.inverter.is_none() && self.step > 0 {
            ui.label(RichText::new("These steps use the demo project's cmos_inverter cell.").small().color(pal.warning));
        }

        ui.add_space(4.0);
        egui::CollapsingHeader::new("All steps").id_salt("tutorial_steps").show(ui, |ui| {
            for (i, s) in STEPS.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, painter) = ui.allocate_painter(Vec2::splat(12.0), Sense::hover());
                    let c = r.rect.center();
                    if self.done[i] {
                        painter.circle_filled(c, 5.0, pal.ok);
                    } else {
                        let col = if i == self.step { pal.accent } else { pal.border };
                        painter.circle_stroke(c, 4.5, Stroke::new(1.5, col));
                    }
                    let text = RichText::new(format!("{}. {}", i + 1, s.title));
                    let text = if i == self.step { text.strong() } else { text.color(pal.text_weak) };
                    if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() {
                        self.step = i;
                    }
                });
            }
        });
    }
}
