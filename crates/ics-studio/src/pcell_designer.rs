//! PCell Designer: a sub-tool window that turns a drawn layout into a parametric cell.
//!
//! The drawn cell becomes the source of a [`Template`]; parameters drive stretch lines,
//! repeats, conditions and contact fills placed on a picture of the source. A live preview
//! generates the cell at any values, suggestions read a drawn device (contacts, fingers, W,
//! L), a corner check runs the technology's DRC deck on the parameter extremes and the lint
//! explains mistakes. Saving stores the master cell; it is then placed from the layout
//! editor's instance tool like the built-in PCells.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use ics_core::drc;
use ics_core::layout::Geometry;
use ics_core::pcell::PCell;
use ics_core::pcell_design::{Axis, CornerRun, Finding, Level, Op, Param, ParamKind, Side, Target, Template, Values};
use ics_core::{CellRef, IRect, Layout, Project, Tech};

use crate::layout_view::layer_color;
use crate::theme::{self, Palette};

/// What the designer asks the app to do.
pub enum PCellAction {
    /// Store the template as master cell `lib/<name>` and refresh its placed variants.
    Publish { lib: String, def: Template },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sel {
    None,
    Param(usize),
    Op(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Guide,
    Design,
    Corners,
    Issues,
}

/// Canvas interaction for the selected operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pick {
    /// Click to place the stretch line.
    Line,
    /// Drag to draw the region / fill rectangle.
    Region,
}

struct CornerJob {
    gen_: u64,
    rx: Receiver<Vec<CornerRun>>,
}

pub struct PCellDesigner {
    pub open: bool,
    tpl: Option<Template>,
    /// Cell the drawing came from.
    source: Option<CellRef>,
    /// Library the master is stored in.
    lib: String,
    /// Template when loaded or last saved, for the unsaved marker.
    baseline: Option<Template>,
    undo: Vec<Template>,
    sel: Sel,
    tab: Tab,
    preview: Values,
    pick: Option<Pick>,
    drag_from: Option<(i64, i64)>,
    gen_: u64,
    lint_gen: u64,
    findings: Vec<Finding>,
    preview_key: Option<(u64, Values)>,
    preview_out: Result<Layout, String>,
    corners: Option<(u64, Vec<CornerRun>)>,
    corner_sel: usize,
    job: Option<CornerJob>,
    status: Option<(bool, String)>,
    show_source_on_preview: bool,
}

impl Default for PCellDesigner {
    fn default() -> Self {
        PCellDesigner {
            open: false,
            tpl: None,
            source: None,
            lib: String::new(),
            baseline: None,
            undo: vec![],
            sel: Sel::None,
            tab: Tab::Guide,
            preview: Values::new(),
            pick: None,
            drag_from: None,
            gen_: 1,
            lint_gen: 0,
            findings: vec![],
            preview_key: None,
            preview_out: Err(String::new()),
            corners: None,
            corner_sel: 0,
            job: None,
            status: None,
            show_source_on_preview: false,
        }
    }
}

fn um(v: i64, tech: &Tech) -> String {
    crate::layout_view::um(v, tech)
}

fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl PCellDesigner {
    /// Open the window; with no design yet, start from the active layout cell.
    pub fn show(&mut self, project: &Project, active: Option<&CellRef>) {
        if self.tpl.is_none()
            && let Some(r) = active
        {
            self.load(project, r);
        }
        self.open = true;
    }

    /// The design being edited.
    pub fn template(&self) -> Option<&Template> {
        self.tpl.as_ref()
    }

    /// Replace the design (automation and tests).
    pub fn template_mut(&mut self) -> Option<&mut Template> {
        self.touch();
        self.tpl.as_mut()
    }

    pub fn is_checking(&self) -> bool {
        self.job.is_some()
    }

    /// Start a design from cell `r`: a designer master is reopened, any other layout becomes
    /// the source of a new design.
    pub fn load(&mut self, project: &Project, r: &CellRef) {
        let Some(c) = project.cell(r) else { return };
        let tpl = match (&c.pcell, &c.layout) {
            (Some(PCell::Template { def, .. }), _) => (**def).clone(),
            (Some(_), _) => {
                self.status = Some((false, format!("{r} is a generated PCell; open the drawn cell instead")));
                return;
            }
            (None, Some(l)) => Template::from_layout(&unique_name(project, &r.lib, &format!("{}_pc", r.cell)), l),
            (None, None) => {
                self.status = Some((false, format!("{r} has no layout")));
                return;
            }
        };
        self.baseline = matches!(c.pcell, Some(PCell::Template { .. })).then(|| tpl.clone());
        self.preview = tpl.defaults();
        self.tpl = Some(tpl);
        self.source = Some(r.clone());
        self.lib = if r.lib == ics_core::pcell::PCELL_LIB { String::new() } else { r.lib.clone() };
        self.undo.clear();
        self.sel = Sel::None;
        self.pick = None;
        self.corners = None;
        self.status = Some((true, format!("Loaded {r}")));
        self.touch();
    }

    /// Master `r` was changed elsewhere (an agent): reload it when it is the design open here
    /// and has no unsaved edits.
    pub fn sync(&mut self, project: &Project, r: &CellRef) {
        let ours = self.tpl.as_ref().is_some_and(|t| t.name == r.cell) && self.lib == r.lib;
        if ours && self.baseline.is_some() && self.baseline == self.tpl {
            self.load(project, r);
            self.status = Some((true, format!("{r} was updated by the agent")));
        }
    }

    fn touch(&mut self) {
        self.gen_ += 1;
    }

    /// Keep a copy for undo before changing the design.
    fn checkpoint(&mut self) {
        if let Some(t) = &self.tpl {
            if self.undo.last() != Some(t) {
                self.undo.push(t.clone());
            }
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
    }

    fn refresh(&mut self, tech: &Tech) {
        let Some(t) = &self.tpl else { return };
        if self.lint_gen != self.gen_ {
            self.findings = t.lint(tech);
            self.lint_gen = self.gen_;
            // Drop preview values of removed parameters, add new ones at their default.
            let defaults = t.defaults();
            self.preview.retain(|k, _| defaults.contains_key(k));
            for (k, v) in defaults {
                self.preview.entry(k).or_insert(v);
            }
        }
        let key = (self.gen_, self.preview.clone());
        if self.preview_key.as_ref() != Some(&key) {
            let given: Vec<(String, f64)> = self.preview.iter().map(|(k, v)| (k.clone(), *v)).collect();
            self.preview_out = t.generate(tech, &given);
            self.preview_key = Some(key);
        }
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let Ok(runs) = job.rx.try_recv() else { return };
        let gen_ = job.gen_;
        self.job = None;
        let bad = runs.iter().filter(|r| r.result.as_ref().map_or(true, |c| !c.violations.is_empty())).count();
        self.status = Some((bad == 0, format!("Corner check: {} of {} corner(s) clean", runs.len() - bad, runs.len())));
        self.corner_sel = runs.iter().position(|r| r.result.as_ref().map_or(true, |c| !c.violations.is_empty())).unwrap_or(0);
        self.corners = Some((gen_, runs));
    }

    fn start_corners(&mut self, ctx: &egui::Context, project: &Project) {
        let Some(t) = self.tpl.clone() else { return };
        let deck = match drc::parse_any(&project.tech.deck_text(), &project.tech) {
            Ok(d) => d.deck,
            Err(e) => {
                self.status = Some((false, format!("The technology deck does not parse: {}", e[0])));
                return;
            }
        };
        let (tx, rx) = channel();
        let (p, ctx) = (project.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(t.check_corners(&p.tech, &deck, &p));
            ctx.request_repaint();
        });
        self.job = Some(CornerJob { gen_: self.gen_, rx });
        self.tab = Tab::Corners;
    }

    /// The designer window (a native window where the platform supports it).
    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>, active: Option<&CellRef>) -> Option<PCellAction> {
        if !self.open {
            return None;
        }
        let Some(project) = project else {
            self.open = false;
            return None;
        };
        self.poll();
        let mut open = true;
        let r = crate::float::tool_window(ctx, "pcell_designer", "PCell Designer", [1240.0, 800.0], &mut open, |ui| {
            self.ui(ui, project, active)
        });
        self.open &= open;
        r.flatten()
    }

