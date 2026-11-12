//! Verification panel: rule-deck DRC, connectivity extraction with net highlighting, and LVS.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind};
use ics_core::drc::{self, Deck, DrcReport};
use ics_core::extract::{self, DevKind, Extraction};
use ics_core::geom::IRect;
use ics_core::layout::Layout;
use ics_core::lvs::{self, LvsErrorKind, LvsReport};
use ics_core::signoff::{self, Runset, SignoffReport};
use ics_core::{Project, Schematic};
use ics_mcp::tools::{self, Verified};

use super::{LayCtx, LayRequest, LayoutEditor, screen_rect, um};
use crate::canvas::View2D;
use crate::theme::{self, Palette};

static GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// A verification step running on a worker thread against a snapshot of the design.
struct Job {
    what: Verify,
    started: Instant,
    generation: u64,
    rx: Receiver<Verified>,
}

pub struct VerifyState {
    /// Deck being edited; `None` until the editor is opened (then it starts from the technology).
    pub deck: Option<String>,
    pub deck_errors: Vec<String>,
    /// SVRF translation notes of the deck last run.
    pub deck_notes: Vec<String>,
    pub drc: Option<DrcReport>,
    pub extraction: Option<Extraction>,
    pub lvs: Option<LvsReport>,
    /// Last external sign-off run, or why it could not run.
    pub signoff: Option<Result<SignoffReport, String>>,
    /// Selected runset of the technology.
    pub runset: usize,
    /// Results no longer reflect the layout.
    pub stale: bool,
    pub highlight: Option<usize>,
    pub pick_nets: bool,
    marker: Option<IRect>,
    /// Changes whenever the layout does; unique across editors.
    generation: u64,
    job: Option<Job>,
}

impl Default for VerifyState {
    fn default() -> Self {
        Self {
            deck: None,
            deck_errors: vec![],
            deck_notes: vec![],
            drc: None,
            extraction: None,
            lvs: None,
            signoff: None,
            runset: 0,
            stale: false,
            highlight: None,
            pick_nets: false,
            marker: None,
            generation: next_generation(),
            job: None,
        }
    }
}

impl VerifyState {
    pub fn layout_changed(&mut self) {
        if self.drc.is_some() || self.extraction.is_some() || self.lvs.is_some() || self.signoff.is_some() {
            self.stale = true;
        }
        self.highlight = None;
        self.generation = next_generation();
    }

    /// Identifies the current layout state; results computed for another generation are stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The step running in the background and for how long.
    pub fn running(&self) -> Option<(Verify, Duration)> {
        self.job.as_ref().map(|j| (j.what, j.started.elapsed()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    Drc,
    Extract,
    Lvs,
    /// An external runset of the technology ([`VerifyState::runset`]).
    Signoff,
}

impl Verify {
    pub fn name(self) -> &'static str {
        match self {
            Verify::Drc => "DRC",
            Verify::Extract => "extraction",
            Verify::Lvs => "LVS",
            Verify::Signoff => "sign-off",
        }
    }
}

enum Task {
    Drc(Deck),
    Extract,
    Lvs(Schematic),
    Signoff(Runset, ics_core::CellRef),
}

impl Task {
    fn run(self, top: &Layout, src: &Project) -> Verified {
        match self {
            Task::Drc(deck) => Verified::Drc(drc::run(&deck, top, src, &src.tech)),
            Task::Extract => Verified::Extract(extract::extract(top, src, &src.tech)),
            Task::Lvs(sch) => {
                let x = extract::extract(top, src, &src.tech);
                let r = lvs::compare(&sch, &x, &lvs::LvsOptions::for_tech(&src.tech));
                Verified::Lvs(x, r)
            }
            Task::Signoff(rs, cell) => {
                let mut p = src.clone();
                if let Some(c) = p.cell_mut(&cell) {
                    c.layout = Some(top.clone());
                }
                Verified::Signoff(signoff::run(&p, &cell, &rs, &signoff::Tools::default()))
            }
        }
    }
}

fn deck_source(cx: &LayCtx) -> String {
    cx.tech.drc_deck.clone().unwrap_or_else(|| drc::default_deck_text(cx.tech))
}

fn deck_errors(errs: &[drc::DeckError]) -> Vec<String> {
    errs.iter().map(|e| format!("line {}: {}", e.line, e.message)).collect()
}

impl LayoutEditor {
    fn zoom_to(&mut self, a: IRect) {
        self.push_view();
        self.view.center = ((a.x0 + a.x1) as f64 / 2.0, (a.y0 + a.y1) as f64 / 2.0);
        let size = a.width().max(a.height()).max(200) as f64;
        self.view.scale = (300.0 / size).clamp(self.view.min_scale, self.view.max_scale);
        self.verify.marker = Some(a);
    }

    /// Start a verification step on a worker thread, on a copy of the cell and the cells it
    /// places. Returns a one-line message for the console; [`Self::poll_verification`]
    /// installs the result.
    pub fn start_verification(&mut self, what: Verify, l: &Layout, cx: &LayCtx, ctx: &egui::Context) -> String {
        if let Some((w, _)) = self.verify.running() {
            return format!("{} is still running", w.name());
        }
        let task = match what {
            Verify::Drc => {
                let text = self.verify.deck.clone().unwrap_or_else(|| deck_source(cx));
                match drc::parse_any(&text, cx.tech) {
                    Ok(parsed) => {
                        self.verify.deck_errors.clear();
                        self.verify.deck_notes = parsed.notes;
                        Task::Drc(parsed.deck)
                    }
                    Err(errs) => {
                        self.verify.deck_errors = deck_errors(&errs);
                        return format!("DRC deck error: {}", self.verify.deck_errors[0]);
                    }
                }
            }
            Verify::Extract => Task::Extract,
            Verify::Lvs => match cx.src.cell(cx.cell).and_then(|c| c.schematic.clone()) {
                Some(sch) => Task::Lvs(sch),
                None => return "LVS needs a schematic view of this cell".into(),
            },
            Verify::Signoff => match cx.tech.signoff.get(self.verify.runset) {
                Some(rs) => Task::Signoff(rs.clone(), cx.cell.clone()),
                None => return "The technology has no sign-off runsets".into(),
            },
        };
        let mut snap = tools::snapshot(cx.src, Some(cx.cell), Some(l));
        let top = snap.cell_mut(cx.cell).and_then(|c| c.layout.take()).unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(task.run(&top, &snap));
            ctx.request_repaint();
        });
        self.verify.job = Some(Job { what, started: Instant::now(), generation: self.verify.generation, rx });
        format!("Running {} on {}", what.name(), cx.cell)
    }

