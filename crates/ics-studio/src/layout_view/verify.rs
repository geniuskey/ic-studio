//! Verification panel: rule-deck DRC, connectivity extraction with net highlighting, and LVS.

use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind};
use ics_core::drc::{self, DrcReport};
use ics_core::extract::{self, DevKind, Extraction};
use ics_core::geom::IRect;
use ics_core::layout::Layout;
use ics_core::lvs::{self, LvsErrorKind, LvsReport};

use super::{LayCtx, LayRequest, LayoutEditor, screen_rect, um};
use crate::canvas::View2D;
use crate::theme::{self, Palette};

#[derive(Default)]
pub struct VerifyState {
    /// Deck being edited; `None` until the editor is opened (then it starts from the technology).
    pub deck: Option<String>,
    pub deck_errors: Vec<String>,
    pub drc: Option<DrcReport>,
    pub extraction: Option<Extraction>,
    pub lvs: Option<LvsReport>,
    /// Results no longer reflect the layout.
    pub stale: bool,
    pub highlight: Option<usize>,
    pub pick_nets: bool,
    marker: Option<IRect>,
}

impl VerifyState {
    pub fn layout_changed(&mut self) {
        if self.drc.is_some() || self.extraction.is_some() || self.lvs.is_some() {
            self.stale = true;
        }
        self.highlight = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    Drc,
    Extract,
    Lvs,
}

fn deck_source(cx: &LayCtx) -> String {
    cx.tech.drc_deck.clone().unwrap_or_else(|| drc::default_deck_text(cx.tech))
}

impl LayoutEditor {
    fn zoom_to(&mut self, a: IRect) {
        self.view.center = ((a.x0 + a.x1) as f64 / 2.0, (a.y0 + a.y1) as f64 / 2.0);
        let size = a.width().max(a.height()).max(200) as f64;
        self.view.scale = (300.0 / size).clamp(self.view.min_scale, self.view.max_scale);
        self.verify.marker = Some(a);
    }

    fn run_drc(&mut self, l: &Layout, cx: &mut LayCtx) {
        let text = self.verify.deck.clone().unwrap_or_else(|| deck_source(cx));
        match drc::parse_deck(&text, cx.tech) {
            Ok(deck) => {
                self.verify.deck_errors.clear();
                self.verify.drc = Some(drc::run(&deck, l, cx.src, cx.tech));
                self.verify.stale = false;
            }
            Err(errs) => {
                self.verify.deck_errors = errs.iter().map(|e| format!("line {}: {}", e.line, e.message)).collect();
            }
        }
    }

    fn run_extract(&mut self, l: &Layout, cx: &mut LayCtx) {
        self.verify.extraction = Some(extract::extract(l, cx.src, cx.tech));
        self.verify.highlight = None;
        self.verify.stale = false;
    }

    fn run_lvs(&mut self, l: &Layout, cx: &mut LayCtx) {
        let Some(sch) = cx.src.cell(cx.cell).and_then(|c| c.schematic.as_ref()) else { return };
        self.run_extract(l, cx);
        if let Some(x) = &self.verify.extraction {
            self.verify.lvs = Some(lvs::compare(sch, x, &lvs::LvsOptions::default()));
        }
    }

    /// Run a verification step; returns a one-line summary for the console.
    pub fn run_verification(&mut self, what: Verify, l: &Layout, cx: &mut LayCtx) -> String {
        match what {
            Verify::Drc => {
                self.run_drc(l, cx);
                match (&self.verify.drc, self.verify.deck_errors.first()) {
                    (_, Some(e)) => format!("DRC deck error: {e}"),
                    (Some(r), None) => format!("DRC: {} violation(s) in {:.0} ms", r.violations.len(), r.millis),
                    (None, None) => "DRC did not run".into(),
                }
            }
            Verify::Extract => {
                self.run_extract(l, cx);
                let x = self.verify.extraction.as_ref().expect("just extracted");
                format!("Extracted {} nets and {} transistors", x.nets.len(), x.devices.len())
            }
            Verify::Lvs => {
                self.run_lvs(l, cx);
                match &self.verify.lvs {
                    Some(r) if r.matched => "LVS: layout matches schematic".into(),
                    Some(r) => format!("LVS: {} difference(s)", r.errors.len()),
                    None => "LVS needs a schematic view of this cell".into(),
                }
            }
        }
    }

    /// Click-to-highlight: returns true if the click was consumed.
    pub(super) fn pick_net(&mut self, p: (i64, i64)) -> bool {
        if !self.verify.pick_nets {
            return false;
        }
        if let Some(x) = &self.verify.extraction {
            self.verify.highlight = x.net_at_any(p);
        }
        true
    }