    fn ui(&mut self, ui: &mut egui::Ui, project: &Project, active: Option<&CellRef>) -> Option<PCellAction> {
        let tech = &project.tech;
        let pal = theme::palette(ui.ctx());
        if self.job.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.refresh(tech);
        let mut action = None;
        egui::Panel::top("pd_toolbar")
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| action = self.toolbar(ui, project, active, &pal));
        if self.tpl.is_none() {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                self.guide(ui, project, active, &pal);
            });
            return action;
        }
        theme::aside(
            egui::Panel::left("pd_list")
                .resizable(true)
                .default_size(300.0)
                .size_range(240.0..=460.0)
                .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8)),
            ui,
            |ui| self.side_list(ui, tech, &pal),
        );
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let issues = self.findings.iter().filter(|f| f.level != Level::Info).count();
                let corners = match (&self.corners, &self.job) {
                    (_, Some(_)) => "Corners (running)".to_string(),
                    (Some((g, runs)), _) => {
                        let bad = runs.iter().filter(|r| r.result.as_ref().map_or(true, |c| !c.violations.is_empty())).count();
                        let stale = if *g != self.gen_ { ", outdated" } else { "" };
                        format!("Corners ({bad} failing{stale})")
                    }
                    _ => "Corners".into(),
                };
                for (t, name) in [
                    (Tab::Guide, "Start here".to_string()),
                    (Tab::Design, "Design".into()),
                    (Tab::Corners, corners),
                    (Tab::Issues, format!("Issues ({issues})")),
                ] {
                    if ui.selectable_label(self.tab == t, name).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("pd_main").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Guide => self.guide(ui, project, active, &pal),
                Tab::Design => self.design(ui, project, &pal),
                Tab::Corners => self.corners_tab(ui, project, &pal),
                Tab::Issues => self.issues(ui, &pal),
            });
        });
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, project: &Project, active: Option<&CellRef>, pal: &Palette) -> Option<PCellAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("PCell Designer").strong().color(pal.text_strong));
            if let Some(s) = &self.source {
                ui.label(RichText::new(format!("· from {s}")).color(pal.text_weak));
            }
            if self.tpl.is_some() && self.baseline.as_ref() != self.tpl.as_ref() {
                ui.label(RichText::new("● not saved").color(pal.warning))
                    .on_hover_text("The design differs from the saved master cell. Save it to place it.");
            }
            ui.separator();
            let load = ui.add_enabled(active.is_some(), egui::Button::new("Load active cell"));
            let hover = match active {
                Some(c) => format!("Start a design from the drawing of {c} (a saved design reopens)"),
                None => "Open a layout cell first".into(),
            };
            if load.on_hover_text(hover).on_disabled_hover_text("Open a layout cell first").clicked()
                && let Some(r) = active
            {
                self.load(project, r);
            }
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                .on_hover_text("Undo the last design change")
                .clicked()
                && let Some(t) = self.undo.pop()
            {
                self.tpl = Some(t);
                self.touch();
            }
            if let Some(t) = &self.tpl
                && ui
                    .button("Export SKILL…")
                    .on_hover_text(
                        "Write the PCell as a SKILL pcDefinePCell for Cadence Virtuoso (.il), generating the same \
                         layout. Layer names are the technology's.",
                    )
                    .clicked()
            {
                let lib = if self.lib.is_empty() { "myLib".to_string() } else { self.lib.clone() };
                match t.to_skill(&project.tech, &lib) {
                    Ok((text, warnings)) => {
                        let pick = rfd::FileDialog::new()
                            .add_filter("SKILL", &["il"])
                            .set_file_name(format!("{}.il", t.name))
                            .save_file();
                        if let Some(path) = pick {
                            self.status = Some(match std::fs::write(&path, text) {
                                Ok(()) if warnings.is_empty() => (true, format!("Wrote {}", path.display())),
                                Ok(()) => (true, format!("Wrote {} ({})", path.display(), warnings.join("; "))),
                                Err(e) => (false, format!("{}: {e}", path.display())),
                            });
                        }
                    }
                    Err(e) => self.status = Some((false, format!("SKILL export: {e}"))),
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    self.open = false;
                }
                let Some(t) = &self.tpl else { return };
                let errors = self.findings.iter().filter(|f| f.level == Level::Error).count();
                let save = egui::Button::new(RichText::new("Save PCell").color(theme::on_fill(pal.accent))).fill(pal.accent);
                let libs: Vec<&str> =
                    project.libraries.iter().map(|l| l.name.as_str()).filter(|l| *l != ics_core::pcell::PCELL_LIB).collect();
                if self.lib.is_empty() || !libs.contains(&self.lib.as_str()) {
                    self.lib = libs.first().map(|s| s.to_string()).unwrap_or_default();
                }
                let hover = format!(
                    "Store the master cell {}/{} and update the variants already placed. Place it with the layout \
                     editor's instance tool (Designed PCells).",
                    self.lib, t.name
                );
                if ui
                    .add_enabled(errors == 0 && !self.lib.is_empty(), save)
                    .on_hover_text(hover)
                    .on_disabled_hover_text("Fix the errors on the Issues tab first")
                    .clicked()
                {
                    action = Some(PCellAction::Publish { lib: self.lib.clone(), def: t.clone() });
                    self.baseline = Some(t.clone());
                }
                let lb = ui.label("in");
                egui::ComboBox::from_id_salt("pd_lib")
                    .selected_text(self.lib.clone())
                    .show_ui(ui, |ui| {
                        for l in &libs {
                            ui.selectable_value(&mut self.lib, l.to_string(), *l);
                        }
                    })
                    .response
                    .labelled_by(lb.id)
                    .on_hover_text("Library of the master cell");
                if let Some((ok, s)) = &self.status {
                    ui.label(RichText::new(s).small().color(if *ok { pal.text_weak } else { pal.danger }));
                }
            });
        });
        action
    }

    /// Show the outcome of a save in the toolbar.
    pub fn saved(&mut self, msg: String, ok: bool) {
        self.status = Some((ok, msg));
    }

    // -----------------------------------------------------------------------------------------
    // Left list: name, parameters, operations, suggestions

    fn side_list(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        egui::ScrollArea::vertical().id_salt("pd_side").auto_shrink([false, false]).show(ui, |ui| {
            let Some(t) = self.tpl.as_mut() else { return };
            theme::section(ui, "PCell");
            let mut name = t.name.clone();
            ui.horizontal(|ui| {
                let lb = ui.label("Name");
                if ui.add(egui::TextEdit::singleline(&mut name).desired_width(170.0)).labelled_by(lb.id).changed() {
                    self.undo.push(t.clone());
                    t.name = name;
                    self.gen_ += 1;
                }
            });
            let mut desc = t.description.clone();
            ui.horizontal(|ui| {
                let lb = ui.label("Note");
                if ui
                    .add(egui::TextEdit::singleline(&mut desc).desired_width(170.0).hint_text("what the cell is"))
                    .labelled_by(lb.id)
                    .changed()
                {
                    t.description = desc;
                    self.gen_ += 1;
                }
            });
            ui.label(
                RichText::new(format!(
                    "{} shape(s), {} instance(s) in the drawing",
                    t.source.shapes.len(),
                    t.source.instances.len()
                ))
                .small()
                .color(pal.text_weak),
            );
            ui.add_space(6.0);
            theme::section(ui, "Parameters");
            let mut new_sel = None;
            for (i, p) in t.params.iter().enumerate() {
                let sev = self.findings.iter().filter(|f| f.target == Target::Param(i)).map(|f| f.level).min();
                let text = format!(
                    "{}  {}  = {}{}",
                    p.name,
                    p.kind.label(),
                    num(p.default),
                    if p.kind == ParamKind::Length { " µm" } else { "" }
                );
                ui.horizontal(|ui| {
                    dot(ui, sev, pal);
                    if ui.selectable_label(self.sel == Sel::Param(i), text).clicked() {
                        new_sel = Some(Sel::Param(i));
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                for kind in ParamKind::ALL {
                    if ui
                        .small_button(format!("+ {}", kind.label()))
                        .on_hover_text(format!("Add a {} parameter", kind.label()))
                        .clicked()
                    {
                        self.undo.push(t.clone());
                        let base = match kind {
                            ParamKind::Length => "len",
                            ParamKind::Int => "n",
                            ParamKind::Bool => "flag",
                        };
                        let mut k = 1;
                        let name = loop {
                            let n = format!("{base}{k}");
                            if t.param(&n).is_none() {
                                break n;
                            }
                            k += 1;
                        };
                        let d = match kind {
                            ParamKind::Length => 1.0,
                            ParamKind::Int => 1.0,
                            ParamKind::Bool => 1.0,
                        };
                        t.params.push(Param::new(&name, kind, d));
                        new_sel = Some(Sel::Param(t.params.len() - 1));
                        self.gen_ += 1;
                    }
                }
            });
            ui.add_space(6.0);
            theme::section(ui, "Operations");
            if t.ops.is_empty() {
                ui.label(RichText::new("None yet. Add one, or use a suggestion below.").small().color(pal.text_weak));
            }
            for (k, op) in t.ops.iter().enumerate() {
                let sev = self.findings.iter().filter(|f| f.target == Target::Op(k)).map(|f| f.level).min();
                ui.horizontal(|ui| {
                    dot(ui, sev, pal);
                    let text = format!("{}. {}", k + 1, op.summary(tech.dbu_per_um));
                    if ui.selectable_label(self.sel == Sel::Op(k), text).clicked() {
                        new_sel = Some(Sel::Op(k));
                    }
                });
            }
            let bb = t.source.shapes_bbox().unwrap_or(IRect::new(0, 0, 1000, 1000));
            let mid = (snap((bb.x0 + bb.x1) / 2, tech.grid), snap((bb.y0 + bb.y1) / 2, tech.grid));
            let quarter = IRect::new(bb.x0, bb.y0, (bb.x0 + bb.x1) / 2, (bb.y0 + bb.y1) / 2);
            let mut add = None;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .small_button("+ Stretch X")
                    .on_hover_text("A vertical line; what is right of it moves by the amount")
                    .clicked()
                {
                    add = Some((
                        Op::Stretch { axis: Axis::X, at: mid.0, side: Side::High, amount: first_len(t), layers: vec![] },
                        Pick::Line,
                    ));
                }
                if ui
                    .small_button("+ Stretch Y")
                    .on_hover_text("A horizontal line; what is above it moves by the amount")
                    .clicked()
                {
                    add = Some((
                        Op::Stretch { axis: Axis::Y, at: mid.1, side: Side::High, amount: first_len(t), layers: vec![] },
                        Pick::Line,
                    ));
                }
                if ui.small_button("+ Repeat").on_hover_text("Copy what lies inside a region count times, pitch apart").clicked()
                {
                    add = Some((
                        Op::Repeat {
                            axis: Axis::X,
                            region: quarter,
                            count: first_int(t),
                            pitch: um(quarter.width(), tech),
                            at: None,
                            layers: vec![],
                        },
                        Pick::Region,
                    ));
                }
                if ui
                    .small_button("+ Condition")
                    .on_hover_text("Drop what lies inside a region when an expression is false")
                    .clicked()
                {
                    add = Some((Op::Condition { region: quarter, when: first_bool(t), layers: vec![] }, Pick::Region));
                }
                if ui.small_button("+ Contact fill").on_hover_text("An area refilled with via cuts after stretching").clicked() {
                    let via = tech.vias.first().map(|v| v.name.clone()).unwrap_or_default();
                    add = Some((Op::ContactFill { rect: quarter, via }, Pick::Region));
                }
            });
            if let Some((op, pick)) = add {
                self.undo.push(t.clone());
                t.ops.push(op);
                new_sel = Some(Sel::Op(t.ops.len() - 1));
                self.pick = Some(pick);
                self.tab = Tab::Design;
                self.gen_ += 1;
            }
            ui.add_space(6.0);
            theme::section(ui, "Suggestions");
            ui.label(RichText::new("Read the drawing and add the usual parameters of a device.").small().color(pal.text_weak));
            let mut sug: Option<(&str, Result<String, String>)> = None;
            let before = t.clone();
            if ui
                .button("Contacts to fills")
                .on_hover_text("Replace every cluster of via cuts by a contact fill, so stretching re-arrays the cuts")
                .clicked()
            {
                let n = t.suggest_contact_fills(tech);
                sug = Some((
                    "contacts",
                    if n > 0 { Ok(format!("{n} contact fill(s) made")) } else { Err("No via cuts found".into()) },
                ));
            }
            if ui
                .button("Detect fingers (nf)")
                .on_hover_text("Equal gates at a fixed pitch become one unit repeated nf times; shapes spanning them follow")
                .clicked()
            {
                sug = Some(("fingers", t.suggest_fingers(tech)));
            }
            if ui.button("Width stretch (w)").on_hover_text("A horizontal line through the diffusion, driven by w").clicked() {
                sug = Some(("width", t.suggest_width(tech)));
            }
            if ui.button("Length stretch (l)").on_hover_text("A vertical line through the first gate, driven by l").clicked() {
                sug = Some(("length", t.suggest_length(tech)));
            }
            if let Some((_, r)) = sug {
                match r {
                    Ok(m) => {
                        if *t != before {
                            self.undo.push(before);
                        }
                        self.status = Some((true, m));
                        self.gen_ += 1;
                        self.tab = Tab::Design;
                    }
                    Err(e) => self.status = Some((false, e)),
                }
            }
            if let Some(s) = new_sel {
                self.sel = s;
                if !matches!(s, Sel::Op(_)) {
                    self.pick = None;
                }
                self.tab = Tab::Design;
            }
        });
    }

    // -----------------------------------------------------------------------------------------
    // Design tab

    fn design(&mut self, ui: &mut egui::Ui, project: &Project, pal: &Palette) {
        let tech = &project.tech;
        match self.sel {
            Sel::Param(i) => self.param_editor(ui, i, pal),
            Sel::Op(k) => self.op_editor(ui, k, tech, pal),
            Sel::None => {
                ui.label(
                    RichText::new("Select a parameter or an operation on the left. The left picture is the drawing with the operations; the right one is the cell generated at the values below.")
                        .color(pal.text_weak),
                );
            }
        }
        ui.add_space(8.0);
        self.values_bar(ui, tech, pal);
        ui.add_space(6.0);
        let w = ui.available_width();
        let h = (ui.available_height() - 10.0).clamp(320.0, 620.0);
        let side = w >= 600.0;
        let cw = if side { (w - 12.0) / 2.0 } else { w };
        let draw = |ui: &mut egui::Ui, me: &mut Self| {
            me.source_canvas(ui, Vec2::new(cw, h), project, pal);
            me.preview_canvas(ui, Vec2::new(cw, h), project, pal);
        };
        if side {
            ui.horizontal_top(|ui| draw(ui, self));
        } else {
            draw(ui, self);
        }
    }

    fn values_bar(&mut self, ui: &mut egui::Ui, tech: &Tech, pal: &Palette) {
        let Some(t) = &self.tpl else { return };
        if t.params.is_empty() {
            return;
        }
        let step = tech.grid.max(1) as f64 / tech.dbu_per_um.max(1) as f64;
        egui::Frame::new().fill(pal.bg_sidebar).stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(8).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Preview at").strong());
                    for p in &t.params {
                        let v = self.preview.entry(p.name.clone()).or_insert(p.default);
                        let lb = ui.label(p.name.as_str());
                        let lo = p.min.unwrap_or(if p.kind == ParamKind::Length { step } else { 0.0 });
                        let hi = p.max.unwrap_or(1e6);
                        match p.kind {
                            ParamKind::Bool => {
                                let mut on = *v != 0.0;
                                ui.checkbox(&mut on, "").labelled_by(lb.id);
                                *v = if on { 1.0 } else { 0.0 };
                            }
                            ParamKind::Int => {
                                ui.add(egui::DragValue::new(v).speed(0.1).range(lo..=hi).max_decimals(0)).labelled_by(lb.id);
                            }
                            ParamKind::Length => {
                                ui.add(egui::DragValue::new(v).speed(step).range(lo..=hi).max_decimals(4).suffix(" µm"))
                                    .labelled_by(lb.id);
                                *v = (*v / step).round() * step;
                            }
                        }
                        ui.add_space(6.0);
                    }
                    if ui.small_button("Defaults").clicked() {
                        self.preview = t.defaults();
                    }
                    ui.checkbox(&mut self.show_source_on_preview, "Overlay drawing");
                });
            },
        );
    }

    fn param_editor(&mut self, ui: &mut egui::Ui, i: usize, pal: &Palette) {
        let Some(t) = self.tpl.as_mut() else { return };
        if i >= t.params.len() {
            self.sel = Sel::None;
            return;
        }
        let before = t.clone();
        let mut remove = false;
        card(ui, pal, |ui| {
            ui.label(RichText::new("Parameter").strong().color(pal.text_strong));
            ui.label(
                RichText::new(
                    "A value the user of the PCell sets. Operations use it by name in their expressions (lengths in µm).",
                )
                .small()
                .color(pal.text_weak),
            );
            let p = &mut t.params[i];
            let old_name = p.name.clone();
            egui::Grid::new("pd_param").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                let lb = ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(160.0)).labelled_by(lb.id);
                ui.end_row();
                let lb = ui.label("Kind");
                egui::ComboBox::from_id_salt("pd_kind")
                    .selected_text(p.kind.label())
                    .show_ui(ui, |ui| {
                        for k in ParamKind::ALL {
                            ui.selectable_value(&mut p.kind, k, k.label());
                        }
                    })
                    .response
                    .labelled_by(lb.id);
                ui.end_row();
                let suffix = if p.kind == ParamKind::Length { " µm" } else { "" };
                let lb = ui.label("Default");
                if p.kind == ParamKind::Bool {
                    let mut on = p.default != 0.0;
                    ui.checkbox(&mut on, "on").labelled_by(lb.id);
                    p.default = if on { 1.0 } else { 0.0 };
                } else {
                    ui.add(egui::DragValue::new(&mut p.default).speed(0.01).max_decimals(4).suffix(suffix)).labelled_by(lb.id);
                }
                ui.end_row();
                if p.kind != ParamKind::Bool {
                    for (label, v) in [("Minimum", &mut p.min), ("Maximum", &mut p.max)] {
                        let lb = ui.label(label);
                        ui.horizontal(|ui| {
                            let mut on = v.is_some();
                            if ui.checkbox(&mut on, "").labelled_by(lb.id).changed() {
                                *v = on.then_some(p.default);
                            }
                            if let Some(x) = v.as_mut() {
                                ui.add(egui::DragValue::new(x).speed(0.01).max_decimals(4).suffix(suffix)).labelled_by(lb.id);
                            } else {
                                ui.label(RichText::new("none").small().color(pal.text_weak));
                            }
                        });
                        ui.end_row();
                    }
                }
                let lb = ui.label("Description");
                ui.add(egui::TextEdit::singleline(&mut p.description).desired_width(260.0)).labelled_by(lb.id);
                ui.end_row();
            });
            if p.name != old_name && !old_name.is_empty() {
                let (from, to) = (old_name, p.name.clone());
                rename_in_ops(&mut t.ops, &from, &to);
            }
            ui.horizontal(|ui| {
                if ui.button("Delete parameter").clicked() {
                    remove = true;
                }
            });
        });
        if remove {
            t.params.remove(i);
            self.sel = Sel::None;
        }
        if *t != before {
            self.undo.push(before);
            self.gen_ += 1;
        }
        self.findings_for(ui, Target::Param(i), pal);
    }

    fn op_editor(&mut self, ui: &mut egui::Ui, k: usize, tech: &Tech, pal: &Palette) {
        let Some(t) = self.tpl.as_mut() else { return };
        if k >= t.ops.len() {
            self.sel = Sel::None;
            return;
        }
        let before = t.clone();
        let params: Vec<String> = t.params.iter().map(|p| p.name.clone()).collect();
        let (mut remove, mut up, mut down) = (false, false, false);
        let mut pick = self.pick;
        card(ui, pal, |ui| {
            let op = &mut t.ops[k];
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}. {}", k + 1, op.title())).strong().color(pal.text_strong));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    remove = ui.button("Delete").clicked();
                    down = ui.small_button("Later").on_hover_text("Move down (stretches apply in list order)").clicked();
                    up = ui.small_button("Earlier").on_hover_text("Move up").clicked();
                });
            });
            ui.label(RichText::new(explain(op)).small().color(pal.text_weak));
            egui::Grid::new("pd_op").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| match op {
                Op::Stretch { axis, at, side, amount, layers } => {
                    axis_pick(ui, axis);
                    let lb = ui.label("Line at");
                    ui.horizontal(|ui| {
                        len_edit(ui, at, tech, lb.id);
                        if ui.selectable_label(pick == Some(Pick::Line), "Pick on drawing").clicked() {
                            pick = if pick == Some(Pick::Line) { None } else { Some(Pick::Line) };
                        }
                    });
                    ui.end_row();
                    let lb = ui.label("Moves");
                    egui::ComboBox::from_id_salt("pd_side")
                        .selected_text(side.label(*axis))
                        .show_ui(ui, |ui| {
                            for s in Side::ALL {
                                ui.selectable_value(side, s, s.label(*axis));
                            }
                        })
                        .response
                        .labelled_by(lb.id);
                    ui.end_row();
                    expr_edit(ui, "Amount", amount, &params, "µm; the line moves by its change from the defaults");
                    layers_edit(ui, layers, tech);
                }
                Op::Repeat { axis, region, count, pitch, at, layers } => {
                    axis_pick(ui, axis);
                    region_row(ui, "Region", region, tech, &mut pick);
                    expr_edit(ui, "Count", count, &params, "copies, rounded; 1 = as drawn");
                    expr_edit(ui, "Pitch", pitch, &params, "µm between copies");
                    let lb = ui.label("Spanning shapes");
                    ui.horizontal(|ui| {
                        let mut auto = at.is_none();
                        if ui.checkbox(&mut auto, "stretch at the region middle").labelled_by(lb.id).changed() {
                            *at = if auto {
                                None
                            } else {
                                Some(snap((axis_lo(*axis, region) + axis_hi(*axis, region)) / 2, tech.grid))
                            };
                        }
                        if let Some(x) = at.as_mut() {
                            len_edit(ui, x, tech, lb.id);
                        }
                    });
                    ui.end_row();
                    layers_edit(ui, layers, tech);
                }
                Op::Condition { region, when, layers } => {
                    region_row(ui, "Region", region, tech, &mut pick);
                    expr_edit(ui, "Keep if", when, &params, "non-zero keeps the items, e.g. flag or nf > 1");
                    layers_edit(ui, layers, tech);
                }
                Op::ContactFill { rect, via } => {
                    region_row(ui, "Area", rect, tech, &mut pick);
                    let lb = ui.label("Via");
                    egui::ComboBox::from_id_salt("pd_via")
                        .selected_text(via.clone())
                        .show_ui(ui, |ui| {
                            for v in &tech.vias {
                                ui.selectable_value(via, v.name.clone(), format!("{} ({})", v.name, v.cut));
                            }
                        })
                        .response
                        .labelled_by(lb.id);
                    ui.end_row();
                }
            });
        });
        self.pick = pick;
        if remove {
            t.ops.remove(k);
            self.sel = Sel::None;
            self.pick = None;
        } else if up && k > 0 {
            t.ops.swap(k, k - 1);
            self.sel = Sel::Op(k - 1);
        } else if down && k + 1 < t.ops.len() {
            t.ops.swap(k, k + 1);
            self.sel = Sel::Op(k + 1);
        }
        if *t != before {
            self.undo.push(before);
            self.gen_ += 1;
        }
        self.findings_for(ui, Target::Op(k), pal);
    }

    fn findings_for(&self, ui: &mut egui::Ui, target: Target, pal: &Palette) {
        for f in self.findings.iter().filter(|f| f.target == target) {
            finding_row(ui, f, pal);
        }
    }

    /// The drawing with the operations; picks lines and regions.
    fn source_canvas(&mut self, ui: &mut egui::Ui, size: Vec2, project: &Project, pal: &Palette) {
        let tech = &project.tech;
        let Some(t) = &self.tpl else { return };
        let mut lay = t.source.clone();
        // Show the cuts the contact fills make in the drawing.
        let mut fills_only = t.clone();
        fills_only.ops.retain(|o| matches!(o, Op::ContactFill { .. }));
        if let Ok(g) = fills_only.generate(tech, &[]) {
            lay = g;
        }
        let bb = bbox_all(&lay, project).unwrap_or(IRect::new(0, 0, 1000, 1000));
        let sel_op = match self.sel {
            Sel::Op(k) => Some(k),
            _ => None,
        };
        let (resp, p) = ui.allocate_painter(size, Sense::click_and_drag());
        p.rect_filled(resp.rect, 6.0, pal.bg_canvas);
        p.rect_stroke(resp.rect, 6.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        let fit = Fit::new(resp.rect.shrink(28.0), grow(bb, 0.06));
        draw_layout(&p, &fit, &lay, project, pal, 1.0);
        let accent = pal.accent;
        for (k, op) in t.ops.iter().enumerate() {
            let on = sel_op == Some(k);
            let c = if on { accent } else { pal.text_weak };
            let w = if on { 2.2 } else { 1.2 };
            match op {
                Op::Stretch { axis, at, side, .. } => {
                    let (a, b) = match axis {
                        Axis::X => (fit.pt(*at, bb.y0 - 10_000_000), fit.pt(*at, bb.y1 + 10_000_000)),
                        Axis::Y => (fit.pt(bb.x0 - 10_000_000, *at), fit.pt(bb.x1 + 10_000_000, *at)),
                    };
                    let (a, b) = (clip(a, resp.rect), clip(b, resp.rect));
                    p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(w, c), 8.0, 5.0));
                    let mid = a + (b - a) * 0.08 + Vec2::new(6.0, 6.0);
                    let dir = match axis {
                        Axis::X => Vec2::new(1.0, 0.0),
                        Axis::Y => Vec2::new(0.0, -1.0),
                    };
                    let at_pt = match axis {
                        Axis::X => Pos2::new(a.x, mid.y + 12.0),
                        Axis::Y => Pos2::new(mid.x + 12.0, a.y),
                    };
                    match side {
                        Side::High => arrow(&p, at_pt, at_pt + dir * 26.0, c),
                        Side::Low => arrow(&p, at_pt, at_pt - dir * 26.0, c),
                        Side::Both => {
                            arrow(&p, at_pt, at_pt + dir * 22.0, c);
                            arrow(&p, at_pt, at_pt - dir * 22.0, c);
                        }
                    }
                    p.text(
                        at_pt + Vec2::new(4.0, 4.0) + dir.rot90() * 10.0,
                        Align2::LEFT_TOP,
                        format!("{}", k + 1),
                        FontId::proportional(11.0),
                        c,
                    );
                }
                Op::Repeat { region, axis, .. } => {
                    let r = fit.r(region);
                    dashed_rect(&p, r, Stroke::new(w, c));
                    // A ghost of the next copy at the default pitch.
                    if let Op::Repeat { pitch, .. } = op
                        && let Ok(e) = ics_core::pcell_design::parse_expr(pitch)
                        && let Ok(v) = e.eval(&t.defaults())
                    {
                        let d = (v * tech.dbu_per_um as f64) as i64;
                        let ghost = match axis {
                            Axis::X => region.translate(d, 0),
                            Axis::Y => region.translate(0, d),
                        };
                        dashed_rect(&p, fit.r(&ghost), Stroke::new(1.0, c.gamma_multiply(0.5)));
                    }
                    p.text(
                        r.left_top() + Vec2::new(3.0, 2.0),
                        Align2::LEFT_TOP,
                        format!("{} repeat", k + 1),
                        FontId::proportional(11.0),
                        c,
                    );
                }
                Op::Condition { region, .. } => {
                    let r = fit.r(region);
                    dashed_rect(&p, r, Stroke::new(w, pal.warning.gamma_multiply(if on { 1.0 } else { 0.7 })));
                    p.text(
                        r.left_top() + Vec2::new(3.0, 2.0),
                        Align2::LEFT_TOP,
                        format!("{} if", k + 1),
                        FontId::proportional(11.0),
                        c,
                    );
                }
                Op::ContactFill { rect, .. } => {
                    let r = fit.r(rect);
                    p.rect_stroke(r, 0.0, Stroke::new(if on { 2.0 } else { 1.0 }, c), StrokeKind::Outside);
                }
            }
        }
        p.text(
            resp.rect.left_top() + Vec2::new(8.0, 6.0),
            Align2::LEFT_TOP,
            "Drawing and operations",
            FontId::proportional(12.0),
            pal.text_weak,
        );
        // Interaction.
        let world = |pos: Pos2| fit.world(pos);
        let g = tech.grid.max(1);
        if let Some(pos) = resp.hover_pos() {
            let (x, y) = world(pos);
            p.text(
                resp.rect.left_bottom() + Vec2::new(8.0, -6.0),
                Align2::LEFT_BOTTOM,
                format!("x {}  y {} µm", um(snap(x, g), tech), um(snap(y, g), tech)),
                FontId::monospace(11.0),
                pal.text_weak,
            );
        }
        let Some(k) = sel_op else {
            if resp.clicked()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                self.select_at(&fit, pos);
            }
            return;
        };
        let edges = edge_coords(&lay);
        let snap_to = |axis: Axis, v: i64, fit: &Fit| -> i64 {
            let tol = (6.0 / fit.s) as i64;
            let list = match axis {
                Axis::X => &edges.0,
                Axis::Y => &edges.1,
            };
            list.iter().copied().filter(|e| (e - v).abs() <= tol).min_by_key(|e| (e - v).abs()).unwrap_or(snap(v, g))
        };
        match self.pick {
            Some(Pick::Line) => {
                if let Some(pos) = resp.hover_pos() {
                    let (x, y) = world(pos);
                    let Some(Op::Stretch { axis, .. }) = self.tpl.as_ref().and_then(|t| t.ops.get(k)) else { return };
                    let axis = *axis;
                    let v = match axis {
                        Axis::X => snap_to(Axis::X, x, &fit),
                        Axis::Y => snap_to(Axis::Y, y, &fit),
                    };
                    let (a, b) = match axis {
                        Axis::X => (Pos2::new(fit.pt(v, 0).x, resp.rect.top()), Pos2::new(fit.pt(v, 0).x, resp.rect.bottom())),
                        Axis::Y => (Pos2::new(resp.rect.left(), fit.pt(0, v).y), Pos2::new(resp.rect.right(), fit.pt(0, v).y)),
                    };
                    p.line_segment([a, b], Stroke::new(1.0, accent.gamma_multiply(0.6)));
                    if resp.clicked() {
                        self.checkpoint();
                        if let Some(Op::Stretch { at, .. }) = self.tpl.as_mut().and_then(|t| t.ops.get_mut(k)) {
                            *at = v;
                        }
                        self.pick = None;
                        self.touch();
                    }
                }
            }
            Some(Pick::Region) => {
                if resp.drag_started()
                    && let Some(pos) = resp.interact_pointer_pos()
                {
                    let (x, y) = world(pos);
                    self.drag_from = Some((snap_to(Axis::X, x, &fit), snap_to(Axis::Y, y, &fit)));
                }
                if let (Some(a), Some(pos)) = (self.drag_from, resp.interact_pointer_pos().or(resp.hover_pos())) {
                    let (x, y) = world(pos);
                    let b = (snap_to(Axis::X, x, &fit), snap_to(Axis::Y, y, &fit));
                    let r = IRect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
                    p.rect_stroke(fit.r(&r), 0.0, Stroke::new(1.5, accent), StrokeKind::Outside);
                    if resp.drag_stopped() {
                        self.drag_from = None;
                        if r.width() > 0 && r.height() > 0 {
                            self.checkpoint();
                            if let Some(op) = self.tpl.as_mut().and_then(|t| t.ops.get_mut(k)) {
                                match op {
                                    Op::Repeat { region, .. } | Op::Condition { region, .. } => *region = r,
                                    Op::ContactFill { rect, .. } => *rect = r,
                                    Op::Stretch { .. } => {}
                                }
                            }
                            self.pick = None;
                            self.touch();
                        }
                    }
                }
                p.text(
                    resp.rect.right_top() + Vec2::new(-8.0, 6.0),
                    Align2::RIGHT_TOP,
                    "Drag to draw the region",
                    FontId::proportional(12.0),
                    accent,
                );
            }
            None => {
                if resp.clicked()
                    && let Some(pos) = resp.interact_pointer_pos()
                {
                    self.select_at(&fit, pos);
                }
            }
        }
        if self.pick == Some(Pick::Line) {
            p.text(
                resp.rect.right_top() + Vec2::new(-8.0, 6.0),
                Align2::RIGHT_TOP,
                "Click to place the line",
                FontId::proportional(12.0),
                accent,
            );
        }
    }

    /// Select the operation drawn nearest to a click.
    fn select_at(&mut self, fit: &Fit, pos: Pos2) {
        let Some(t) = &self.tpl else { return };
        let mut best: Option<(f32, usize)> = None;
        for (k, op) in t.ops.iter().enumerate() {
            let d = match op {
                Op::Stretch { axis: Axis::X, at, .. } => (fit.pt(*at, 0).x - pos.x).abs(),
                Op::Stretch { axis: Axis::Y, at, .. } => (fit.pt(0, *at).y - pos.y).abs(),
                Op::Repeat { region: r, .. } | Op::Condition { region: r, .. } | Op::ContactFill { rect: r, .. } => {
                    let sr = fit.r(r);
                    if sr.expand(4.0).contains(pos) { 3.0 + sr.area().sqrt() * 0.01 } else { f32::INFINITY }
                }
            };
            if d < 8.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, k));
            }
        }
        if let Some((_, k)) = best {
            self.sel = Sel::Op(k);
        }
    }

    /// The cell generated at the preview values.
    fn preview_canvas(&mut self, ui: &mut egui::Ui, size: Vec2, project: &Project, pal: &Palette) {
        let tech = &project.tech;
        let (resp, p) = ui.allocate_painter(size, Sense::hover());
        p.rect_filled(resp.rect, 6.0, pal.bg_canvas);
        p.rect_stroke(resp.rect, 6.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        match &self.preview_out {
            Ok(lay) => {
                let Some(bb) = bbox_all(lay, project) else { return };
                let all = match (&self.tpl, self.show_source_on_preview) {
                    (Some(t), true) => bbox_all(&t.source, project).map_or(bb, |s| s.union(&bb)),
                    _ => bb,
                };
                let fit = Fit::new(resp.rect.shrink(28.0), grow(all, 0.06));
                if let (Some(t), true) = (&self.tpl, self.show_source_on_preview) {
                    let st = Stroke::new(1.0, pal.text_weak.gamma_multiply(0.7));
                    for s in ics_core::hier::flatten(&t.source, project, 16) {
                        match &s.geom {
                            Geometry::Polygon { points } => {
                                p.add(egui::Shape::closed_line(points.iter().map(|&(x, y)| fit.pt(x, y)).collect(), st));
                            }
                            Geometry::Path { points, width } => draw_path(&p, &fit, points, *width, None, st),
                            g => {
                                for r in g.rects() {
                                    p.rect_stroke(fit.r(&r), 0.0, st, StrokeKind::Middle);
                                }
                            }
                        }
                    }
                }
                draw_layout(&p, &fit, lay, project, pal, 1.0);
                let r = fit.r(&bb);
                draw_dim(
                    &p,
                    Pos2::new(r.left(), r.bottom() + 14.0),
                    Pos2::new(r.right(), r.bottom() + 14.0),
                    &format!("{} µm", um(bb.width(), tech)),
                    pal,
                );
                draw_dim(
                    &p,
                    Pos2::new(r.right() + 14.0, r.bottom()),
                    Pos2::new(r.right() + 14.0, r.top()),
                    &format!("{} µm", um(bb.height(), tech)),
                    pal,
                );
                let name = self.tpl.as_ref().map(|t| t.values_text(&self.preview)).unwrap_or_default();
                p.text(
                    resp.rect.left_top() + Vec2::new(8.0, 6.0),
                    Align2::LEFT_TOP,
                    format!("Generated: {name}"),
                    FontId::proportional(12.0),
                    pal.text_weak,
                );
            }
            Err(e) => {
                p.text(resp.rect.center(), Align2::CENTER_CENTER, wrap(e, 60), FontId::proportional(13.0), pal.danger);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Corners and issues

    fn corners_tab(&mut self, ui: &mut egui::Ui, project: &Project, pal: &Palette) {
        let tech = &project.tech;
        ui.label(RichText::new("Corner check").heading().color(pal.text_strong));
        ui.label(
            RichText::new(
                "Generates the cell at its defaults, at each parameter's minimum and maximum (booleans toggled) and at all \
                 minimums / maximums together, and runs the technology's DRC deck on each. A PCell should be DRC clean over \
                 its whole range: narrow the range or fix the operation that breaks.",
            )
            .color(pal.text),
        );
        ui.horizontal(|ui| {
            let busy = self.job.is_some();
            if ui.add_enabled(!busy, egui::Button::new(if busy { "Checking…" } else { "Run corner check" })).clicked() {
                self.start_corners(ui.ctx(), project);
            }
            if busy {
                ui.spinner();
            }
            if let Some(t) = &self.tpl {
                ui.label(RichText::new(format!("{} corner(s)", t.corners().len())).small().color(pal.text_weak));
            }
        });
        ui.add_space(6.0);
        let Some((g, runs)) = &self.corners else {
            ui.label(RichText::new("Not run yet.").color(pal.text_weak));
            return;
        };
        if *g != self.gen_ {
            ui.label(RichText::new("The design changed since this check: run it again.").small().color(pal.warning));
        }
        let mut pick = None;
        egui::Grid::new("pd_corners").num_columns(3).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Corner").strong());
            ui.label(RichText::new("Result").strong());
            ui.label(RichText::new("Size").strong());
            ui.end_row();
            for (i, r) in runs.iter().enumerate() {
                if ui.selectable_label(self.corner_sel == i, &r.label).clicked() {
                    pick = Some(i);
                }
                match &r.result {
                    Ok(c) if c.violations.is_empty() => {
                        ui.label(RichText::new("DRC clean").color(pal.ok));
                    }
                    Ok(c) => {
                        ui.label(RichText::new(format!("{} violation(s)", c.violations.len())).color(pal.danger));
                    }
                    Err(e) => {
                        ui.label(RichText::new(format!("does not generate: {e}")).color(pal.danger));
                    }
                }
                match r.result.as_ref().ok().and_then(|c| c.layout.shapes_bbox()) {
                    Some(b) => ui.label(format!("{} × {} µm", um(b.width(), tech), um(b.height(), tech))),
                    None => ui.label(""),
                };
                ui.end_row();
            }
        });
        if let Some(i) = pick {
            self.corner_sel = i;
        }
        let Some(run) = runs.get(self.corner_sel) else { return };
        ui.add_space(8.0);
        let Ok(c) = &run.result else { return };
        let mut by_rule: std::collections::BTreeMap<&str, (usize, &str)> = Default::default();
        for v in &c.violations {
            let e = by_rule.entry(v.rule.as_str()).or_insert((0, v.description.as_str()));
            e.0 += 1;
        }
        for (rule, (n, d)) in &by_rule {
            ui.label(RichText::new(format!("{rule}: {d} ({n})")).small().color(pal.danger));
        }
        let (resp, p) = ui.allocate_painter(Vec2::new(ui.available_width().min(900.0), 420.0), Sense::hover());
        p.rect_filled(resp.rect, 6.0, pal.bg_canvas);
        let Some(bb) = bbox_all(&c.layout, project) else { return };
        let fit = Fit::new(resp.rect.shrink(20.0), grow(bb, 0.06));
        draw_layout(&p, &fit, &c.layout, project, pal, 0.8);
        for v in &c.violations {
            let r = fit.r(&v.marker).expand(2.0);
            p.rect_stroke(r, 0.0, Stroke::new(2.0, pal.danger), StrokeKind::Outside);
        }
        p.text(
            resp.rect.left_top() + Vec2::new(8.0, 6.0),
            Align2::LEFT_TOP,
            &run.label,
            FontId::proportional(12.0),
            pal.text_weak,
        );
    }

    fn issues(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        if self.findings.is_empty() {
            ui.label(RichText::new("No issues.").color(pal.ok));
        }
        let mut go = None;
        for f in &self.findings {
            ui.horizontal(|ui| {
                finding_row(ui, f, pal);
                let label = match f.target {
                    Target::Param(i) => self.tpl.as_ref().and_then(|t| t.params.get(i)).map(|p| p.name.clone()),
                    Target::Op(k) => Some(format!("operation {}", k + 1)),
                    Target::Template => None,
                };
                if let Some(l) = label
                    && ui.small_button(format!("Show {l}")).clicked()
                {
                    go = Some(f.target);
                }
            });
        }
        match go {
            Some(Target::Param(i)) => {
                self.sel = Sel::Param(i);
                self.tab = Tab::Design;
            }
            Some(Target::Op(k)) => {
                self.sel = Sel::Op(k);
                self.tab = Tab::Design;
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------------------------
    // Guide

    fn guide(&mut self, ui: &mut egui::Ui, project: &Project, active: Option<&CellRef>, pal: &Palette) {
        ui.set_max_width(ui.available_width().min(800.0));
        ui.label(RichText::new("Making a PCell from a drawn layout").heading().color(pal.text_strong));
        ui.label(
            RichText::new(
                "A parametric cell (PCell) is a layout that redraws itself for the values its user picks, such as width, \
                 length or finger count. Here you start from a cell that is already drawn and DRC clean, and describe how \
                 it changes: lines that stretch it, parts that repeat, parts that are optional, and contacts that re-array. \
                 Every change is previewed and checked with DRC at the extremes before you save it.",
            )
            .color(pal.text),
        );
        ui.add_space(10.0);
        let loaded = self.tpl.is_some();
        step(ui, 1, "Load the drawn cell", pal, |ui| {
            ui.label(
                "Open the finished layout in the editor and load it. Draw it at typical values, clean of DRC errors. For a \
                 multi-finger device, draw two or three identical fingers so the pitch can be read.",
            );
            match (active, &self.source) {
                (Some(a), _) => {
                    if ui.button(format!("Load {a}")).clicked() {
                        self.load(project, a);
                    }
                }
                (None, None) => {
                    ui.label(RichText::new("No layout cell is open.").small().color(pal.warning));
                }
                _ => {}
            }
            if let Some(s) = &self.source {
                ui.label(RichText::new(format!("Loaded: {s}")).small().color(pal.ok));
            }
        });
        if !loaded {
            concepts(ui, pal);
            return;
        }
        let t = self.tpl.as_ref().expect("loaded");
        let fills = t.ops.iter().filter(|o| matches!(o, Op::ContactFill { .. })).count();
        let stretches = t.ops.iter().filter(|o| matches!(o, Op::Stretch { .. })).count();
        let repeats = t.ops.iter().filter(|o| matches!(o, Op::Repeat { .. })).count();
        let mut run_sug: Option<u8> = None;
        step(ui, 2, "Let contacts re-array", pal, |ui| {
            ui.label(
                "A stretched contact is a DRC error: cuts have one fixed size. A contact fill owns an area and fills it with \
                 as many cuts as fit, at the via's size and spacing, after the cell is stretched.",
            );
            ui.horizontal(|ui| {
                if ui.button("Contacts to fills").clicked() {
                    run_sug = Some(0);
                }
                ui.label(RichText::new(format!("{fills} fill(s) now")).small().color(pal.text_weak));
            });
        });
        step(ui, 3, "Repeat fingers or units", pal, |ui| {
            ui.label(
                "A repeat copies what lies inside its region count times, pitch apart. Shapes that span the region (the \
                 diffusion, implant, gate strap) stretch with the copies. For a MOS, the fingers can be detected: the \
                 drawing is cut down to the fewest fingers that work and nf repeats the last one, with the drawn count as \
                 default so nothing changes until nf does.",
            );
            ui.horizontal(|ui| {
                if ui.button("Detect fingers (nf)").clicked() {
                    run_sug = Some(1);
                }
                ui.label(RichText::new(format!("{repeats} repeat(s) now")).small().color(pal.text_weak));
            });
        });
        step(ui, 4, "Place stretch lines", pal, |ui| {
            ui.label(
                "A stretch line crosses the cell; everything on its moving side shifts by the change of its amount from the \
                 default, so the shapes it crosses grow. Put W lines through the middle of the diffusion and L lines through \
                 the gate. Shapes that must not grow (contacts, labels) should not cross a line.",
            );
            ui.horizontal(|ui| {
                if ui.button("Width stretch (w)").clicked() {
                    run_sug = Some(2);
                }
                if ui.button("Length stretch (l)").clicked() {
                    run_sug = Some(3);
                }
                ui.label(RichText::new(format!("{stretches} line(s) now")).small().color(pal.text_weak));
            });
        });
        step(ui, 5, "Set parameter ranges", pal, |ui| {
            ui.label(
                "Give every parameter a minimum and maximum: they are what the corner check tests and what users of the \
                 PCell may enter. Expressions combine parameters, e.g. (nf - 1) * 0.72 or if(guard, 0.5, 0).",
            );
        });
        step(ui, 6, "Check the corners", pal, |ui| {
            ui.label("Run DRC on the cell at the defaults and at the extremes of every parameter.");
            if ui.button("Run corner check").clicked() {
                self.start_corners(ui.ctx(), project);
            }
        });
        step(ui, 7, "Save and place", pal, |ui| {
            ui.label(
                "Save PCell stores the master cell (its layout is the default variant). In the layout editor, the instance \
                 tool lists it under Designed PCells; placed instances can be edited in the inspector and stretched by their \
                 handles. Saving again updates every placed variant. Export SKILL writes the same PCell as a \
                 pcDefinePCell for Cadence Virtuoso.",
            );
        });
        concepts(ui, pal);
        if let Some(s) = run_sug {
            let tech = &project.tech;
            let t = self.tpl.as_mut().expect("loaded");
            let before = t.clone();
            let r = match s {
                0 => {
                    let n = t.suggest_contact_fills(tech);
                    if n > 0 { Ok(format!("{n} contact fill(s) made")) } else { Err("No via cuts found".into()) }
                }
                1 => t.suggest_fingers(tech),
                2 => t.suggest_width(tech),
                _ => t.suggest_length(tech),
            };
            match r {
                Ok(m) => {
                    self.undo.push(before);
                    self.status = Some((true, m));
                    self.touch();
                }
                Err(e) => self.status = Some((false, e)),
            }
        }
    }
}

fn concepts(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(10.0);
    ui.label(RichText::new("Good practice").strong().color(pal.text_strong));
    for tip in [
        "Stretch lines apply in list order, and each one moves the lines and regions after it.",
        "A W line through the middle of the diffusion stretches the gates, contacts and metal straps together.",
        "Keep contacts as fills: the corner check flags stretched cuts as width errors.",
        "Use a condition for optional parts (a guard ring, dummy gates, a body tap).",
        "Name parameters as the schematic does (w, l, nf, m) so LVS and the schematic match.",
        "Lengths are in µm and snapped to the manufacturing grid.",
    ] {
        ui.label(RichText::new(format!("· {tip}")).color(pal.text));
    }
}

/// Rename parameter `from` to `to` in every expression.
fn rename_in_ops(ops: &mut [Op], from: &str, to: &str) {
    let swap = |s: &mut String| {
        let mut out = String::new();
        let mut word = String::new();
        for c in s.chars().chain(['\0']) {
            if c.is_ascii_alphanumeric() || c == '_' {
                word.push(c);
                continue;
            }
            out.push_str(if word == from { to } else { &word });
            word.clear();
            if c != '\0' {
                out.push(c);
            }
        }
        *s = out;
    };
    for op in ops {
        match op {
            Op::Stretch { amount, .. } => swap(amount),
            Op::Repeat { count, pitch, .. } => {
                swap(count);
                swap(pitch);
            }
            Op::Condition { when, .. } => swap(when),
            Op::ContactFill { .. } => {}
        }
    }
}

fn explain(op: &Op) -> &'static str {
    match op {
        Op::Stretch { .. } => {
            "Everything on the moving side shifts by the change of the amount from its default value; shapes crossing the line grow."
        }
        Op::Repeat { .. } => {
            "Items wholly inside the region are copied count times, pitch apart; shapes that span the region stretch to cover the copies."
        }
        Op::Condition { .. } => "Items wholly inside the region are left out when the expression is 0 (false).",
        Op::ContactFill { .. } => {
            "After stretching and repeating, the area is filled with the largest centred array of via cuts."
        }
    }
}

fn first_len(t: &Template) -> String {
    t.params.iter().find(|p| p.kind == ParamKind::Length).map_or("0".into(), |p| p.name.clone())
}

fn first_int(t: &Template) -> String {
    t.params.iter().find(|p| p.kind == ParamKind::Int).map_or("1".into(), |p| p.name.clone())
}

fn first_bool(t: &Template) -> String {
    t.params.iter().find(|p| p.kind == ParamKind::Bool).map_or("1".into(), |p| p.name.clone())
}

fn unique_name(p: &Project, lib: &str, base: &str) -> String {
    let base: String = base.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    if p.cell(&CellRef::new(lib, &base)).is_none() {
        return base;
    }
    (2..).map(|k| format!("{base}{k}")).find(|n| p.cell(&CellRef::new(lib, n)).is_none()).expect("unbounded")
}

fn snap(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g.div_euclid(2)).div_euclid(g) * g }
}

fn axis_lo(a: Axis, r: &IRect) -> i64 {
    match a {
        Axis::X => r.x0,
        Axis::Y => r.y0,
    }
}

fn axis_hi(a: Axis, r: &IRect) -> i64 {
    match a {
        Axis::X => r.x1,
        Axis::Y => r.y1,
    }
}

fn axis_pick(ui: &mut egui::Ui, axis: &mut Axis) {
    let lb = ui.label("Direction");
    ui.horizontal(|ui| {
        ui.selectable_value(axis, Axis::X, "along x").labelled_by(lb.id);
        ui.selectable_value(axis, Axis::Y, "along y").labelled_by(lb.id);
    });
    ui.end_row();
}

fn len_edit(ui: &mut egui::Ui, v: &mut i64, tech: &Tech, label: egui::Id) {
    let k = tech.dbu_per_um.max(1) as f64;
    let mut f = *v as f64 / k;
    let step = tech.grid.max(1) as f64 / k;
    if ui.add(egui::DragValue::new(&mut f).speed(step).max_decimals(4).suffix(" µm")).labelled_by(label).changed() {
        *v = snap((f * k).round() as i64, tech.grid);
    }
}

fn region_row(ui: &mut egui::Ui, label: &str, r: &mut IRect, tech: &Tech, pick: &mut Option<Pick>) {
    let lb = ui.label(label);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            for (name, v) in [("x0", &mut r.x0), ("y0", &mut r.y0), ("x1", &mut r.x1), ("y1", &mut r.y1)] {
                let l = ui.label(RichText::new(name).small());
                len_edit(ui, v, tech, l.id);
            }
        });
        if ui.selectable_label(*pick == Some(Pick::Region), "Draw on the drawing").labelled_by(lb.id).clicked() {
            *pick = if *pick == Some(Pick::Region) { None } else { Some(Pick::Region) };
        }
    });
    if r.x0 > r.x1 {
        std::mem::swap(&mut r.x0, &mut r.x1);
    }
    if r.y0 > r.y1 {
        std::mem::swap(&mut r.y0, &mut r.y1);
    }
    ui.end_row();
}