    pub fn is_verifying(&self) -> bool {
        self.verify.job.is_some()
    }

    /// Install the result of a finished background step; returns its console summary.
    pub fn poll_verification(&mut self) -> Option<String> {
        let job = self.verify.job.as_ref()?;
        let v = match job.rx.try_recv() {
            Ok(v) => v,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                let what = job.what;
                self.verify.job = None;
                return Some(format!("{} failed with an internal error", what.name()));
            }
        };
        let generation = self.verify.job.take()?.generation;
        Some(self.install_verified(v, generation))
    }

    /// Show results computed for layout `generation`; returns a one-line summary.
    pub fn install_verified(&mut self, v: Verified, generation: u64) -> String {
        let msg = match v {
            Verified::Drc(r) => {
                self.verify.deck_errors.clear();
                let msg = format!("DRC: {} violation(s) in {:.0} ms", r.violations.len(), r.millis);
                self.verify.drc = Some(r);
                msg
            }
            Verified::Extract(x) => {
                let msg = format!("Extracted {} nets and {} transistors", x.nets.len(), x.devices.len());
                self.verify.extraction = Some(x);
                msg
            }
            Verified::Lvs(x, r) => {
                self.verify.extraction = Some(x);
                let msg = if r.matched {
                    "LVS: layout matches schematic".to_string()
                } else {
                    format!("LVS: {} difference(s)", r.errors.len())
                };
                self.verify.lvs = Some(r);
                msg
            }
            Verified::Signoff(r) => {
                let msg = match &r {
                    Ok(rep) => {
                        if let Some(d) = &rep.drc {
                            self.verify.drc = Some(d.clone());
                        }
                        rep.summary()
                    }
                    Err(e) => format!("Sign-off failed: {}", e.lines().next().unwrap_or_default()),
                };
                self.verify.signoff = Some(r);
                msg
            }
        };
        self.verify.highlight = None;
        self.verify.stale = generation != self.verify.generation;
        if self.verify.stale { format!("{msg} (the layout changed while it ran)") } else { msg }
    }

    /// Progress row while `what` runs. Cancel discards the result; the worker finishes on its own.
    fn job_row(&mut self, ui: &mut egui::Ui, what: Verify) {
        let Some((w, t)) = self.verify.running().filter(|(w, _)| *w == what) else { return };
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new());
            ui.label(egui::RichText::new(format!("Running {}… {:.1} s", w.name(), t.as_secs_f64())).small());
            if ui.small_button("Cancel").clicked() {
                self.verify.job = None;
            }
        });
        ui.ctx().request_repaint_after(Duration::from_millis(100));
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
        let idle = !self.is_verifying();

        ui.add_space(12.0);
        theme::section(ui, "Design Rule Check");
        ui.horizontal(|ui| {
            let run = ui.add_enabled(idle, egui::Button::new("▶ Run DRC"));
            if run.on_hover_text("Run the rule deck on the flattened cell").clicked() {
                self.start_verification(Verify::Drc, l, cx, ui.ctx());
            }
            if self.verify.drc.is_some() && ui.button("Clear").clicked() {
                self.verify.drc = None;
                self.verify.marker = None;
            }
        });
        egui::CollapsingHeader::new("Rule deck").id_salt("deck").show(ui, |ui| {
            let deck = self.verify.deck.get_or_insert_with(|| deck_source(cx));
            let lb =
                ui.label(egui::RichText::new("width / space / separation / enclosure / extension / area / antenna / density · µm · derived: name = a and|or|not|xor b, name = a size|wide d").small().weak());
            ui.add(egui::TextEdit::multiline(deck).code_editor().desired_rows(10).desired_width(f32::INFINITY))
                .labelled_by(lb.id);
            ui.horizontal(|ui| {
                if ui.button("Save to technology").on_hover_text("Store this deck in the project's tech.json").clicked() {
                    match drc::parse_any(deck, tech) {
                        Ok(_) => {
                            self.verify.deck_errors.clear();
                            self.requests.push(LayRequest::SetDeck(deck.clone()));
                        }
                        Err(errs) => self.verify.deck_errors = deck_errors(&errs),
                    }
                }
                if ui.button("Reset").on_hover_text("Rules derived from the technology layers and vias").clicked() {
                    *deck = drc::default_deck_text(tech);
                }
            });
        });
        self.job_row(ui, Verify::Drc);
        if !self.verify.deck_notes.is_empty() {
            egui::CollapsingHeader::new(format!("SVRF notes ({})", self.verify.deck_notes.len().saturating_sub(1)))
                .id_salt("deck_notes")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("deck_notes_scroll").max_height(160.0).show(ui, |ui| {
                        for n in &self.verify.deck_notes {
                            ui.label(egui::RichText::new(n).small().color(pal.text_weak));
                        }
                    });
                });
        }
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
            let run = ui.add_enabled(idle, egui::Button::new("Extract"));
            if run.on_hover_text("Find nets and transistors in the flattened cell").clicked() {
                self.start_verification(Verify::Extract, l, cx, ui.ctx());
            }
            ui.add_enabled(self.verify.extraction.is_some(), egui::Checkbox::new(&mut self.verify.pick_nets, "Pick nets"))
                .on_hover_text("Click shapes in the canvas to highlight their net");
        });
        self.job_row(ui, Verify::Extract);
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
                let run = ui.add_enabled(idle, egui::Button::new("▶ Run LVS"));
                if run.on_hover_text("Extract the layout and compare it with the schematic").clicked() {
                    self.start_verification(Verify::Lvs, l, cx, ui.ctx());
                }
            }
        }
        self.job_row(ui, Verify::Lvs);
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
        if !tech.signoff.is_empty() {
            ui.add_space(12.0);
            theme::section(ui, "Sign-off");
            self.verify.runset = self.verify.runset.min(tech.signoff.len() - 1);
            let label = ui.label(egui::RichText::new("Runset").small());
            egui::ComboBox::from_id_salt("runset")
                .width(ui.available_width() - 8.0)
                .selected_text(tech.signoff[self.verify.runset].name.as_str())
                .show_ui(ui, |ui| {
                    for (i, rs) in tech.signoff.iter().enumerate() {
                        ui.selectable_value(&mut self.verify.runset, i, format!("{} · {}", rs.name, rs.tool.label()));
                    }
                })
                .response
                .labelled_by(label.id);
            let rs = &tech.signoff[self.verify.runset];
            let run = ui.add_enabled(idle, egui::Button::new(format!("▶ Run {}", rs.tool.label())));
            if run.on_hover_text(format!("Write the cell to GDS and run {}", rs.deck)).clicked() {
                self.start_verification(Verify::Signoff, l, cx, ui.ctx());
            }
            self.job_row(ui, Verify::Signoff);
            match &self.verify.signoff {
                Some(Ok(rep)) => {
                    let good = rep
                        .drc
                        .as_ref()
                        .map_or(rep.lvs.as_ref().and_then(|l| l.matched) == Some(true), |d| d.violations.is_empty());
                    ui.colored_label(if good { pal.ok } else { pal.danger }, rep.summary());
                    if let Some(lv) = &rep.lvs {
                        egui::ScrollArea::vertical().id_salt("signoff_lvs").max_height(160.0).show(ui, |ui| {
                            for m in &lv.messages {
                                ui.label(egui::RichText::new(m).small());
                            }
                        });
                    }
                    if !rep.notes.is_empty() {
                        egui::CollapsingHeader::new(format!("Deck notes ({})", rep.notes.len())).id_salt("signoff_notes").show(
                            ui,
                            |ui| {
                                egui::ScrollArea::vertical().id_salt("signoff_notes_scroll").max_height(160.0).show(ui, |ui| {
                                    for n in &rep.notes {
                                        ui.label(egui::RichText::new(n).small().color(pal.text_weak));
                                    }
                                });
                            },
                        );
                    }
                    ui.label(egui::RichText::new(format!("Reports: {}", rep.dir.display())).small().color(pal.text_weak));
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(e).small().color(pal.danger));
                }
                None => {}
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
