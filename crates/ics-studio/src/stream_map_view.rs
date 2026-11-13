//! Stream Layer Map editor (Virtuoso XStream layer map): the stream (layer, datatype) pairs the
//! technology layers are read from and written as on File > Import / Export.

use eframe::egui::{self, RichText};
use ics_core::Project;
use ics_core::stream_map;
use ics_core::tech::{StreamMapEntry, Tech};

use crate::theme::{self, Palette};

#[derive(Default)]
pub struct StreamMapEditor {
    pub open: bool,
    pub rows: Vec<StreamMapEntry>,
    baseline: Vec<StreamMapEntry>,
    status: Option<(bool, String)>,
}

impl StreamMapEditor {
    pub fn show(&mut self, tech: &Tech) {
        if !self.open {
            self.rows = tech.stream_map.clone();
            self.baseline = self.rows.clone();
            self.status = None;
        }
        self.open = true;
    }

    /// Replace the draft with the rows of a layer map file; returns the number of rows read.
    pub fn load_text(&mut self, text: &str, tech: &Tech) -> usize {
        let (rows, warnings) = stream_map::parse(text, tech);
        let n = rows.len();
        self.rows = rows;
        self.status = Some(match warnings.first() {
            None => (true, format!("Read {n} row(s)")),
            Some(w) => (false, format!("Read {n} row(s), skipped {} line(s): {w}", warnings.len())),
        });
        n
    }

    fn draft(&self, tech: &Tech) -> Tech {
        Tech { stream_map: self.rows.clone(), ..tech.clone() }
    }

    /// The editor window; returns the rows to apply to the technology.
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>) -> Option<Vec<StreamMapEntry>> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        let builder = egui::ViewportBuilder::default()
            .with_title("Stream Layer Map")
            .with_inner_size([860.0, 640.0])
            .with_min_inner_size([560.0, 380.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("stream_map"), builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                self.open = false;
            }
            self.ui(ui, &project.tech)
        })
    }

    fn ui(&mut self, ui: &mut egui::Ui, tech: &Tech) -> Option<Vec<StreamMapEntry>> {
        let pal = theme::palette(ui.ctx());
        let mut apply = None;
        egui::Panel::top("sm_toolbar")
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| apply = self.toolbar(ui, tech, &pal));
        let draft = self.draft(tech);
        egui::Panel::right("sm_preview")
            .resizable(true)
            .default_size(280.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| preview(ui, &draft, &pal));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            let problems = stream_map::problems(&draft);
            for p in &problems {
                ui.label(RichText::new(p).small().color(pal.warning));
            }
            if !problems.is_empty() {
                ui.add_space(4.0);
            }
            self.table(ui, tech, &pal);
        });
        apply
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) -> Option<Vec<StreamMapEntry>> {
        let mut apply = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Stream Layer Map").strong().color(pal.text_strong));
            ui.label(RichText::new(format!("· {}", tech.name)).color(pal.text_weak));
            if self.rows != self.baseline {
                ui.label(RichText::new("● not applied").color(pal.warning));
            }
            ui.separator();
            if ui
                .button("Load map file…")
                .on_hover_text("Read a Virtuoso layer map (layer purpose streamLayer datatype)")
                .clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Layer map", &["map", "layermap", "txt"]).pick_file()
            {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        self.load_text(&text, tech);
                    }
                    Err(e) => self.status = Some((false, format!("Cannot read {}: {e}", path.display()))),
                }
            }
            if ui.button("Save map file…").clicked()
                && let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.layermap", tech.name)).save_file()
            {
                let text = stream_map::write(&self.draft(tech));
                self.status = Some(match std::fs::write(&path, text) {
                    Ok(()) => (true, format!("Wrote {}", path.display())),
                    Err(e) => (false, format!("Cannot write {}: {e}", path.display())),
                });
            }
            if ui.button("From technology").on_hover_text("One row per layer with the technology's own stream numbers").clicked()
            {
                self.rows = tech
                    .layers
                    .iter()
                    .map(|l| StreamMapEntry { layer: l.name.clone(), stream_layer: l.gds_layer, datatype: l.gds_datatype })
                    .collect();
            }
            if ui.button("Clear").on_hover_text("No map: layers use their own stream numbers").clicked() {
                self.rows.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let changed = self.rows != self.baseline;
                let b = egui::Button::new(RichText::new("Apply").strong().color(theme::on_fill(pal.accent))).fill(pal.accent);
                if ui.add_enabled(changed, b).clicked() {
                    self.baseline = self.rows.clone();
                    self.status = Some((true, "Applied to the technology".into()));
                    apply = Some(self.rows.clone());
                }
                if let Some((ok, s)) = &self.status {
                    ui.label(RichText::new(s).small().color(if *ok { pal.ok } else { pal.danger }));
                }
            });
        });
        apply
    }

    fn table(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        let mut remove = None;
        egui::ScrollArea::vertical().id_salt("sm_rows").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("sm_grid").striped(true).num_columns(4).spacing([14.0, 4.0]).show(ui, |ui| {
                let head = |ui: &mut egui::Ui, t: &str| ui.label(RichText::new(t).small().strong().color(pal.text_weak));
                let hl = head(ui, "LAYER");
                let hs = head(ui, "STREAM LAYER");
                let hd = head(ui, "DATATYPE");
                head(ui, "");
                ui.end_row();
                for (i, row) in self.rows.iter_mut().enumerate() {
                    let shown = if row.layer.is_empty() { "(drop on import)".to_string() } else { row.layer.clone() };
                    let known = row.layer.is_empty() || tech.layer_index(&row.layer).is_some();
                    egui::ComboBox::from_id_salt(("sm_layer", i))
                        .width(170.0)
                        .selected_text(if known { RichText::new(shown) } else { RichText::new(shown).color(pal.danger) })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut row.layer, String::new(), "(drop on import)");
                            for l in &tech.layers {
                                ui.selectable_value(&mut row.layer, l.name.clone(), l.name.as_str());
                            }
                        })
                        .response
                        .labelled_by(hl.id);
                    ui.add(egui::DragValue::new(&mut row.stream_layer).range(0..=u16::MAX)).labelled_by(hs.id);
                    ui.add(egui::DragValue::new(&mut row.datatype).range(0..=u16::MAX)).labelled_by(hd.id);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if self.rows.is_empty() {
                ui.label(RichText::new("No map rows: every layer uses its own stream numbers.").color(pal.text_weak));
            }
            ui.add_space(6.0);
            if ui.button("+ Row").clicked() {
                let layer = tech.layers.first().map(|l| l.name.clone()).unwrap_or_default();
                self.rows.push(StreamMapEntry { layer, stream_layer: 0, datatype: 0 });
            }
        });
        if let Some(i) = remove {
            self.rows.remove(i);
        }
    }
}

fn preview(ui: &mut egui::Ui, draft: &Tech, pal: &Palette) {
    theme::section(ui, "Written as");
    ui.label(RichText::new("Stream numbers each layer gets on export").small().color(pal.text_weak));
    let pairs = stream_map::export_pairs(draft);
    egui::ScrollArea::vertical().id_salt("sm_preview_scroll").show(ui, |ui| {
        egui::Grid::new("sm_preview_grid").striped(true).num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
            for (l, (g, d)) in draft.layers.iter().zip(pairs) {
                let changed = (g, d) != (l.gds_layer, l.gds_datatype);
                ui.label(RichText::new(&l.name).monospace());
                ui.label(RichText::new(format!("{g}/{d}")).monospace().color(if changed { pal.accent } else { pal.text_weak }));
                ui.end_row();
            }
        });
    });
}