fn expr_edit(ui: &mut egui::Ui, label: &str, text: &mut String, params: &[String], hint: &str) {
    let lb = ui.label(label);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(text).desired_width(220.0).font(egui::TextStyle::Monospace)).labelled_by(lb.id);
            ui.menu_button("Insert", |ui| {
                for p in params {
                    if ui.button(p).clicked() {
                        if !text.is_empty() && !text.ends_with(' ') {
                            text.push(' ');
                        }
                        text.push_str(p);
                        ui.close();
                    }
                }
                if params.is_empty() {
                    ui.label("No parameters yet");
                }
            });
        });
        let pal = theme::palette(ui.ctx());
        match ics_core::pcell_design::parse_expr(text) {
            Ok(_) => ui.label(RichText::new(hint).small().color(pal.text_weak)),
            Err(e) => ui.label(RichText::new(e).small().color(pal.danger)),
        };
    });
    ui.end_row();
}

fn layers_edit(ui: &mut egui::Ui, layers: &mut Vec<usize>, tech: &Tech) {
    let lb = ui.label("Layers");
    let text = if layers.is_empty() {
        "all".to_string()
    } else {
        layers.iter().filter_map(|&l| tech.layer(l)).map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")
    };
    ui.menu_button(text, |ui| {
        if ui.button("All layers").clicked() {
            layers.clear();
        }
        ui.separator();
        for (i, d) in tech.layers.iter().enumerate() {
            let mut on = layers.contains(&i);
            if ui.checkbox(&mut on, &d.name).changed() {
                if on {
                    layers.push(i);
                    layers.sort_unstable();
                } else {
                    layers.retain(|&l| l != i);
                }
            }
        }
    })
    .response
    .labelled_by(lb.id);
    ui.end_row();
}

