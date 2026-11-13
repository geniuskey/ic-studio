//! Hierarchy Browser (Virtuoso Design Hierarchy): every layout of the project as a tree from its
//! top cells, with placement counts, where-used and flat statistics of the selected cell.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, RichText};
use ics_core::{CellRef, Project};

use crate::theme::{self, Palette};

/// Placement counts between the project's layouts.
#[derive(Default)]
pub struct Usage {
    /// Cell -> (child -> placements, arrays counted element by element).
    pub children: BTreeMap<CellRef, BTreeMap<CellRef, u64>>,
    /// Cell -> (parent -> placements).
    pub parents: BTreeMap<CellRef, BTreeMap<CellRef, u64>>,
    /// Layouts no other layout places.
    pub tops: Vec<CellRef>,
}

pub fn usage(p: &Project) -> Usage {
    let mut u = Usage::default();
    let layouts: Vec<CellRef> = p.all_cells().filter(|(_, c)| c.layout.is_some()).map(|(r, _)| r).collect();
    for (r, c) in p.all_cells() {
        let Some(l) = &c.layout else { continue };
        for i in &l.instances {
            *u.children.entry(r.clone()).or_default().entry(i.cell.clone()).or_default() += i.count();
            *u.parents.entry(i.cell.clone()).or_default().entry(r.clone()).or_default() += i.count();
        }
    }
    u.tops = layouts.into_iter().filter(|r| !u.parents.contains_key(r)).collect();
    u
}

/// Total placements of every cell under `top` (flattened through the hierarchy).
pub fn flat_counts(u: &Usage, top: &CellRef) -> BTreeMap<CellRef, u64> {
    let mut out = BTreeMap::new();
    fn walk(u: &Usage, c: &CellRef, mult: u64, depth: u32, out: &mut BTreeMap<CellRef, u64>) {
        if depth > 64 {
            return;
        }
        for (k, n) in u.children.get(c).into_iter().flatten() {
            *out.entry(k.clone()).or_default() += mult * n;
            walk(u, k, mult * n, depth + 1, out);
        }
    }
    walk(u, top, 1, 0, &mut out);
    out
}

pub enum HierAction {
    Open(CellRef, crate::app::Workspace),
}

#[derive(Default)]
pub struct HierBrowser {
    pub open: bool,
    pub selected: Option<CellRef>,
    pub filter: String,
}

