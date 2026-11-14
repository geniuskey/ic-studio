//! OPC tool: model-based optical proximity correction of a layout layer, with a preview of the
//! target, the corrected mask and what each prints.

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use ics_core::opc::{self, EpeStats, OpcParams, OpcReport};
use ics_core::region::Region;
use ics_core::tech::Tech;
use ics_core::{CellRef, IRect, Project};

use crate::theme::{self, Palette};

/// Polygons drawn per region in the preview.
const MAX_DRAWN: usize = 4000;

pub struct OpcTool {
    pub open: bool,
    pub params: Option<OpcParams>,
    /// Layer the mask is written to; the corrected layer itself replaces its shapes.
    pub output: Option<usize>,
    preview: Option<(CellRef, Result<OpcReport, String>)>,
    show: [bool; 4],
    zoom: f32,
    pan: Vec2,
}

impl Default for OpcTool {
    fn default() -> Self {
        OpcTool { open: false, params: None, output: None, preview: None, show: [true; 4], zoom: 1.0, pan: Vec2::ZERO }
    }
}

impl OpcTool {
    pub fn show(&mut self, tech: &Tech) {
        if self.params.as_ref().is_none_or(|p| tech.layer(p.layer).is_none()) {
            let layer = tech.poly().or_else(|| tech.metal1()).unwrap_or(0);
            self.params = Some(OpcParams::for_layer(tech, layer));
            self.output = None;
        }
        self.open = true;
    }

    pub fn run(&self, project: &Project, cell: &CellRef) -> Result<OpcReport, String> {
        let p = self.params.as_ref().ok_or("no OPC parameters")?;
        let top = project.cell(cell).and_then(|c| c.layout.as_ref()).ok_or_else(|| format!("{cell} has no layout"))?;
        opc::correct(top, project, &project.tech, p)
    }

    /// The tool window; returns true when the user asks to write the mask into the active cell.
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>, active: Option<&CellRef>) -> bool {
        if !self.open {
            return false;
        }
        let Some(project) = project else {
            self.open = false;
            return false;
        };
        let mut open = true;
        let r = crate::float::tool_window(ctx, "opc", "OPC", [560.0, 760.0], &mut open, |ui| {
            let pal = theme::palette(ui.ctx());
            let mut apply = false;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| apply = self.ui(ui, project, active, &pal));
            });
            apply
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(&mut self, ui: &mut egui::Ui, project: &Project, active: Option<&CellRef>, pal: &Palette) -> bool {
        let tech = &project.tech;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Optical Proximity Correction").strong().color(pal.text_strong));
            match active {
                Some(c) => ui.label(RichText::new(format!("· {c}")).color(pal.text_weak)),
                None => ui.label(RichText::new("· open a layout cell").color(pal.warning)),
            };
        });
        ui.label(
            RichText::new(
                "Gaussian blur + threshold resist model; edges are fragmented and moved against their placement error.",
            )
            .small()
            .color(pal.text_weak),
        );
        ui.separator();
        let Some(p) = self.params.as_mut() else { return false };
        let before = (p.clone(), self.output);
        theme::section(ui, "Layers");
        egui::Grid::new("opc_layers").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            let l = ui.label("Correct");
            let mut layer = p.layer;
            layer_combo(ui, "opc_layer", tech, &mut layer, l.id);
            if layer != p.layer {
                *p = OpcParams::for_layer(tech, layer);
            }
            ui.end_row();
            let l = ui.label("Write mask to");
            let mut out = self.output.unwrap_or(p.layer);
            layer_combo(ui, "opc_output", tech, &mut out, l.id);
            self.output = (out != p.layer).then_some(out);
            ui.end_row();
        });
        if self.output.is_none() {
            ui.label(RichText::new("The mask replaces the layer's shapes in this cell").small().color(pal.text_weak));
        }
        theme::section(ui, "Optical model");
        egui::Grid::new("opc_model").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            dbu_field(ui, "Blur sigma", &mut p.sigma, tech);
            let l = ui.label("Threshold");
            ui.add(egui::DragValue::new(&mut p.threshold).range(0.05..=0.95).speed(0.01)).labelled_by(l.id);
            ui.end_row();
        });
        theme::section(ui, "Correction");
        egui::Grid::new("opc_corr").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            dbu_field(ui, "Fragment length", &mut p.fragment, tech);
            dbu_field(ui, "Max outward move", &mut p.max_out, tech);
            dbu_field(ui, "Max inward move", &mut p.max_in, tech);
            dbu_field(ui, "EPE tolerance", &mut p.tolerance, tech);
            let l = ui.label("Iterations");
            ui.add(egui::DragValue::new(&mut p.iterations).range(1..=50)).labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Gain");
            ui.add(egui::DragValue::new(&mut p.gain).range(0.1..=1.0).speed(0.01)).labelled_by(l.id);
            ui.end_row();
        });
        if ui.button("Defaults").on_hover_text("Reset to values derived from the layer's min width and spacing").clicked() {
            *p = OpcParams::for_layer(tech, p.layer);
        }
        let layer = p.layer;
        if (p.clone(), self.output) != before {
            self.preview = None;
        }
        ui.add_space(10.0);
        let mut apply = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(active.is_some(), egui::Button::new("Preview"))
                .on_hover_text("Run the correction without changing the layout")
                .clicked()
                && let Some(c) = active
            {
                self.preview = Some((c.clone(), self.run(project, c)));
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            let b = egui::Button::new(RichText::new("Apply OPC").strong().color(theme::on_fill(pal.layout))).fill(pal.layout);
            apply =
                ui.add_enabled(active.is_some(), b).on_hover_text("Write the corrected mask into the cell (undoable)").clicked();
        });
        if let Some((c, r)) = &self.preview {
            ui.add_space(6.0);
            match r {
                Ok(rep) => {
                    let color = tech.layer(layer).map_or(pal.layout, |d| Color32::from_rgb(d.color[0], d.color[1], d.color[2]));
                    report(ui, c, rep, tech, pal);
                    ui.horizontal_wrapped(|ui| {
                        for (k, name) in ["Target", "Mask", "Printed (no OPC)", "Printed (OPC)"].iter().enumerate() {
                            ui.checkbox(&mut self.show[k], *name);
                        }
                    });
                    canvas(ui, rep, color, self.show, &mut self.zoom, &mut self.pan, pal);
                }
                Err(e) => {
                    ui.label(RichText::new(e).color(pal.danger));
                }
            }
        }
        if apply {
            self.preview = None;
        }
        apply
    }
}