    /// Inspector section with DRC, extraction and LVS.
    pub(super) fn verify_panel(&mut self, ui: &mut egui::Ui, l: &Layout, cx: &mut LayCtx) {
        let pal = theme::palette(ui.ctx());
        let tech = cx.tech;
        let mut zoom: Option<IRect> = None;

        ui.add_space(12.0);
        theme::section(ui, "Design Rule Check");
        ui.horizontal(|ui| {
            if ui.button("▶ Run DRC").on_hover_text("Run the rule deck on the flattened cell").clicked() {
                self.run_drc(l, cx);
            }
            if self.verify.drc.is_some() && ui.button("Clear").clicked() {
                self.verify.drc = None;
                self.verify.marker = None;
            }
        });
        egui::CollapsingHeader::new("Rule deck").id_salt("deck").show(ui, |ui| {
            let deck = self.verify.deck.get_or_insert_with(|| deck_source(cx));
            let lb =
                ui.label(egui::RichText::new("width / space / separation / enclosure / extension / area · µm").small().weak());
            ui.add(egui::TextEdit::multiline(deck).code_editor().desired_rows(10).desired_width(f32::INFINITY))
                .labelled_by(lb.id);
            ui.horizontal(|ui| {
                if ui.button("Save to technology").on_hover_text("Store this deck in the project's tech.json").clicked() {
                    match drc::parse_deck(deck, tech) {
                        Ok(_) => {
                            self.verify.deck_errors.clear();
                            self.requests.push(LayRequest::SetDeck(deck.clone()));
                        }
                        Err(errs) => {
                            self.verify.deck_errors = errs.iter().map(|e| format!("line {}: {}", e.line, e.message)).collect();
                        }
                    }
                }
                if ui.button("Reset").on_hover_text("Rules derived from the technology layers and vias").clicked() {
                    *deck = drc::default_deck_text(tech);
                }
            });
        });
        for e in &self.verify.deck_errors {
            ui.label(egui::RichText::new(e).small().color(pal.danger));
        }
        if let Some(r) = &self.verify.drc {
            let summary = format!(
                "{} rule(s) · {} shapes · {:.0} ms{}",
                r.rules_run,
                r.shapes,
                r.millis,
                if r.skipped > 0 { format!(" · {} non-Manhattan skipped", r.skipped) } else { String::new() }
            );
            ui.label(egui::RichText::new(summary).small().color(pal.text_weak));
            if r.violations.is_empty() {
                ui.colored_label(pal.ok, "✔ No violations");
            } else {
                ui.colored_label(pal.danger, format!("{} violation(s)", r.violations.len()));
                let mut groups: Vec<(&str, Vec<&drc::Violation>)> = vec![];
                for v in &r.violations {
                    match groups.iter_mut().find(|g| g.0 == v.rule) {
                        Some(g) => g.1.push(v),
                        None => groups.push((&v.rule, vec![v])),
                    }
                }
                egui::ScrollArea::vertical().id_salt("drc").max_height(240.0).show(ui, |ui| {
                    for (rule, vs) in groups {
                        egui::CollapsingHeader::new(format!("{rule}  ({})", vs.len())).id_salt(("drc_rule", rule)).show(
                            ui,
                            |ui| {
                                ui.label(egui::RichText::new(&vs[0].description).small().color(pal.text_weak));
                                for v in vs.iter().take(500) {
                                    let m = v.marker;
                                    let txt = format!("at {}, {} µm", um((m.x0 + m.x1) / 2, tech), um((m.y0 + m.y1) / 2, tech));
                                    if ui.add(egui::Button::new(egui::RichText::new(txt).small()).frame(false)).clicked() {
                                        zoom = Some(m);
                                    }
                                }
                            },
                        );
                    }
                });
            }
        }

        ui.add_space(12.0);
        theme::section(ui, "Connectivity");
        ui.horizontal(|ui| {
            if ui.button("Extract").on_hover_text("Find nets and transistors in the flattened cell").clicked() {
                self.run_extract(l, cx);
            }
            ui.add_enabled(self.verify.extraction.is_some(), egui::Checkbox::new(&mut self.verify.pick_nets, "Pick nets"))
                .on_hover_text("Click shapes in the canvas to highlight their net");
        });
        if let Some(x) = &self.verify.extraction {
            let n = x.devices.iter().filter(|d| d.kind == DevKind::Nmos).count();
            let p = x.devices.len() - n;
            ui.label(
                egui::RichText::new(format!("{} nets · {} nmos · {} pmos", x.nets.len(), n, p)).small().color(pal.text_weak),
            );
            for w in x.warnings.iter().take(20) {
                ui.label(egui::RichText::new(w).small().color(pal.warning));
            }
            egui::CollapsingHeader::new("Nets").id_salt("nets").show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("net_list").max_height(160.0).show(ui, |ui| {
                    let mut order: Vec<usize> = (0..x.nets.len()).collect();
                    order.sort_by_key(|&i| (!x.nets[i].labelled, x.nets[i].name.clone()));
                    for i in order.into_iter().take(2000) {
                        let net = &x.nets[i];
                        let on = self.verify.highlight == Some(i);
                        let text = egui::RichText::new(&net.name).small();
                        let text = if net.labelled { text.strong() } else { text };
                        if ui.selectable_label(on, text).clicked() {
                            self.verify.highlight = if on { None } else { Some(i) };
                        }
                    }
                });
            });
            egui::CollapsingHeader::new("Devices").id_salt("devices").show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("dev_list").max_height(160.0).show(ui, |ui| {
                    for d in x.devices.iter().take(2000) {
                        let kind = if d.kind == DevKind::Nmos { "nmos" } else { "pmos" };
                        let net = |i: usize| x.nets.get(i).map_or("?", |n| n.name.as_str());
                        let txt = format!(
                            "{} {kind} W={} L={}  d={} g={} s={}",
                            d.name,
                            um(d.w, tech),
                            um(d.l, tech),
                            net(d.d),
                            net(d.g),
                            net(d.s)
                        );
                        if ui.add(egui::Button::new(egui::RichText::new(txt).small()).frame(false)).clicked() {
                            zoom = Some(d.gate);
                        }
                    }
                });
            });
        }

        ui.add_space(12.0);
        theme::section(ui, "Layout vs Schematic");
        let schematic = cx.src.cell(cx.cell).and_then(|c| c.schematic.as_ref());
        match schematic {
            None => {
                ui.label(egui::RichText::new("This cell has no schematic to compare against").small().color(pal.text_weak));
            }
            Some(_) => {
                if ui.button("▶ Run LVS").on_hover_text("Extract the layout and compare it with the schematic").clicked() {
                    self.run_lvs(l, cx);
                }
            }
        }
        if let Some(r) = &self.verify.lvs {
            ui.label(
                egui::RichText::new(format!(
                    "{} schematic / {} layout devices · {} matched",
                    r.schematic_devices,
                    r.layout_devices,
                    r.devices.len()
                ))
                .small()
                .color(pal.text_weak),
            );
            if r.matched {
                ui.colored_label(pal.ok, "✔ Layout matches schematic");
            } else {
                ui.colored_label(pal.danger, format!("{} difference(s)", r.errors.len()));
                egui::ScrollArea::vertical().id_salt("lvs").max_height(200.0).show(ui, |ui| {
                    for e in &r.errors {
                        let tag = match e.kind {
                            LvsErrorKind::MissingDevice => "missing",
                            LvsErrorKind::ExtraDevice => "extra",
                            LvsErrorKind::Parameter => "param",
                            LvsErrorKind::Net => "net",
                        };
                        let txt = egui::RichText::new(format!("[{tag}] {}", e.message)).small();
                        match e.marker {
                            Some(m) => {
                                if ui.add(egui::Button::new(txt).frame(false)).on_hover_text("Click to zoom").clicked() {
                                    zoom = Some(m);
                                }
                            }
                            None => {
                                ui.label(txt);
                            }
                        }
                    }
                });
            }
        }
        if self.verify.stale {
            ui.label(egui::RichText::new("Results are out of date: the layout changed").small().color(pal.warning));
        }
        if let Some(z) = zoom {
            self.zoom_to(z);
        }
    }

    /// Canvas overlay: DRC markers, the highlighted net and the selected marker.
    pub(super) fn draw_verify(&self, painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette) {
        if let (Some(x), Some(h)) = (&self.verify.extraction, self.verify.highlight)
            && let Some(net) = x.nets.get(h)
        {
            let fill = Color32::from_rgb(255, 210, 60);
            let mut mesh = egui::Mesh::default();
            let mut n = 0;
            for (_, region) in &net.shapes {
                for r in region.rects() {
                    let sr = screen_rect(view, rect, *r);
                    if sr.intersects(rect) && n < 50_000 {
                        mesh.add_colored_rect(sr.intersect(rect.expand(2.0)), fill.gamma_multiply(0.45));
                        n += 1;
                    }
                }
            }
            painter.add(egui::Shape::mesh(mesh));
            painter.text(
                rect.left_top() + egui::vec2(10.0, 8.0),
                egui::Align2::LEFT_TOP,
                format!("net {}", net.name),
                egui::FontId::proportional(13.0),
                fill,
            );
        }
        if let Some(r) = &self.verify.drc {
            for v in r.violations.iter().take(20_000) {
                let sr = screen_rect(view, rect, v.marker).expand(2.0);
                if sr.intersects(rect) {
                    painter.rect_stroke(sr, 1.0, Stroke::new(1.5, pal.danger), StrokeKind::Outside);
                }
            }
        }
        if let Some(m) = self.verify.marker {
            painter.rect_stroke(screen_rect(view, rect, m).expand(6.0), 3.0, Stroke::new(2.0, pal.warning), StrokeKind::Outside);
        }
    }
}
