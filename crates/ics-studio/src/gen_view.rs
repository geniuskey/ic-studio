//! Layout Generators: guard rings, via arrays (Virtuoso Create Guard Ring / Create Via Array)
//! and matched device placement (Layout XL Modgen) for the active layout.

use eframe::egui::{self, RichText};
use ics_core::layout_gen::{GuardRing, ViaArray};
use ics_core::matching::{MatchSpec, Pattern};
use ics_core::tech::Tech;
use ics_core::{CellRef, IRect, Project};

use crate::theme::{self, Palette};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum GenTab {
    #[default]
    GuardRing,
    ViaArray,
    Matching,
    Example,
}

pub enum GenAction {
    GuardRing(GuardRing),
    ViaArray(ViaArray),
    /// Via arrays on every overlap of the via's layers, within the area when given.
    ViaOverlaps {
        via: String,
        area: Option<IRect>,
    },
    Match(MatchSpec),
    /// Place the active cell's devices like the reference cell's layout, then route if asked.
    PlaceLike {
        reference: CellRef,
        route: bool,
    },
}

pub struct LayoutGenerators {
    pub open: bool,
    pub tab: GenTab,
    /// Area in µm (x0, y0, x1, y1) for the guard ring and the via array.
    pub area: [f64; 4],
    pub ring: GuardRing,
    pub via: String,
    pub overlaps: bool,
    pub matching: MatchSpec,
    /// Reference cell for layout by example.
    pub reference: Option<CellRef>,
    pub route_after: bool,
}

impl Default for LayoutGenerators {
    fn default() -> Self {
        Self {
            open: false,
            tab: GenTab::GuardRing,
            area: [0.0, 0.0, 5.0, 5.0],
            ring: GuardRing { area: IRect::new(0, 0, 0, 0), well: false, width: 0, gap: 0, contacts: true, metal: true },
            via: String::new(),
            overlaps: true,
            matching: MatchSpec::default(),
            reference: None,
            route_after: true,
        }
    }
}

fn um(v: i64, tech: &Tech) -> f64 {
    v as f64 / tech.dbu_per_um.max(1) as f64
}

fn dbu(v: f64, tech: &Tech) -> i64 {
    let g = tech.grid.max(1);
    ((v * tech.dbu_per_um as f64 / g as f64).round() as i64) * g
}

impl LayoutGenerators {
    pub fn show(&mut self, tab: GenTab, sel: Option<IRect>, tech: &Tech) {
        self.open = true;
        self.tab = tab;
        if let Some(b) = sel {
            self.set_area(b, tech);
        }
    }

    pub fn set_area(&mut self, b: IRect, tech: &Tech) {
        self.area = [um(b.x0, tech), um(b.y0, tech), um(b.x1, tech), um(b.y1, tech)];
    }

    pub fn area_dbu(&self, tech: &Tech) -> IRect {
        let [x0, y0, x1, y1] = self.area;
        IRect::new(dbu(x0.min(x1), tech), dbu(y0.min(y1), tech), dbu(x0.max(x1), tech), dbu(y0.max(y1), tech))
    }