fn report(ui: &mut egui::Ui, cell: &CellRef, rep: &OpcReport, tech: &Tech, pal: &Palette) {
    let nm = |v: f64| v * 1000.0 / tech.dbu_per_um.max(1) as f64;
    egui::Frame::new().fill(pal.bg_widget).corner_radius(6).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(format!("Preview · {cell}")).strong());
        egui::Grid::new("opc_stats").num_columns(4).spacing([14.0, 2.0]).show(ui, |ui| {
            for h in ["", "EPE rms", "EPE max", "On target"] {
                ui.label(RichText::new(h).small().color(pal.text_weak));
            }
            ui.end_row();
            let n = rep.fragments.len();
            let row = |ui: &mut egui::Ui, name: &str, s: &EpeStats, c: Color32| {
                ui.label(name);
                ui.label(RichText::new(format!("{:.2} nm", nm(s.rms))).color(c));
                ui.label(RichText::new(format!("{:.2} nm", nm(s.max))).color(c));
                ui.label(RichText::new(format!("{} / {n}", s.within)).color(c));
                ui.end_row();
            };
            row(ui, "Drawn", &rep.before, pal.danger);
            row(ui, "Corrected", &rep.after, if rep.after.within == n { pal.ok } else { pal.warning });
        });
        let um2 = |a: i128| a as f64 / (tech.dbu_per_um.max(1) as f64).powi(2);
        ui.label(
            RichText::new(format!(
                "{} iteration(s) · mask area {:.3} µm² ({:+.1}% of drawn)",
                rep.iterations,
                um2(rep.mask.area()),
                (rep.mask.area() as f64 / rep.target.area().max(1) as f64 - 1.0) * 100.0
            ))
            .small()
            .color(pal.text_weak),
        );
    });
}

