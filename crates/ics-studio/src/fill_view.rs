//! Dummy Fill tool: fill a layout layer with grid-aligned squares to meet its density rule.

use eframe::egui::{self, RichText};
use ics_core::fill::{self, FillParams, FillReport};
use ics_core::tech::Tech;
use ics_core::{CellRef, IRect, Project};

use crate::theme::{self, Palette};

#[derive(Default)]
pub struct FillTool {
    pub open: bool,
    pub params: Option<FillParams>,
    /// Fill only this region (µm: x0, y0, x1, y1) instead of the whole cell.
    pub region: Option<[f64; 4]>,
    /// Last preview: the cell it was made for and its result.
    preview: Option<(CellRef, Result<FillReport, String>)>,
}

impl FillTool {
    pub fn show(&mut self, tech: &Tech) {
        if self.params.as_ref().is_none_or(|p| tech.layer(p.layer).is_none()) {
            let layer = tech.metal1().or_else(|| tech.layers.iter().position(|l| l.routing)).unwrap_or(0);
            self.params = Some(FillParams::for_layer(tech, layer));
        }
        self.open = true;
    }

    /// The parameters with the region applied, for `tech`.
    pub fn effective(&self, tech: &Tech) -> Option<FillParams> {
        let mut p = self.params.clone()?;
        p.area = self.region.map(|[x0, y0, x1, y1]| {
            let d = |v: f64| snap(v, tech);
            IRect::new(d(x0.min(x1)), d(y0.min(y1)), d(x0.max(x1)), d(y0.max(y1)))
        });
        Some(p)
    }

    pub fn run(&self, project: &Project, cell: &CellRef) -> Result<FillReport, String> {
        let p = self.effective(&project.tech).ok_or("no fill parameters")?;
        let top = project.cell(cell).and_then(|c| c.layout.as_ref()).ok_or_else(|| format!("{cell} has no layout"))?;
        fill::generate(top, project, &project.tech, &p)
    }

