//! Layer Display editor, after Virtuoso's Display Resource Editor: each layer's display packet
//! (fill colour, outline colour, fill style, stipple bitmap and line style) with a live preview.
//! Edits are a draft until applied to the technology.

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use ics_core::Project;
use ics_core::tech::{FillStyle, LayerDef, LineStyle, Stipple, Tech};

use crate::layout_view::packet::Packet;
use crate::theme::{self, Palette};

/// Largest stipple bitmap edited, in bits per side.
const MAX_BITS: usize = 32;

#[derive(Default)]
pub struct DisplayEditor {
    pub open: bool,
    layers: Vec<LayerDef>,
    baseline: Vec<LayerDef>,
    sel: usize,
    filter: String,
    /// Value painted by the current drag over the bitmap.
    paint: Option<bool>,
    status: Option<(bool, String)>,
}

/// The display fields of a layer.
fn display_of(l: &LayerDef) -> ([u8; 4], FillStyle, Option<Stipple>, Option<[u8; 3]>, LineStyle) {
    (l.color, l.fill, l.stipple.clone(), l.outline, l.line)
}

/// Copy the display packet of `from` to the layers of `tech` with the same name; returns how
/// many layers changed.
pub fn apply_display(tech: &mut Tech, from: &[LayerDef]) -> usize {
    let mut n = 0;
    for l in &mut tech.layers {
        if let Some(d) = from.iter().find(|d| d.name == l.name)
            && display_of(l) != display_of(d)
        {
            (l.color, l.fill, l.stipple, l.outline, l.line) = display_of(d);
            n += 1;
        }
    }
    n
}

impl DisplayEditor {
    /// Open the editor on the technology, on layer `layer` when given.
    pub fn show(&mut self, tech: &Tech, layer: Option<&str>) {
        let names = |v: &[LayerDef]| v.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
        if !self.open || names(&self.layers) != names(&tech.layers) {
            self.layers = tech.layers.clone();
            self.baseline = self.layers.clone();
            self.status = None;
            self.sel = 0;
        }
        if let Some(i) = layer.and_then(|n| self.layers.iter().position(|l| l.name == n)) {
            self.sel = i;
        }
        self.open = true;
    }

    /// The layer being edited (draft).
    pub fn current(&mut self) -> Option<&mut LayerDef> {
        self.layers.get_mut(self.sel)
    }

