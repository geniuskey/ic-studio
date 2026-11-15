//! Process Emulation window: the cross-section of the active cell's layout along a cut line,
//! built by a process flow step by step (`ics_core::process`).

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use egui_plot::{Legend, Plot, PlotImage, PlotPoint, Points};
use ics_core::process::{Cut, GENERIC180, ProcessFlow, Section, default_cut, emulate_cell};
use ics_core::{CellRef, Project};

pub enum ProcessAction {
    Load,
}

pub struct ProcessView {
    pub open: bool,
    pub cell: Option<CellRef>,
    /// Flow text (TOML).
    pub text: String,
    pub cut: Cut,
    /// Steps run (the whole flow when equal to its length).
    pub steps: usize,
    pub flow: Option<ProcessFlow>,
    pub section: Option<Section>,
    pub error: Option<String>,
    show_flow: bool,
    dirty: bool,
    texture: Option<TextureHandle>,
    /// x (µm along the cut) of the stack shown below the picture.
    probe: Option<f64>,
}

impl Default for ProcessView {
    fn default() -> Self {
        Self {
            open: false,
            cell: None,
            text: GENERIC180.to_string(),
            cut: Cut { x0: 0.0, y0: 0.0, x1: 1.0, y1: 0.0 },
            steps: usize::MAX,
            flow: None,
            section: None,
            error: None,
            show_flow: false,
            dirty: true,
            texture: None,
            probe: None,
        }
    }
}

impl ProcessView {
    pub fn show(&mut self, project: &Project, cell: &CellRef) {
        if self.cell.as_ref() != Some(cell) {
            self.cell = Some(cell.clone());
            match default_cut(project, cell) {
                Ok(c) => self.cut = c,
                Err(e) => self.error = Some(e),
            }
            self.dirty = true;
        }
        self.open = true;
    }

    /// Re-run the flow when the inputs changed.
    pub fn update(&mut self, project: &Project) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.texture = None;
        self.section = None;
        let Some(cell) = self.cell.clone() else { return };
        let flow = match ProcessFlow::from_toml(&self.text) {
            Ok(f) => f,
            Err(e) => {
                self.error = Some(format!("process flow: {e}"));
                self.flow = None;
                return;
            }
        };
        let steps = self.steps.min(flow.steps.len());
        match emulate_cell(project, &cell, &flow, &self.cut, Some(steps)) {
            Ok(s) => {
                self.section = Some(s);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.flow = Some(flow);
    }

    /// Load a flow file.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.steps = usize::MAX;
        self.dirty = true;
    }