fn dot(ui: &mut egui::Ui, sev: Option<Level>, pal: &Palette) {
    let c = match sev {
        Some(Level::Error) => pal.danger,
        Some(Level::Warning) => pal.warning,
        _ => Color32::TRANSPARENT,
    };
    let (r, p) = ui.allocate_painter(Vec2::splat(8.0), Sense::hover());
    p.circle_filled(r.rect.center(), 3.5, c);
}

fn finding_row(ui: &mut egui::Ui, f: &Finding, pal: &Palette) {
    let (tag, c) = match f.level {
        Level::Error => ("error", pal.danger),
        Level::Warning => ("warning", pal.warning),
        Level::Info => ("note", pal.text_weak),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tag).small().strong().color(c));
        ui.label(RichText::new(&f.message).small().color(pal.text));
    });
}

fn card(ui: &mut egui::Ui, pal: &Palette, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(pal.bg_sidebar).stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(10).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        },
    );
}

/// A numbered step of the guide.
fn step(ui: &mut egui::Ui, n: usize, title: &str, pal: &Palette, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(pal.bg_sidebar).stroke(Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(10).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, p) = ui.allocate_painter(Vec2::splat(24.0), Sense::hover());
                p.circle_filled(r.rect.center(), 11.0, pal.accent);
                p.text(
                    r.rect.center(),
                    Align2::CENTER_CENTER,
                    n.to_string(),
                    FontId::proportional(13.0),
                    theme::on_fill(pal.accent),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong().color(pal.text_strong));
                    body(ui);
                });
            });
        },
    );
    ui.add_space(6.0);
}