    /// The tool window; returns true when the user asks to add the fill to the active cell.
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>, active: Option<&CellRef>) -> bool {
        if !self.open {
            return false;
        }
        let Some(project) = project else {
            self.open = false;
            return false;
        };
        let mut open = true;
        let r = crate::float::tool_window(ctx, "dummy_fill", "Dummy Fill", [520.0, 640.0], &mut open, |ui| {
            let pal = theme::palette(ui.ctx());
            let mut add = false;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| add = self.ui(ui, project, active, &pal));
            });
            add
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(&mut self, ui: &mut egui::Ui, project: &Project, active: Option<&CellRef>, pal: &Palette) -> bool {
        let tech = &project.tech;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Dummy Fill").strong().color(pal.text_strong));
            match active {
                Some(c) => ui.label(RichText::new(format!("· {c}")).color(pal.text_weak)),
                None => ui.label(RichText::new("· open a layout cell").color(pal.warning)),
            };
        });
        ui.separator();
        let Some(p) = self.params.as_mut() else { return false };
        let before = p.clone();
        theme::section(ui, "Layer");
        ui.horizontal(|ui| {
            let l = ui.label("Fill layer");
            let name = tech.layer(p.layer).map_or("?", |d| d.name.as_str());
            let mut layer = p.layer;
            egui::ComboBox::from_id_salt("fill_layer")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for (k, d) in tech.layers.iter().enumerate() {
                        ui.selectable_value(&mut layer, k, d.name.as_str());
                    }
                })
                .response
                .labelled_by(l.id);
            if layer != p.layer {
                *p = FillParams::for_layer(tech, layer);
            }
            if ui.button("Defaults").on_hover_text("Reset to values derived from the layer rules and its density rule").clicked()
            {
                *p = FillParams::for_layer(tech, p.layer);
            }
        });
        if let Some(r) = fill::density_rule(tech, p.layer) {
            ui.label(
                RichText::new(format!(
                    "Density rule: {:.0}% – {:.0}% in {} µm windows",
                    r.min * 100.0,
                    r.max * 100.0,
                    um(r.window, tech)
                ))
                .small()
                .color(pal.text_weak),
            );
        } else {
            ui.label(RichText::new("No density rule on this layer in the DRC deck").small().color(pal.text_weak));
        }
        theme::section(ui, "Shapes");
        egui::Grid::new("fill_geom").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            dbu_field(ui, "Square size", &mut p.size, tech);
            dbu_field(ui, "Spacing", &mut p.spacing, tech);
            dbu_field(ui, "Keepout", &mut p.keepout, tech);
        });
        ui.label(RichText::new("Avoid shapes on").small().color(pal.text_weak));
        ui.horizontal_wrapped(|ui| {
            for (k, d) in tech.layers.iter().enumerate() {
                let mut on = p.avoid.contains(&k);
                if ui.checkbox(&mut on, d.name.as_str()).changed() {
                    if on {
                        p.avoid.push(k);
                    } else {
                        p.avoid.retain(|x| *x != k);
                    }
                }
            }
        });
        theme::section(ui, "Density");
        let mut targeted = p.target.is_some();
        ui.checkbox(&mut targeted, "Stop at a target density per window").on_hover_text("Off: fill every free position");
        if targeted != p.target.is_some() {
            p.target = targeted.then_some(0.3);
        }
        if let Some(t) = p.target.as_mut() {
            egui::Grid::new("fill_density").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                let l = ui.label("Target");
                let mut pct = *t * 100.0;
                ui.add(egui::DragValue::new(&mut pct).range(1.0..=95.0).speed(0.5).suffix(" %")).labelled_by(l.id);
                *t = pct / 100.0;
                ui.end_row();
                dbu_field(ui, "Window", &mut p.window, tech);
                dbu_field(ui, "Step", &mut p.step, tech);
            });
        }
        theme::section(ui, "Area");
        let mut whole = self.region.is_none();
        ui.horizontal(|ui| {
            ui.radio_value(&mut whole, true, "Whole cell");
            ui.radio_value(&mut whole, false, "Region");
        });
        if whole {
            self.region = None;
        } else {
            let bbox = active.and_then(|c| project.cell(c)).and_then(|c| c.layout.as_ref()).and_then(|l| {
                l.shapes
                    .iter()
                    .map(|s| s.bbox())
                    .reduce(|a, b| IRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
            });
            let r = self.region.get_or_insert_with(|| {
                bbox.map_or([0.0, 0.0, 100.0, 100.0], |b| [um(b.x0, tech), um(b.y0, tech), um(b.x1, tech), um(b.y1, tech)])
            });
            egui::Grid::new("fill_region").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                for (k, name) in ["x0", "y0", "x1", "y1"].iter().enumerate() {
                    let l = ui.label(*name);
                    ui.add(egui::DragValue::new(&mut r[k]).speed(0.5).suffix(" µm")).labelled_by(l.id);
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        }
        let changed = self.params.as_ref() != Some(&before);
        if changed {
            self.preview = None;
        }
        ui.add_space(10.0);
        let mut add = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(active.is_some(), egui::Button::new("Preview"))
                .on_hover_text("Compute the fill without changing the layout")
                .clicked()
                && let Some(c) = active
            {
                self.preview = Some((c.clone(), self.run(project, c)));
            }
            let b = egui::Button::new(RichText::new("Add Fill").strong().color(theme::on_fill(pal.layout))).fill(pal.layout);
            add = ui.add_enabled(active.is_some(), b).on_hover_text("Add the fill squares to the cell (undoable)").clicked();
        });
        if let Some((c, r)) = &self.preview {
            ui.add_space(6.0);
            match r {
                Ok(rep) => report(ui, c, rep, pal),
                Err(e) => {
                    ui.label(RichText::new(e).color(pal.danger));
                }
            }
        }
        if add {
            self.preview = None;
        }
        add
    }
}

/// Summary lines of a fill result.
pub fn summary(rep: &FillReport) -> String {
    let mut s = format!("{} square(s), density {:.1}% → {:.1}%", rep.rects.len(), rep.before * 100.0, rep.after * 100.0);
    if rep.limits.is_some() && rep.windows_below + rep.windows_above > 0 {
        s += &format!(", {} window(s) below and {} above the rule", rep.windows_below, rep.windows_above);
    }
    s
}

fn report(ui: &mut egui::Ui, cell: &CellRef, rep: &FillReport, pal: &Palette) {
    egui::Frame::new().fill(pal.bg_widget).corner_radius(6).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(format!("Preview · {cell}")).strong());
        ui.label(summary(rep));
        ui.label(RichText::new(format!("{} free position(s)", rep.candidates)).small().color(pal.text_weak));
        if let Some((lo, hi)) = rep.limits {
            let ok = rep.windows_below + rep.windows_above == 0;
            let t = format!(
                "Rule {:.0}% – {:.0}%: {}",
                lo * 100.0,
                hi * 100.0,
                if ok { "every window passes" } else { "some windows fail" }
            );
            ui.label(RichText::new(t).color(if ok { pal.ok } else { pal.warning }));
        }
    });
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
    if ui.add(egui::DragValue::new(&mut x).speed(step).range(0.0..=1e4).suffix(" µm")).labelled_by(l.id).changed() {
        *v = snap(x, tech);
    }
    ui.end_row();
}
