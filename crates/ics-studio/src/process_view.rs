//! Process Emulation window: the cross-section of the active cell's layout along a cut line,
//! built by a process flow step by step (`ics_core::process`).

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use egui_plot::{Legend, Line, Plot, PlotImage, PlotPoint, PlotPoints, Points};
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
    /// Colour the semiconductors by net doping.
    pub show_doping: bool,
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
            show_doping: true,
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

    /// Colour of a net doping (cm⁻³): red for n, blue for p, deeper with the concentration
    /// (1e14 to 1e21, log).
    pub fn doping_color(net: f64) -> Color32 {
        let t = ((net.abs().max(1.0).log10() - 14.0) / 7.0).clamp(0.0, 1.0) as f32;
        let mix = |a: f32, b: f32| (a + (b - a) * t).round() as u8;
        if net > 0.0 {
            Color32::from_rgb(mix(250.0, 170.0), mix(215.0, 10.0), mix(215.0, 30.0))
        } else {
            Color32::from_rgb(mix(215.0, 20.0), mix(225.0, 60.0), mix(250.0, 170.0))
        }
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
        let doping = self.show_doping && !s.dopants.is_empty();
        for iz in 0..s.nz {
            let row = s.nz - 1 - iz;
            for ix in 0..s.nx {
                let c = s.cells[iz * s.nx + ix];
                if c > 0 {
                    img.pixels[row * s.nx + ix] = colors[c as usize - 1];
                }
                if doping && s.is_semi(ix, iz) {
                    let net = s.doping(ix, iz).0;
                    if net != 0.0 {
                        // Junctions (sign change to the right or below) are drawn dark.
                        let flips = |jx: usize, jz: usize| s.is_semi(jx, jz) && s.doping(jx, jz).0 * net < 0.0;
                        let edge = (ix + 1 < s.nx && flips(ix + 1, iz)) || (iz > 0 && flips(ix, iz - 1));
                        img.pixels[row * s.nx + ix] = if edge { Color32::from_gray(30) } else { Self::doping_color(net) };
                    }
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
                if self.section.as_ref().is_some_and(|s| !s.dopants.is_empty())
                    && ui
                        .toggle_value(&mut self.show_doping, "Doping")
                        .on_hover_text("Colour silicon and poly by net doping: red n, blue p")
                        .changed()
                {
                    self.texture = None;
                }
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
            let doped = !s.dopants.is_empty();
            let text_h = if doped { 64.0 + 44.0 } else { 64.0 };
            let plot_h = (ui.available_height() - text_h).max(220.0);
            let shown = self.probe.unwrap_or(w / 2.0);
            let show_doping = self.show_doping;
            ui.horizontal(|ui| {
                let sec_w = if doped { ui.available_width() * 0.64 } else { ui.available_width() };
                Plot::new("pe_plot")
                    .data_aspect(1.0)
                    .width(sec_w)
                    .height(plot_h)
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
                        // Legend entries: one point per material, off to the side; with the doping
                        // shown, n and p in place of the semiconductors.
                        let overlay = doped && show_doping;
                        if overlay {
                            for (name, net) in [("n-type", 1e19), ("p-type", -1e19)] {
                                pu.points(Points::new(name, vec![[w / 2.0, s.z0]]).color(Self::doping_color(net)).radius(0.0));
                            }
                        }
                        for (k, m) in s.materials.iter().enumerate() {
                            if overlay && s.semi[k] {
                                continue;
                            }
                            let c = f.color(m);
                            pu.points(
                                Points::new(m.clone(), vec![[w / 2.0, s.z0]])
                                    .color(Color32::from_rgb(c[0], c[1], c[2]))
                                    .radius(0.0),
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
                if doped {
                    // The dopant depth profile at the probe, depth up the side like the section.
                    let prof = s.profile(shown);
                    let colors = [
                        Color32::from_rgb(40, 90, 220),
                        Color32::from_rgb(200, 60, 40),
                        Color32::from_rgb(220, 130, 20),
                        Color32::from_rgb(120, 60, 180),
                    ];
                    Plot::new("pe_profile")
                        .width(ui.available_width())
                        .height(plot_h)
                        .legend(Legend::default().position(egui_plot::Corner::RightBottom))
                        .x_axis_label(format!("cm⁻³ at x = {shown:.2} µm"))
                        .y_axis_label("z (µm)")
                        .x_axis_formatter(|m, _| {
                            if (m.value - m.value.round()).abs() < 1e-6 { format!("1e{:.0}", m.value) } else { String::new() }
                        })
                        .include_x(14.0)
                        .include_x(21.0)
                        .allow_scroll(false)
                        .show(ui, |pu| {
                            let log = |v: f64| if v > 1e10 { Some(v.log10()) } else { None };
                            for (k, (sp, _)) in s.dopants.iter().enumerate() {
                                let pts: Vec<[f64; 2]> = prof.iter().filter_map(|(z, _, c)| log(c[k]).map(|l| [l, *z])).collect();
                                pu.line(Line::new(sp.name(), PlotPoints::from(pts)).color(colors[k % colors.len()]));
                            }
                            let net: Vec<[f64; 2]> = prof.iter().filter_map(|(z, n, _)| log(n.abs()).map(|l| [l, *z])).collect();
                            pu.line(
                                Line::new("|net|", PlotPoints::from(net))
                                    .color(pal.text_weak)
                                    .style(egui_plot::LineStyle::dashed_dense()),
                            );
                            for z in s.junctions(shown) {
                                pu.hline(egui_plot::HLine::new("junction", z).color(pal.text_weak).width(0.5));
                            }
                        });
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
            if doped {
                let layers: Vec<String> = s
                    .doped_layers(x)
                    .iter()
                    .map(|l| {
                        format!(
                            "{} {} to {:.0} nm, peak {:.1e}, {} Ω/□",
                            if l.n_type { "n" } else { "p" },
                            l.material,
                            -l.bottom * 1000.0,
                            l.peak,
                            ics_core::units::format_eng(l.sheet_resistance, "")
                        )
                    })
                    .collect();
                let j: Vec<String> = s.junctions(x).iter().map(|z| format!("{:.0} nm", -z * 1000.0)).collect();
                ui.label(RichText::new(layers.join("  |  ")).monospace());
                ui.label(
                    RichText::new(format!("junctions at {} below z = 0", if j.is_empty() { "—".into() } else { j.join(", ") }))
                        .color(pal.text_weak),
                );
            }
        });
        self.open = open;
        action
    }
}