fn wrap(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for w in s.split_whitespace() {
        if col + w.len() > n && col > 0 {
            out.push('\n');
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(w);
        col += w.len();
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Drawing

/// World (dbu, y up) to screen mapping that fits `bb` into `rect`.
struct Fit {
    rect: Rect,
    bb: IRect,
    s: f32,
}

impl Fit {
    fn new(rect: Rect, bb: IRect) -> Fit {
        let w = (bb.x1 - bb.x0).max(1) as f32;
        let h = (bb.y1 - bb.y0).max(1) as f32;
        let s = (rect.width() / w).min(rect.height() / h).max(1e-9);
        Fit { rect, bb, s }
    }

    fn mid(&self) -> (f32, f32) {
        ((self.bb.x0 + self.bb.x1) as f32 / 2.0, (self.bb.y0 + self.bb.y1) as f32 / 2.0)
    }

    fn pt(&self, x: i64, y: i64) -> Pos2 {
        let c = self.rect.center();
        let (mx, my) = self.mid();
        Pos2::new(c.x + (x as f32 - mx) * self.s, c.y - (y as f32 - my) * self.s)
    }

    fn world(&self, p: Pos2) -> (i64, i64) {
        let c = self.rect.center();
        let (mx, my) = self.mid();
        (((p.x - c.x) / self.s + mx).round() as i64, (my - (p.y - c.y) / self.s).round() as i64)
    }

    fn r(&self, r: &IRect) -> Rect {
        Rect::from_two_pos(self.pt(r.x0, r.y0), self.pt(r.x1, r.y1))
    }
}

fn grow(bb: IRect, f: f32) -> IRect {
    let m = (((bb.x1 - bb.x0).max(bb.y1 - bb.y0)) as f32 * f) as i64;
    IRect { x0: bb.x0 - m, y0: bb.y0 - m, x1: bb.x1 + m, y1: bb.y1 + m }
}

fn clip(p: Pos2, r: Rect) -> Pos2 {
    Pos2::new(p.x.clamp(r.left(), r.right()), p.y.clamp(r.top(), r.bottom()))
}

fn bbox_all(l: &Layout, src: &Project) -> Option<IRect> {
    ics_core::hier::flatten(l, src, 16).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b))
}

/// Shape edges (x and y) for snapping picks.
fn edge_coords(l: &Layout) -> (Vec<i64>, Vec<i64>) {
    let (mut xs, mut ys) = (vec![], vec![]);
    for s in &l.shapes {
        let b = s.bbox();
        xs.extend([b.x0, b.x1, (b.x0 + b.x1) / 2]);
        ys.extend([b.y0, b.y1, (b.y0 + b.y1) / 2]);
    }
    (xs, ys)
}

fn draw_layout(p: &egui::Painter, fit: &Fit, l: &Layout, src: &Project, pal: &Palette, alpha: f32) {
    let tech = &src.tech;
    let mut flat = ics_core::hier::flatten(l, src, 16);
    // Big shapes (wells, implants) first so small ones stay visible.
    flat.sort_by_key(|s| std::cmp::Reverse(s.geom.bbox().width() as i128 * s.geom.bbox().height() as i128));
    for s in &flat {
        let c = tech.layer(s.layer).map_or(pal.text_weak, |d| layer_color(d, pal.dark));
        match &s.geom {
            Geometry::Label { text, at, .. } => {
                p.text(fit.pt(at.0, at.1), Align2::LEFT_BOTTOM, text, FontId::proportional(11.0), c.gamma_multiply(alpha));
            }
            Geometry::Polygon { points } => {
                let pts: Vec<Pos2> = points.iter().map(|&(x, y)| fit.pt(x, y)).collect();
                p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, c.gamma_multiply(alpha))));
            }
            Geometry::Path { points, width } => {
                draw_path(p, fit, points, *width, Some(c.gamma_multiply(0.28 * alpha)), Stroke::new(1.0, c.gamma_multiply(alpha)))
            }
            g => {
                for r in g.rects() {
                    let sr = fit.r(&r);
                    p.rect_filled(sr, 0.0, c.gamma_multiply(0.28 * alpha));
                    p.rect_stroke(sr, 0.0, Stroke::new(1.0, c.gamma_multiply(alpha)), StrokeKind::Middle);
                }
            }
        }
    }
}