    fn image(&self, s: &Section, flow: &ProcessFlow) -> ColorImage {
        let colors: Vec<Color32> = s
            .materials
            .iter()
            .map(|m| {
                let c = flow.color(m);
                Color32::from_rgb(c[0], c[1], c[2])
            })
            .collect();
        let mut img = ColorImage::new([s.nx, s.nz], vec![Color32::TRANSPARENT; s.nx * s.nz]);
        for iz in 0..s.nz {
            let row = s.nz - 1 - iz;
            for ix in 0..s.nx {
                let c = s.cells[iz * s.nx + ix];
                if c > 0 {
                    img.pixels[row * s.nx + ix] = colors[c as usize - 1];
                }
            }
        }
        img
    }

    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>) -> Option<ProcessAction> {
        if !self.open {
            return None;
        }
        if let Some(p) = project {
            self.update(p);
        }
        let mut action = None;
        let mut open = self.open;
        let title = match &self.cell {
            Some(c) => format!("Process Emulation: {c}"),
            None => "Process Emulation".into(),
        };
        crate::float::tool_window(ctx, "process_emulation", &title, [860.0, 760.0], &mut open, |ui| {
            let pal = crate::theme::palette(ui.ctx());
            ui.horizontal(|ui| {
                let l = ui.label("Flow");
                let name = self.flow.as_ref().map_or("—".to_string(), |f| f.name.clone());
                ui.label(RichText::new(name).strong()).labelled_by(l.id);
                if ui.button("Load…").clicked() {
                    action = Some(ProcessAction::Load);
                }
                if ui.button("Generic 180 nm").clicked() {
                    self.set_text(GENERIC180.to_string());
                }
                ui.toggle_value(&mut self.show_flow, "Edit flow");
            });
            if self.show_flow {
                let l = ui.label(RichText::new("Process flow (TOML)").small().color(pal.text_weak));
                egui::ScrollArea::vertical().id_salt("pe_flow").max_height(200.0).show(ui, |ui| {
                    let r = ui
                        .add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_width(f32::INFINITY))
                        .labelled_by(l.id);
                    if r.changed() {
                        self.dirty = true;
                    }
                });
            }
            ui.horizontal(|ui| {
                let l = ui.label("Cut (µm)");
                for (name, v) in
                    [("x0", &mut self.cut.x0), ("y0", &mut self.cut.y0), ("x1", &mut self.cut.x1), ("y1", &mut self.cut.y1)]
                {
                    let n = ui.label(RichText::new(name).small().color(pal.text_weak));
                    let r = ui.add(egui::DragValue::new(v).speed(0.01).max_decimals(3)).labelled_by(n.id).labelled_by(l.id);
                    if r.changed() {
                        self.dirty = true;
                    }
                }
                if ui.button("Across the diffusion").on_hover_text("Horizontal through the largest diffusion area").clicked()
                    && let (Some(p), Some(c)) = (project, &self.cell)
                    && let Ok(cut) = default_cut(p, c)
                {
                    self.cut = cut;
                    self.dirty = true;
                }
            });
            if let Some(f) = &self.flow {
                let n = f.steps.len();
                let mut k = self.steps.min(n);
                ui.horizontal(|ui| {
                    let l = ui.label("After step");
                    let r = ui.add(egui::Slider::new(&mut k, 0..=n)).labelled_by(l.id);
                    let what = if k == 0 { "substrate".to_string() } else { f.steps[k - 1].label() };
                    ui.label(RichText::new(what).color(pal.text_weak));
                    if r.changed() {
                        self.steps = if k == n { usize::MAX } else { k };
                        self.dirty = true;
                    }
                });
            }
            if let Some(e) = &self.error {
                ui.colored_label(pal.danger, e);
            }
            let (Some(s), Some(f)) = (&self.section, &self.flow) else { return };
            let tex = match &self.texture {
                Some(t) => t.clone(),
                None => {
                    let t = ui.ctx().load_texture("process_section", self.image(s, f), TextureOptions::NEAREST);
                    self.texture = Some(t.clone());
                    t
                }
            };
            let (w, h) = (s.nx as f64 * s.h, s.nz as f64 * s.h);
            let mut probe = self.probe;
            let stack_h = 64.0;
            Plot::new("pe_plot")
                .data_aspect(1.0)
                .height((ui.available_height() - stack_h).max(220.0))
                .legend(Legend::default().position(egui_plot::Corner::RightTop))
                .x_axis_label("µm along the cut")
                .y_axis_label("z (µm)")
                .allow_scroll(false)
                .show(ui, |pu| {
                    pu.image(PlotImage::new(
                        "",
                        tex.id(),
                        PlotPoint::new(w / 2.0, s.z0 + h / 2.0),
                        egui::vec2(w as f32, h as f32),
                    ));
                    // Legend entries: one point per material, off to the side.
                    for m in &s.materials {
                        let c = f.color(m);
                        pu.points(
                            Points::new(m.clone(), vec![[w / 2.0, s.z0]]).color(Color32::from_rgb(c[0], c[1], c[2])).radius(0.0),
                        );
                    }
                    if (pu.response().clicked() || pu.response().dragged())
                        && let Some(pt) = pu.pointer_coordinate()
                    {
                        probe = Some(pt.x.clamp(0.0, w));
                    }
                    if let Some(x) = probe {
                        pu.vline(egui_plot::VLine::new("probe", x).color(pal.accent).width(1.0));
                    }
                });
            self.probe = probe;
            let x = self.probe.unwrap_or(w / 2.0);
            let stack: Vec<String> = s.stack_at(x).iter().rev().map(|(m, t)| format!("{m} {:.0} nm", t * 1000.0)).collect();
            ui.label(
                RichText::new(format!("x = {x:.3} µm, surface {:.3} µm (click the section to probe)", s.surface_at(x)))
                    .color(pal.text_weak),
            );
            ui.label(RichText::new(stack.join("  |  ")).monospace());
        });
        self.open = open;
        action
    }
}