    /// The window; `sel` is the bounding box of the layout selection, `devices` the MOS devices
    /// placed in the active layout.
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        project: Option<&Project>,
        cell: Option<&CellRef>,
        sel: Option<IRect>,
        devices: &[String],
    ) -> Option<GenAction> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        let mut open = true;
        let r = crate::float::tool_window(ctx, "layout_generators", "Layout Generators", [560.0, 600.0], &mut open, |ui| {
            let pal = theme::palette(ui.ctx());
            let mut action = None;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Layout Generators").strong().color(pal.text_strong));
                    ui.label(
                        RichText::new(cell.map_or("· open a layout cell".into(), |c| format!("· {c}"))).color(pal.text_weak),
                    );
                });
                ui.horizontal(|ui| {
                    for (t, n) in [
                        (GenTab::GuardRing, "Guard Ring"),
                        (GenTab::ViaArray, "Via Array"),
                        (GenTab::Matching, "Matched Placement"),
                        (GenTab::Example, "By Example"),
                    ] {
                        if ui.selectable_label(self.tab == t, n).clicked() {
                            self.tab = t;
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    action = match self.tab {
                        GenTab::GuardRing => self.ring_ui(ui, &project.tech, sel, cell.is_some(), &pal),
                        GenTab::ViaArray => self.via_ui(ui, &project.tech, sel, cell.is_some(), &pal),
                        GenTab::Matching => self.match_ui(ui, devices, cell.is_some(), &pal),
                        GenTab::Example => self.example_ui(ui, project, cell, &pal),
                    };
                });
            });
            action
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn area_ui(&mut self, ui: &mut egui::Ui, tech: &Tech, sel: Option<IRect>, pal: &Palette) {
        egui::Grid::new("gen_area").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
            for (k, name) in ["x0", "y0", "x1", "y1"].iter().enumerate() {
                let l = ui.label(*name);
                ui.add(egui::DragValue::new(&mut self.area[k]).speed(0.05).suffix(" µm")).labelled_by(l.id);
                if k % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Use selection"))
                .on_disabled_hover_text("Select shapes or instances first")
                .clicked()
                && let Some(b) = sel
            {
                self.set_area(b, tech);
            }
            let a = self.area_dbu(tech);
            ui.label(
                RichText::new(format!("{:.3} × {:.3} µm", um(a.width(), tech), um(a.height(), tech)))
                    .small()
                    .color(pal.text_weak),
            );
        });
    }

    fn ring_ui(&mut self, ui: &mut egui::Ui, tech: &Tech, sel: Option<IRect>, ready: bool, pal: &Palette) -> Option<GenAction> {
        theme::section(ui, "Area to surround");
        self.area_ui(ui, tech, sel, pal);
        theme::section(ui, "Ring");
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.ring.well, false, "p+ substrate ring (NMOS)");
            ui.radio_value(&mut self.ring.well, true, "n+ ring in nwell (PMOS)");
        });
        egui::Grid::new("gen_ring").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            for (label, v) in [("Width (0 = minimum)", &mut self.ring.width), ("Gap to area (0 = minimum)", &mut self.ring.gap)] {
                let l = ui.label(label);
                let mut x = um(*v, tech);
                if ui.add(egui::DragValue::new(&mut x).speed(0.01).range(0.0..=100.0).suffix(" µm")).labelled_by(l.id).changed()
                {
                    *v = dbu(x, tech);
                }
                ui.end_row();
            }
        });
        ui.checkbox(&mut self.ring.contacts, "Contacts along the ring");
        ui.checkbox(&mut self.ring.metal, "Metal1 strap");
        if self.ring.contacts && !self.ring.metal {
            ui.label(RichText::new("Contacts without the metal strap are not DRC clean").small().color(pal.warning));
        }
        ui.add_space(8.0);
        let b = egui::Button::new(RichText::new("Create Guard Ring").strong().color(theme::on_fill(pal.layout))).fill(pal.layout);
        ui.add_enabled(ready, b)
            .clicked()
            .then(|| GenAction::GuardRing(GuardRing { area: self.area_dbu(tech), ..self.ring.clone() }))
    }

    fn via_ui(&mut self, ui: &mut egui::Ui, tech: &Tech, sel: Option<IRect>, ready: bool, pal: &Palette) -> Option<GenAction> {
        theme::section(ui, "Via");
        if tech.via(&self.via).is_none() {
            self.via = tech
                .vias
                .iter()
                .find(|v| v.name.to_ascii_lowercase().starts_with("via"))
                .or(tech.vias.first())
                .map_or(String::new(), |v| v.name.clone());
        }
        let l = ui.label("Via");
        egui::ComboBox::from_id_salt("gen_via")
            .selected_text(self.via.as_str())
            .show_ui(ui, |ui| {
                for v in &tech.vias {
                    ui.selectable_value(&mut self.via, v.name.clone(), format!("{}  ({} → {})", v.name, v.bottom, v.top));
                }
            })
            .response
            .labelled_by(l.id);
        theme::section(ui, "Where");
        ui.radio_value(&mut self.overlaps, true, "Every overlap of the via's layers");
        ui.radio_value(&mut self.overlaps, false, "Fill the area below");
        let mut limit = !self.overlaps;
        if self.overlaps {
            ui.checkbox(&mut limit, "Only within the area below");
        }
        self.area_ui(ui, tech, sel, pal);
        ui.add_space(8.0);
        let b = egui::Button::new(RichText::new("Create Via Arrays").strong().color(theme::on_fill(pal.layout))).fill(pal.layout);
        if !ui.add_enabled(ready && !self.via.is_empty(), b).clicked() {
            return None;
        }
        let area = self.area_dbu(tech);
        Some(if self.overlaps {
            GenAction::ViaOverlaps { via: self.via.clone(), area: limit.then_some(area) }
        } else {
            GenAction::ViaArray(ViaArray { area, via: self.via.clone() })
        })
    }

    fn example_ui(&mut self, ui: &mut egui::Ui, project: &Project, cell: Option<&CellRef>, pal: &Palette) -> Option<GenAction> {
        theme::section(ui, "Layout by example");
        ui.label(
            RichText::new(
                "Reuse the placement of a finished cell with the same circuit: devices are matched by topology, \
                 placed where their counterparts are and sized from this cell's schematic. Wells, implants and taps \
                 are copied; wires are routed again.",
            )
            .color(pal.text_weak),
        );
        let refs: Vec<CellRef> = project
            .all_cells()
            .filter(|(r, c)| {
                Some(r) != cell && c.schematic.is_some() && c.layout.as_ref().is_some_and(|l| !l.instances.is_empty())
            })
            .map(|(r, _)| r)
            .collect();
        if self.reference.as_ref().is_some_and(|r| !refs.contains(r)) {
            self.reference = None;
        }
        ui.horizontal(|ui| {
            let l = ui.label("Reference");
            egui::ComboBox::from_id_salt("gen_reference")
                .selected_text(self.reference.as_ref().map_or("choose a cell".into(), |r| r.to_string()))
                .show_ui(ui, |ui| {
                    for r in &refs {
                        ui.selectable_value(&mut self.reference, Some(r.clone()), r.to_string());
                    }
                })
                .response
                .labelled_by(l.id);
        });
        ui.checkbox(&mut self.route_after, "Route the flylines afterwards");
        let ready = cell.is_some() && self.reference.is_some();
        if ui.add_enabled(ready, egui::Button::new("Place like reference")).clicked() {
            return Some(GenAction::PlaceLike { reference: self.reference.clone()?, route: self.route_after });
        }
        None
    }

    fn match_ui(&mut self, ui: &mut egui::Ui, devices: &[String], ready: bool, pal: &Palette) -> Option<GenAction> {
        let m = &mut self.matching;
        theme::section(ui, "Devices");
        if devices.is_empty() {
            ui.label(RichText::new("No MOS devices in this layout (generate it from the schematic first)").color(pal.text_weak));
        }
        m.devices.retain(|d| devices.contains(d));
        ui.horizontal_wrapped(|ui| {
            for d in devices {
                let mut on = m.devices.contains(d);
                if ui.checkbox(&mut on, d.as_str()).changed() {
                    if on {
                        m.devices.push(d.clone());
                    } else {
                        m.devices.retain(|x| x != d);
                    }
                }
            }
        });
        theme::section(ui, "Pattern");
        ui.horizontal(|ui| {
            ui.radio_value(&mut m.pattern, Pattern::CommonCentroid, "Common centroid (ABBA)");
            ui.radio_value(&mut m.pattern, Pattern::Interdigitated, "Interdigitated (ABAB)");
            ui.radio_value(&mut m.pattern, Pattern::CommonCentroid2D, "2-D common centroid");
        });
        egui::Grid::new("gen_match").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            let l = ui.label("Fingers per unit");
            ui.add(egui::DragValue::new(&mut m.unit_fingers).range(1..=64)).labelled_by(l.id);
            ui.end_row();
            if m.pattern == Pattern::CommonCentroid2D {
                let l = ui.label("Rows");
                ui.add(egui::DragValue::new(&mut m.rows).range(2..=16)).labelled_by(l.id);
                ui.end_row();
            }
        });
        ui.checkbox(&mut m.dummies, "Dummy devices at both row ends");
        let counts: Vec<usize> = m.devices.iter().map(|_| 2).collect();
        if m.devices.len() >= 2
            && let Ok(p) = ics_core::matching::plan(m, &counts)
        {
            ui.label(RichText::new(format!("Preview with 2 units each: {}", p.pattern)).monospace().small().color(pal.text_weak));
        }
        ui.label(RichText::new("Run before routing: wires crossing the block are not moved.").small().color(pal.text_weak));
        ui.add_space(8.0);
        let b = egui::Button::new(RichText::new("Place Matched").strong().color(theme::on_fill(pal.layout))).fill(pal.layout);
        ui.add_enabled(ready && m.devices.len() >= 2, b)
            .on_disabled_hover_text("Pick at least two devices")
            .clicked()
            .then(|| GenAction::Match(m.clone()))
    }
}