/// A path segment by segment: axis-aligned ones as rectangles, any-angle ones as their
/// outline (a quadrilateral `width` wide).
fn draw_path(p: &egui::Painter, fit: &Fit, points: &[(i64, i64)], width: i64, fill: Option<Color32>, st: Stroke) {
    let hw = width as f64 / 2.0;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.0 == b.0 || a.1 == b.1 {
            let h = width / 2;
            let sr = fit.r(&IRect::new(a.0.min(b.0) - h, a.1.min(b.1) - h, a.0.max(b.0) + h, a.1.max(b.1) + h));
            if let Some(f) = fill {
                p.rect_filled(sr, 0.0, f);
            }
            p.rect_stroke(sr, 0.0, st, StrokeKind::Middle);
            continue;
        }
        let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = (-dy / len * hw, dx / len * hw);
        let q = |x: i64, y: i64, s: f64| fit.pt((x as f64 + nx * s).round() as i64, (y as f64 + ny * s).round() as i64);
        let quad = vec![q(a.0, a.1, 1.0), q(b.0, b.1, 1.0), q(b.0, b.1, -1.0), q(a.0, a.1, -1.0)];
        p.add(egui::Shape::convex_polygon(quad, fill.unwrap_or(Color32::TRANSPARENT), st));
    }
}