    /// The editor window; returns the layers whose display to apply to the technology.
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>) -> Option<Vec<LayerDef>> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        if self.layers.len() != project.tech.layers.len() {
            self.open = false;
            self.show(&project.tech, None);
        }
        let mut open = true;
        let r = crate::float::tool_window(ctx, "layer_display", "Layer Display", [820.0, 600.0], &mut open, |ui| {
            self.ui(ui, &project.tech)
        });
        self.open &= open;
        r.unwrap_or_default()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tech: &Tech) -> Option<Vec<LayerDef>> {
        let pal = theme::palette(ui.ctx());
        let mut apply = None;
        egui::Panel::top("ld_toolbar")
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| apply = self.toolbar(ui, tech, &pal));
        egui::Panel::left("ld_layers")
            .resizable(true)
            .default_size(220.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| self.layer_list(ui, &pal));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("ld_packet").auto_shrink([false, false]).show(ui, |ui| self.packet(ui, &pal));
        });
        apply
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) -> Option<Vec<LayerDef>> {
        let mut apply = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Layer Display").strong().color(pal.text_strong));
            ui.label(RichText::new(format!("· {}", tech.name)).color(pal.text_weak));
            let changed = self.layers != self.baseline;
            if changed {
                ui.label(RichText::new("● not applied").color(pal.warning));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new("Apply").strong().color(theme::on_fill(pal.accent))).fill(pal.accent);
                if ui.add_enabled(changed, b).clicked() {
                    self.baseline = self.layers.clone();
                    self.status = Some((true, "Applied to the technology".into()));
                    apply = Some(self.layers.clone());
                }
                if ui.add_enabled(changed, egui::Button::new("Revert")).on_hover_text("Discard the edits not applied").clicked() {
                    self.layers = self.baseline.clone();
                    self.status = None;
                }
                if let Some((ok, s)) = &self.status {
                    ui.label(RichText::new(s).small().color(if *ok { pal.ok } else { pal.danger }));
                }
            });
        });
        apply
    }

    fn layer_list(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.horizontal(|ui| {
            let fl = ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("layer name").desired_width(f32::INFINITY))
                .labelled_by(fl.id);
        });
        ui.add_space(4.0);
        let filter = self.filter.trim().to_lowercase();
        egui::ScrollArea::vertical().id_salt("ld_list").auto_shrink([false, false]).show(ui, |ui| {
            for i in 0..self.layers.len() {
                let def = &self.layers[i];
                if !filter.is_empty() && !def.name.to_lowercase().contains(&filter) {
                    continue;
                }
                let (r, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), 24.0), Sense::click());
                let r = r.on_hover_text(format!("{}  GDS {}/{}", def.name, def.gds_layer, def.gds_datatype));
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, i == self.sel, &def.name));
                if i == self.sel {
                    painter.rect_filled(r.rect, 4.0, pal.accent.gamma_multiply(0.22));
                } else if r.hovered() {
                    painter.rect_filled(r.rect, 4.0, pal.bg_hover);
                }
                let sw = Rect::from_min_size(r.rect.min + Vec2::new(6.0, 4.0), Vec2::new(26.0, 16.0));
                swatch(&painter, sw, def, pal);
                let edited = self.baseline.get(i).is_some_and(|b| display_of(b) != display_of(def));
                let name = if edited { format!("{} •", def.name) } else { def.name.clone() };
                painter.text(
                    sw.right_center() + Vec2::new(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(13.0),
                    pal.text,
                );
                if r.clicked() {
                    self.sel = i;
                }
            }
        });
    }

    fn packet(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(def) = self.layers.get(self.sel).cloned() else {
            ui.label("The technology has no layers.");
            return;
        };
        ui.label(RichText::new(&def.name).heading().color(pal.text_strong));
        ui.add_space(6.0);
        preview(ui, &def, pal);
        ui.add_space(10.0);

        let mut d = def.clone();
        egui::Grid::new("ld_colours").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            let l = ui.label("Fill colour");
            let mut rgb = [d.color[0], d.color[1], d.color[2]];
            if ui.color_edit_button_srgb(&mut rgb).labelled_by(l.id).changed() {
                d.color = [rgb[0], rgb[1], rgb[2], d.color[3]];
            }
            ui.end_row();
            let l = ui.label("Outline colour");
            ui.horizontal(|ui| {
                let mut same = d.outline.is_none();
                if ui.checkbox(&mut same, "Same as fill").changed() {
                    d.outline = if same { None } else { Some([d.color[0], d.color[1], d.color[2]]) };
                }
                if let Some(o) = &mut d.outline {
                    ui.color_edit_button_srgb(o).labelled_by(l.id);
                }
            });
            ui.end_row();
        });

        ui.add_space(10.0);
        theme::section(ui, "Fill style");
        ui.horizontal_wrapped(|ui| {
            for f in FillStyle::ALL {
                let sample = LayerDef { fill: f, stipple: None, ..d.clone() };
                let on = d.fill == f;
                if option(ui, &sample, on, f.name(), pal).clicked() && !on {
                    d.fill = f;
                    d.stipple = None;
                }
            }
        });

        if d.fill.is_stipple() {
            ui.add_space(10.0);
            theme::section(ui, "Stipple");
            ui.label(
                RichText::new(if d.stipple.is_some() {
                    "Custom bitmap. Click or drag over the bits to change them."
                } else {
                    "Built-in bitmap. Click or drag over the bits to make a custom one."
                })
                .small()
                .color(pal.text_weak),
            );
            self.stipple_editor(ui, &mut d, pal);
        }

        ui.add_space(10.0);
        theme::section(ui, "Outline");
        ui.horizontal_wrapped(|ui| {
            for l in LineStyle::ALL {
                let sample = LayerDef { fill: FillStyle::Outline, line: l, ..d.clone() };
                if option(ui, &sample, d.line == l, l.name(), pal).clicked() {
                    d.line = l;
                }
            }
        });

        if d != def {
            self.layers[self.sel] = d;
            self.status = None;
        }
    }

    fn stipple_editor(&mut self, ui: &mut egui::Ui, d: &mut LayerDef, pal: &Palette) {
        let Some(base) = d.pattern() else { return };
        let mut bits = base.bits();
        let (mut w, mut h) = (base.width(), base.height());
        let mut changed = false;
        ui.horizontal(|ui| {
            let lw = ui.label("Width");
            changed |= ui.add(egui::DragValue::new(&mut w).range(1..=MAX_BITS)).labelled_by(lw.id).changed();
            let lh = ui.label("Height");
            changed |= ui.add(egui::DragValue::new(&mut h).range(1..=MAX_BITS)).labelled_by(lh.id).changed();
            if ui.button("Invert").clicked() {
                bits.iter_mut().flatten().for_each(|b| *b ^= 1);
                changed = true;
            }
            if ui.button("Clear").clicked() {
                bits.iter_mut().flatten().for_each(|b| *b = 0);
                changed = true;
            }
            if ui
                .add_enabled(d.stipple.is_some(), egui::Button::new("Built-in"))
                .on_hover_text("Use the fill style's own bitmap")
                .clicked()
            {
                d.stipple = None;
            }
        });
        if changed {
            bits.resize(h, vec![]);
            for row in &mut bits {
                row.resize(w, 0);
            }
        }
        let cell = (220.0 / w.max(h) as f32).clamp(6.0, 16.0);
        let size = Vec2::new(w as f32, h as f32) * cell;
        let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Stipple bitmap"));
        let fill = crate::layout_view::layer_color(d, pal.dark);
        painter.rect_filled(resp.rect, 0.0, pal.bg_canvas);
        for (y, row) in bits.iter().enumerate() {
            for (x, &b) in row.iter().enumerate() {
                let r = Rect::from_min_size(resp.rect.min + Vec2::new(x as f32, y as f32) * cell, Vec2::splat(cell));
                if b == 1 {
                    painter.rect_filled(r.shrink(0.5), 0.0, fill);
                }
                painter.rect_stroke(r, 0.0, Stroke::new(0.5, pal.border), StrokeKind::Inside);
            }
        }
        let at = |p: Pos2| {
            let v = (p - resp.rect.min) / cell;
            (v.x >= 0.0 && v.y >= 0.0 && (v.x as usize) < w && (v.y as usize) < h).then_some((v.x as usize, v.y as usize))
        };
        if resp.is_pointer_button_down_on()
            && let Some((x, y)) = resp.interact_pointer_pos().and_then(at)
        {
            let v = *self.paint.get_or_insert(bits[y][x] == 0);
            if (bits[y][x] == 1) != v {
                bits[y][x] = v as u8;
                changed = true;
            }
        } else {
            self.paint = None;
        }
        if changed && let Some(s) = Stipple::from_bits(&bits) {
            d.stipple = Some(s);
        }
    }
}