impl HierBrowser {
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>, active: Option<&CellRef>) -> Option<HierAction> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        if self.selected.is_none() {
            self.selected = active.cloned();
        }
        let mut open = true;
        let r = crate::float::tool_window(ctx, "hier_browser", "Hierarchy Browser", [900.0, 600.0], &mut open, |ui| {
            self.ui(ui, project)
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(&mut self, ui: &mut egui::Ui, project: &Project) -> Option<HierAction> {
        let pal = theme::palette(ui.ctx());
        let u = usage(project);
        let mut action = None;
        egui::Panel::left("hb_tree")
            .resizable(true)
            .default_size(380.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| {
                ui.label(RichText::new("Hierarchy Browser").strong().color(pal.text_strong));
                let l = ui.label(RichText::new("Filter").small().color(pal.text_weak));
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("cell name").desired_width(f32::INFINITY))
                    .labelled_by(l.id);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let f = self.filter.to_ascii_lowercase();
                    if f.is_empty() {
                        theme::section(ui, &format!("Top cells ({})", u.tops.len()));
                        for t in &u.tops {
                            self.node(ui, &u, t, 1, &mut vec![], &pal);
                        }
                    } else {
                        for (r, c) in
                            project.all_cells().filter(|(r, c)| c.layout.is_some() && r.cell.to_ascii_lowercase().contains(&f))
                        {
                            let _ = c;
                            if ui.selectable_label(self.selected.as_ref() == Some(&r), r.to_string()).clicked() {
                                self.selected = Some(r);
                            }
                        }
                    }
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            let Some(sel) = self.selected.clone() else {
                ui.label(RichText::new("Select a cell in the tree.").color(pal.text_weak));
                return;
            };
            action = self.details(ui, project, &u, &sel, &pal);
        });
        action
    }

    fn node(&mut self, ui: &mut egui::Ui, u: &Usage, c: &CellRef, count: u64, path: &mut Vec<CellRef>, pal: &Palette) {
        let kids = u.children.get(c);
        let label = if count > 1 { format!("{}  ×{count}", c.cell) } else { c.cell.clone() };
        let sel = self.selected.as_ref() == Some(c);
        if path.contains(c) || path.len() > 32 {
            ui.label(RichText::new(format!("{label} (recursive)")).color(pal.danger));
            return;
        }
        match kids {
            Some(kids) if !kids.is_empty() => {
                let id = ui.make_persistent_id(("hb", path.clone(), c.clone()));
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, path.is_empty())
                    .show_header(ui, |ui| {
                        if ui.selectable_label(sel, RichText::new(&label).strong()).clicked() {
                            self.selected = Some(c.clone());
                        }
                    })
                    .body(|ui| {
                        path.push(c.clone());
                        for (k, n) in kids {
                            self.node(ui, u, k, *n, path, pal);
                        }
                        path.pop();
                    });
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    if ui.selectable_label(sel, label).clicked() {
                        self.selected = Some(c.clone());
                    }
                });
            }
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, p: &Project, u: &Usage, r: &CellRef, pal: &Palette) -> Option<HierAction> {
        let mut action = None;
        let Some(cell) = p.cell(r) else {
            ui.label(RichText::new(format!("{r} is not in the project")).color(pal.danger));
            return None;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(r.to_string()).heading().strong());
            if cell.layout.is_some() && ui.button("Open layout").clicked() {
                action = Some(HierAction::Open(r.clone(), crate::app::Workspace::Layout));
            }
            if cell.schematic.is_some() && ui.button("Open schematic").clicked() {
                action = Some(HierAction::Open(r.clone(), crate::app::Workspace::Schematic));
            }
        });
        if !cell.description.is_empty() {
            ui.label(RichText::new(&cell.description).color(pal.text_weak));
        }
        ui.add_space(6.0);
        if let Some(l) = &cell.layout {
            let flat = ics_core::hier::flatten(l, p, 64);
            let um = p.tech.dbu_per_um.max(1) as f64;
            let bbox = flat
                .iter()
                .filter(|s| !matches!(s.geom, ics_core::Geometry::Label { .. }))
                .map(|s| s.geom.bbox())
                .reduce(|a, b| ics_core::IRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)));
            egui::Grid::new("hb_stats").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(RichText::new(k).color(pal.text_weak));
                    ui.label(RichText::new(v).monospace());
                    ui.end_row();
                };
                row("Shapes", l.shapes.len().to_string());
                row(
                    "Instances",
                    format!("{} ({} placements)", l.instances.len(), l.instances.iter().map(|i| i.count()).sum::<u64>()),
                );
                row("Flat shapes", flat.len().to_string());
                if let Some(b) = bbox {
                    row("Extent", format!("{:.3} × {:.3} µm", b.width() as f64 / um, b.height() as f64 / um));
                }
                row(
                    "Views",
                    [cell.schematic.as_ref().map(|_| "schematic"), Some("layout")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            });
        } else {
            ui.label(RichText::new("No layout view").color(pal.text_weak));
        }
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            theme::section(&mut cols[0], "Places (flattened)");
            let flat = flat_counts(u, r);
            if flat.is_empty() {
                cols[0].label(RichText::new("No sub-cells").color(pal.text_weak));
            }
            egui::ScrollArea::vertical().id_salt("hb_places").show(&mut cols[0], |ui| {
                for (k, n) in &flat {
                    if ui.selectable_label(false, format!("{}  ×{n}", k.cell)).clicked() {
                        self.selected = Some(k.clone());
                    }
                }
            });
            theme::section(&mut cols[1], "Used by");
            let parents: BTreeSet<_> = u.parents.get(r).into_iter().flatten().collect();
            if parents.is_empty() {
                cols[1].label(RichText::new("Top cell: not placed anywhere").color(pal.text_weak));
            }
            egui::ScrollArea::vertical().id_salt("hb_used").show(&mut cols[1], |ui| {
                for (k, n) in parents {
                    if ui.selectable_label(false, format!("{}  ×{n}", k.cell)).clicked() {
                        self.selected = Some(k.clone());
                    }
                }
            });
        });
        action
    }
}