fn dashed_rect(p: &egui::Painter, r: Rect, st: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.extend(egui::Shape::dashed_line(&pts, st, 6.0, 4.0));
}

fn arrow(p: &egui::Painter, a: Pos2, b: Pos2, col: Color32) {
    let st = Stroke::new(1.6, col);
    p.line_segment([a, b], st);
    let d = (b - a).normalized();
    let n = Vec2::new(-d.y, d.x);
    p.line_segment([b, b - d * 6.0 + n * 4.0], st);
    p.line_segment([b, b - d * 6.0 - n * 4.0], st);
}

fn draw_dim(p: &egui::Painter, a: Pos2, b: Pos2, label: &str, pal: &Palette) {
    let col = pal.text_weak;
    let st = Stroke::new(1.0, col);
    p.line_segment([a, b], st);
    let d = (b - a).normalized();
    let n = Vec2::new(-d.y, d.x);
    for q in [a, b] {
        p.line_segment([q - n * 4.0, q + n * 4.0], st);
    }
    let mid = a + (b - a) / 2.0;
    let g = p.layout_no_wrap(label.to_string(), FontId::monospace(11.0), col);
    let r = Rect::from_center_size(mid, g.size() + Vec2::new(6.0, 2.0));
    p.rect_filled(r, 3.0, pal.bg_canvas);
    p.galley(r.min + Vec2::new(3.0, 1.0), g, col);
}
