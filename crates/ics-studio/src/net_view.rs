//! Net Browser: the nets of the active layout's last extraction with their layers, area, device
//! terminals and wire capacitance; picking one highlights it in the layout and zooms to it.

use eframe::egui::{self, RichText};
use ics_core::extract::Extraction;
use ics_core::tech::Tech;
use ics_core::units::format_eng;
use ics_core::{CellRef, Project};

use crate::layout_view::LayoutEditor;
use crate::theme::{self, Palette};

pub struct NetRow {
    pub index: usize,
    pub name: String,
    pub labelled: bool,
    pub layers: String,
    /// Conductor area in µm².
    pub area: f64,
    /// Device terminals (drain, gate, source, bulk) on the net.
    pub terminals: usize,
    /// Wire capacitance to substrate in farads.
    pub cap: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    #[default]
    Name,
    Cap,
    Area,
    Terminals,
}

#[derive(Default)]
pub struct NetBrowser {
    pub open: bool,
    pub filter: String,
    pub sort: SortBy,
    /// Rows of the extraction identified by the key.
    rows: Vec<NetRow>,
    key: Option<(CellRef, usize, usize, usize)>,
}

/// Table rows of an extraction.
pub fn net_rows(x: &Extraction, tech: &Tech) -> Vec<NetRow> {
    let caps = ics_core::pex::net_caps(x, tech);
    let um2 = (tech.dbu_per_um.max(1) as f64).powi(2);
    let mut terminals = vec![0usize; x.nets.len()];
    for d in &x.devices {
        for n in [d.d, d.g, d.s, d.b] {
            if let Some(t) = terminals.get_mut(n) {
                *t += 1;
            }
        }
    }
    x.nets
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut layers: Vec<&str> = n.shapes.iter().filter_map(|(l, _)| tech.layer(*l).map(|d| d.name.as_str())).collect();
            layers.dedup();
            NetRow {
                index: i,
                name: n.name.clone(),
                labelled: n.labelled,
                layers: layers.join(", "),
                area: Some(n.shapes.iter().map(|(_, r)| r.area() as f64).sum::<f64>() / um2).filter(|a| *a > 0.0).unwrap_or(0.0),
                terminals: terminals[i],
                cap: caps.iter().find(|c| c.net == n.name).map_or(0.0, |c| c.farads),
            }
        })
        .collect()
}

pub enum NetAction {
    Extract,
}

impl NetBrowser {
    pub fn rows(&self) -> &[NetRow] {
        &self.rows
    }

    fn refresh(&mut self, cell: &CellRef, x: &Extraction, tech: &Tech) {
        let key = (cell.clone(), x as *const Extraction as usize, x.nets.len(), x.devices.len());
        if self.key.as_ref() != Some(&key) {
            self.rows = net_rows(x, tech);
            self.key = Some(key);
        }
    }

    pub fn window(
        &mut self,
        ctx: &egui::Context,
        project: Option<&Project>,
        cell: Option<&CellRef>,
        ed: Option<&mut LayoutEditor>,
    ) -> Option<NetAction> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        let mut open = true;
        let r = crate::float::tool_window(ctx, "net_browser", "Net Browser", [720.0, 560.0], &mut open, |ui| {
            let pal = theme::palette(ui.ctx());
            let mut action = None;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                action = self.ui(ui, &project.tech, cell, ed, &pal);
            });
            action
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        tech: &Tech,
        cell: Option<&CellRef>,
        ed: Option<&mut LayoutEditor>,
        pal: &Palette,
    ) -> Option<NetAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Net Browser").strong().color(pal.text_strong));
            ui.label(RichText::new(cell.map_or("· open a layout cell".into(), |c| format!("· {c}"))).color(pal.text_weak));
            ui.separator();
            let busy = ed.as_ref().is_some_and(|e| e.is_verifying());
            if ui
                .add_enabled(cell.is_some() && !busy, egui::Button::new("Extract"))
                .on_hover_text("Extract the nets again")
                .clicked()
            {
                action = Some(NetAction::Extract);
            }
            if busy {
                ui.spinner();
            }
        });
        let (Some(cell), Some(ed)) = (cell, ed) else {
            ui.label(RichText::new("Open a layout cell.").color(pal.text_weak));
            return action;
        };
        let Some(x) = ed.verify.extraction.as_ref() else {
            ui.label(RichText::new("No extraction yet: press Extract.").color(pal.text_weak));
            return action;
        };
        self.refresh(cell, x, tech);
        if ed.verify.stale {
            ui.label(RichText::new("The layout changed since this extraction.").small().color(pal.warning));
        }
        ui.horizontal(|ui| {
            let l = ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(180.0).hint_text("vdd, net1…")).labelled_by(l.id);
            let l = ui.label("Sort by");
            egui::ComboBox::from_id_salt("nb_sort")
                .selected_text(match self.sort {
                    SortBy::Name => "Name",
                    SortBy::Cap => "Capacitance",
                    SortBy::Area => "Area",
                    SortBy::Terminals => "Terminals",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.sort, SortBy::Name, "Name");
                    ui.selectable_value(&mut self.sort, SortBy::Cap, "Capacitance");
                    ui.selectable_value(&mut self.sort, SortBy::Area, "Area");
                    ui.selectable_value(&mut self.sort, SortBy::Terminals, "Terminals");
                })
                .response
                .labelled_by(l.id);
            ui.label(RichText::new(format!("{} nets · {} devices", x.nets.len(), x.devices.len())).small().color(pal.text_weak));
        });
        let f = self.filter.to_ascii_lowercase();
        let mut order: Vec<&NetRow> =
            self.rows.iter().filter(|r| f.is_empty() || r.name.to_ascii_lowercase().contains(&f)).collect();
        match self.sort {
            SortBy::Name => order.sort_by(|a, b| (!a.labelled, &a.name).cmp(&(!b.labelled, &b.name))),
            SortBy::Cap => order.sort_by(|a, b| b.cap.total_cmp(&a.cap)),
            SortBy::Area => order.sort_by(|a, b| b.area.total_cmp(&a.area)),
            SortBy::Terminals => order.sort_by_key(|r| std::cmp::Reverse(r.terminals)),
        }
        let mut pick = None;
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("nb_grid").striped(true).num_columns(5).spacing([18.0, 4.0]).show(ui, |ui| {
                for h in ["NET", "LAYERS", "AREA", "TERMINALS", "WIRE CAP"] {
                    ui.label(RichText::new(h).small().strong().color(pal.text_weak));
                }
                ui.end_row();
                for r in order {
                    let on = ed.verify.highlight == Some(r.index);
                    let text = RichText::new(&r.name).monospace();
                    let text = if r.labelled { text.strong() } else { text.color(pal.text_weak) };
                    if ui.selectable_label(on, text).on_hover_text("Highlight and zoom").clicked() {
                        pick = Some(r.index);
                    }
                    ui.label(RichText::new(&r.layers).small());
                    ui.label(format!("{:.3} µm²", r.area));
                    ui.label(r.terminals.to_string());
                    ui.label(format_eng(r.cap, "F"));
                    ui.end_row();
                }
            });
        });
        if let Some(k) = pick {
            ed.show_net(k);
        }
        action
    }
}