/// Preview of the target, mask and printed contours; scroll to zoom, drag to pan.
fn canvas(ui: &mut egui::Ui, rep: &OpcReport, color: Color32, show: [bool; 4], zoom: &mut f32, pan: &mut Vec2, pal: &Palette) {
    let size = Vec2::new(ui.available_width(), 320.0);
    let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let rect = resp.rect;
    painter.rect_filled(rect, 4.0, pal.bg_canvas);
    if resp.dragged() {
        *pan += resp.drag_delta();
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let f = (scroll / 200.0).exp();
            let at = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center()) - rect.center();
            *pan = at - (at - *pan) * f;
            *zoom *= f;
        }
    }
    if resp.double_clicked() {
        *zoom = 1.0;
        *pan = Vec2::ZERO;
    }
    let a = rep.area;
    let fit = (rect.width() / a.width().max(1) as f32).min(rect.height() / a.height().max(1) as f32) * 0.95;
    let s = fit * *zoom;
    let (cx, cy) = ((a.x0 + a.x1) as f32 / 2.0, (a.y0 + a.y1) as f32 / 2.0);
    let c = rect.center() + *pan;
    let to = |x: i64, y: i64| Pos2::new(c.x + (x as f32 - cx) * s, c.y - (y as f32 - cy) * s);
    let screen = |r: &IRect| Rect::from_two_pos(to(r.x0, r.y0), to(r.x1, r.y1));
    let painter = painter.with_clip_rect(rect);
    if show[0] {
        for r in rep.target.rects().iter().take(MAX_DRAWN * 4) {
            painter.rect_filled(screen(r), 0.0, color.gamma_multiply(0.25));
        }
    }
    if show[1] {
        for r in rep.mask.rects().iter().take(MAX_DRAWN * 4) {
            painter.rect_filled(screen(r), 0.0, pal.accent.gamma_multiply(0.18));
        }
        outline(&painter, &rep.mask, &to, Stroke::new(1.2, pal.accent));
    }
    if show[2] {
        outline(&painter, &rep.printed_before, &to, Stroke::new(1.4, pal.danger));
    }
    if show[3] {
        outline(&painter, &rep.printed_after, &to, Stroke::new(1.6, pal.ok));
    }
    if show[0] {
        outline(&painter, &rep.target, &to, Stroke::new(1.0, pal.text));
    }
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
    painter.text(
        rect.left_bottom() + Vec2::new(8.0, -6.0),
        egui::Align2::LEFT_BOTTOM,
        "scroll to zoom · drag to pan · double-click to fit",
        egui::FontId::proportional(11.0),
        pal.text_weak,
    );
}

fn outline(painter: &egui::Painter, r: &Region, to: &impl Fn(i64, i64) -> Pos2, stroke: Stroke) {
    for poly in r.to_polygons().iter().take(MAX_DRAWN) {
        let mut pts: Vec<Pos2> = poly.iter().map(|&(x, y)| to(x, y)).collect();
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
        painter.add(egui::Shape::line(pts, stroke));
    }
}

fn layer_combo(ui: &mut egui::Ui, id: &str, tech: &Tech, layer: &mut usize, label: egui::Id) {
    let name = tech.layer(*layer).map_or("?", |d| d.name.as_str());
    egui::ComboBox::from_id_salt(id)
        .selected_text(name)
        .show_ui(ui, |ui| {
            for (k, d) in tech.layers.iter().enumerate() {
                ui.selectable_value(layer, k, d.name.as_str());
            }
        })
        .response
        .labelled_by(label);
}

fn um(v: i64, tech: &Tech) -> f64 {
    v as f64 / tech.dbu_per_um as f64
}

fn snap(v: f64, tech: &Tech) -> i64 {
    let g = tech.grid.max(1);
    ((v * tech.dbu_per_um as f64 / g as f64).round() as i64) * g
}

fn dbu_field(ui: &mut egui::Ui, label: &str, v: &mut i64, tech: &Tech) {
    let l = ui.label(label);
    let mut x = um(*v, tech);
    let step = um(tech.grid.max(1), tech);
    if ui.add(egui::DragValue::new(&mut x).speed(step).range(0.0..=100.0).suffix(" µm")).labelled_by(l.id).changed() {
        *v = snap(x, tech);
    }
    ui.end_row();
}