/// A packet sample in `r` on the canvas colour.
fn swatch(painter: &egui::Painter, r: Rect, def: &LayerDef, pal: &Palette) {
    painter.rect_filled(r, 2.0, pal.bg_canvas);
    let pk = Packet::new(painter.ctx(), def, pal.dark);
    painter.add(pk.rect_shape(r, 1.0));
}

/// A selectable sample of a fill or line style.
fn option(ui: &mut egui::Ui, sample: &LayerDef, on: bool, name: &str, pal: &Palette) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(Vec2::new(46.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name));
    let frame = if on {
        pal.accent
    } else if resp.hovered() {
        pal.text_weak
    } else {
        pal.border
    };
    painter.rect_stroke(resp.rect, 4.0, Stroke::new(if on { 2.0 } else { 1.0 }, frame), StrokeKind::Inside);
    swatch(&painter, resp.rect.shrink(5.0), sample, pal);
    resp.on_hover_text(name)
}

/// Large preview: the layer over a second shape of itself, as in a layout.
fn preview(ui: &mut egui::Ui, def: &LayerDef, pal: &Palette) {
    let (resp, painter) = ui.allocate_painter(Vec2::new(ui.available_width().min(360.0), 110.0), Sense::hover());
    let r = resp.rect;
    painter.rect_filled(r, 4.0, pal.bg_canvas);
    let pk = Packet::new(painter.ctx(), def, pal.dark);
    let a = Rect::from_min_size(r.min + Vec2::new(14.0, 14.0), Vec2::new(r.width() * 0.5, 66.0));
    let b = Rect::from_min_size(r.min + Vec2::new(r.width() * 0.4, 34.0), Vec2::new(r.width() * 0.5, 62.0));
    let mut lp = pk.begin();
    lp.rect(a, 1.0);
    lp.rect(b, 1.0);
    lp.polygon(
        &[
            Pos2::new(r.max.x - 40.0, r.min.y + 10.0),
            Pos2::new(r.max.x - 12.0, r.min.y + 10.0),
            Pos2::new(r.max.x - 12.0, r.min.y + 30.0),
        ],
        1.0,
    );
    painter.add(lp.finish());
}
