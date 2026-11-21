//! Pixel Optics: quantum efficiency, crosstalk and photocurrent of an image sensor pixel stack,
//! with the photocurrent written into a current source of the pixel circuit.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use ics_optics::calibrate::{Knob, Measured};
use ics_optics::light::Spectrum;
use ics_optics::pixel::{COLORS, Method, PixelResult, Settings};

use crate::theme;

/// The bundled example stack, shown until another is loaded.
const EXAMPLE: &str = include_str!("../../../pixels/bsi_1p4um.toml");
/// The demo 4T pixel read from the open project's layout.
const LAYOUT_EXAMPLE: &str = include_str!("../../../pixels/aps4t_fsi.toml");
/// A back-side illuminated pixel with full-depth trenches and the device model.
pub const FDTI_EXAMPLE: &str = include_str!("../../../pixels/bsi_fdti_2um.toml");
/// The demo 2 x 2 Bayer cell `bayer_fdti` with full-depth trenches, from its layout.
pub const BAYER_EXAMPLE: &str = include_str!("../../../pixels/bayer_fdti.toml");

pub struct PixelOptics {
    pub open: bool,
    /// Stack definition (TOML) being edited.
    pub text: String,
    pub path: Option<PathBuf>,
    pub cra: f64,
    pub lux: f64,
    pub cct: f64,
    /// IR-cut edge (nm), 0 = none.
    pub ir_cut: f64,
    /// Current source the photocurrent goes into, and the pixel colour it takes.
    pub source: String,
    pub color: usize,
    pub result: Option<Result<PixelResult, String>>,
    pub currents: Option<[f64; 4]>,
    /// RCWA instead of the ray model, with this many harmonics.
    pub rcwa: bool,
    pub harmonics: usize,
    job: Option<std::sync::mpsc::Receiver<Outcome>>,
    /// The stack of the last run, with its layout shapes filled in.
    pub stack: Option<ics_optics::PixelStack>,
    /// Device map shown (index into the result's maps).
    pub map: usize,
    /// The layout the last run's stack read.
    pub layout_source: Option<ics_optics::pixel::LayoutSource>,
    /// Measured QE shown over the model (file name, table).
    pub measured: Option<(String, Measured)>,
    /// Parameters to fit (`ics pixel --fit` names, comma separated) and the last fit.
    pub fit_knobs: String,
    pub fit: Option<Result<FitSummary, String>>,
    fit_job: Option<std::sync::mpsc::Receiver<FitOutcome>>,
}

/// A finished fit, for display.
#[derive(Clone)]
pub struct FitSummary {
    pub rms_before: f64,
    pub rms_after: f64,
    pub evaluations: usize,
    /// Parameter, factor, fitted value as a stack line.
    pub knobs: Vec<(String, f64, String)>,
    pub stages: Vec<String>,
    pub warnings: Vec<String>,
    /// The stack text with the fitted values written in.
    pub patched: Result<String, String>,
}

/// A fit, then the fitted stack over the full spectrum.
type FitOutcome = (Result<FitSummary, String>, Option<Outcome>);

type Outcome = (Result<PixelResult, String>, Option<[f64; 4]>);

impl Default for PixelOptics {
    fn default() -> Self {
        PixelOptics {
            open: false,
            text: EXAMPLE.to_string(),
            path: None,
            cra: 0.0,
            lux: 100.0,
            cct: 5000.0,
            ir_cut: 650.0,
            source: "I1".into(),
            color: 1,
            result: None,
            currents: None,
            rcwa: false,
            harmonics: 11,
            job: None,
            stack: None,
            map: 0,
            layout_source: None,
            measured: None,
            fit_knobs: "cf,cf_thickness".into(),
            fit: None,
            fit_job: None,
        }
    }
}

/// What the window asks the application to do.
pub enum PixelAction {
    /// Set the DC value (A) of current source `source` in the active cell's schematic.
    SetSource {
        source: String,
        amps: f64,
    },
    Load,
    Run,
    /// Build the stack of the active cell from the technology's cross section.
    FromPdk,
    /// Write the device model's photodiode figures into the technology.
    ApplyDevice,
    /// Pick a measured QE table.
    LoadMeasured,
    /// Fit the stack's parameters to the measured QE.
    Fit,
}

impl PixelOptics {
    pub fn show(&mut self) {
        self.open = true;
    }

    /// The stack of the current text, with `layout` entries filled from `project`.
    fn prepare(&self, project: Option<&ics_core::Project>) -> Result<ics_optics::PixelStack, String> {
        let mut st = ics_optics::PixelStack::from_toml(&self.text)?;
        if let Some(src) = st.layout.clone() {
            let p = project.ok_or(format!("the stack reads the layout of {}; open a project", src.cell))?;
            let r = match src.cell.split_once('/') {
                Some((lib, cell)) => ics_core::CellRef::new(lib, cell),
                None => {
                    p.all_cells().find(|(_, c)| c.name == src.cell).map(|(r, _)| r).ok_or(format!("no cell '{}'", src.cell))?
                }
            };
            let px = ics_core::pixel_layout::PixelLayout::with_params(p, &r, &src.param_list(), src.boundary.as_deref())?;
            st.fill_from_layout(px.size_um(), |e| px.rects_um(e))?;
        }
        Ok(st)
    }

    /// Run the model on the current stack text: at once for the ray model, on a worker thread
    /// for RCWA (see [`Self::is_running`]). `layout` entries read `project`.
    pub fn run(&mut self, ctx: Option<&egui::Context>, project: Option<&ics_core::Project>) {
        let st = match self.prepare(project) {
            Ok(st) => st,
            Err(e) => {
                self.result = Some(Err(e));
                self.currents = None;
                self.stack = None;
                return;
            }
        };
        self.layout_source = ics_optics::PixelStack::from_toml(&self.text).ok().and_then(|s| s.layout);
        self.stack = Some(st.clone());
        let (cra, lux, cct, ir_cut) = (self.cra, self.lux, self.cct, self.ir_cut);
        let method = if self.rcwa { Method::Rcwa { harmonics: self.harmonics } } else { Method::Ray };
        let work = move || -> Outcome { spectrum(st, cra, lux, cct, ir_cut, method) };
        // The device model takes seconds: run it on a worker thread like RCWA.
        if method == Method::Ray && self.stack.as_ref().is_none_or(|s| s.device.is_none()) {
            (self.result, self.currents) = {
                let (r, c) = work();
                (Some(r), c)
            };
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.cloned();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        self.job = Some(rx);
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some() || self.fit_job.is_some()
    }

    pub fn load_measured(&mut self, path: &std::path::Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m = Measured::from_csv(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.measured = Some((name, m));
        self.fit = None;
        Ok(())
    }

    /// Fit the parameters in `fit_knobs` to the measured QE on a worker thread, then run the
    /// fitted stack over the whole spectrum (see [`Self::wait`]).
    pub fn start_fit(&mut self, ctx: Option<&egui::Context>, project: Option<&ics_core::Project>) {
        let Some((_, m)) = self.measured.clone() else {
            self.fit = Some(Err("load a measured QE table first".into()));
            return;
        };
        let knobs: Result<Vec<Knob>, String> = self
            .fit_knobs
            .split(',')
            .filter(|k| !k.trim().is_empty())
            .map(Knob::parse)
            .collect::<Result<Vec<_>, _>>()
            .map(|v| v.concat());
        let (st, knobs) = match (self.prepare(project), knobs) {
            (Ok(st), Ok(k)) => (st, k),
            (Err(e), _) | (_, Err(e)) => {
                self.fit = Some(Err(e));
                return;
            }
        };
        self.layout_source = ics_optics::PixelStack::from_toml(&self.text).ok().and_then(|s| s.layout);
        let method = if self.rcwa { Method::Rcwa { harmonics: self.harmonics } } else { Method::Ray };
        let (cra, lux, cct, ir_cut) = (self.cra, self.lux, self.cct, self.ir_cut);
        let text = self.text.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.cloned();
        std::thread::spawn(move || {
            let base = Settings { wavelengths: vec![], cra, azimuth: 0.0, rays_per_side: 32, method };
            let out = match ics_optics::calibrate::fit(&st, &base, &m, &knobs, 200) {
                Ok(f) => {
                    let summary = FitSummary {
                        rms_before: f.rms_before.1,
                        rms_after: f.rms_after.1,
                        evaluations: f.evaluations,
                        knobs: f.knobs.clone(),
                        stages: f.stages.clone(),
                        warnings: f.warnings.clone(),
                        patched: ics_optics::calibrate::patch_stack_toml(&text, &f),
                    };
                    (Ok(summary), Some(spectrum(f.stack, cra, lux, cct, ir_cut, method)))
                }
                Err(e) => (Err(e), None),
            };
            let _ = tx.send(out);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        self.fit_job = Some(rx);
    }

    fn take_fit(&mut self, (fit, outcome): FitOutcome) {
        self.fit = Some(fit);
        if let Some((r, c)) = outcome {
            self.result = Some(r);
            self.currents = c;
        }
    }

    /// Wait for a running RCWA job (automation and tests).
    pub fn wait(&mut self) {
        if let Some(rx) = self.job.take()
            && let Ok((r, c)) = rx.recv()
        {
            self.result = Some(r);
            self.currents = c;
        }
        if let Some(rx) = self.fit_job.take()
            && let Ok(out) = rx.recv()
        {
            self.take_fit(out);
        }
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.job
            && let Ok((r, c)) = rx.try_recv()
        {
            self.result = Some(r);
            self.currents = c;
            self.job = None;
        }
        if let Some(rx) = &self.fit_job
            && let Ok(out) = rx.try_recv()
        {
            self.fit_job = None;
            self.take_fit(out);
        }
    }

    pub fn load(&mut self, path: PathBuf) -> Result<(), String> {
        self.text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.path = Some(path);
        Ok(())
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<PixelAction> {
        if !self.open {
            return None;
        }
        self.poll();
        let mut open = true;
        let mut action = None;
        crate::float::tool_window(ctx, "pixel_optics", "Pixel Optics", [1100.0, 760.0], &mut open, |ui| {
            action = self.ui(ui);
        });
        self.open &= open;
        action
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<PixelAction> {
        let pal = theme::palette(ui.ctx());
        let mut action = None;
        theme::aside(
            egui::Panel::left("pixel_stack")
                .resizable(true)
                .default_size(360.0)
                .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8)),
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Pixel stack").strong().color(pal.text_strong));
                    if ui.button("Load…").clicked() {
                        action = Some(PixelAction::Load);
                    }
                    if ui.button("BSI example").clicked() {
                        self.text = EXAMPLE.to_string();
                        self.path = None;
                    }
                    if ui
                        .button("FDTI device")
                        .on_hover_text("Back-side illuminated 2 µm pixel with full-depth trenches and the device model")
                        .clicked()
                    {
                        self.text = FDTI_EXAMPLE.to_string();
                        self.path = None;
                    }
                    if ui
                        .button("Bayer FDTI (layout)")
                        .on_hover_text("The demo 2 x 2 cell bayer_fdti from the open project's layout, with the device model")
                        .clicked()
                    {
                        self.text = BAYER_EXAMPLE.to_string();
                        self.path = None;
                    }
                    if ui
                        .button("Demo pixel (layout)")
                        .on_hover_text("The demo 4T pixel's metals and photodiode from the open project")
                        .clicked()
                    {
                        self.text = LAYOUT_EXAMPLE.to_string();
                        self.path = None;
                    }
                    if ui.button("From PDK").on_hover_text("The open cell's stack from the technology's cross section").clicked()
                    {
                        action = Some(PixelAction::FromPdk);
                    }
                });
                if let Some(p) = &self.path {
                    ui.label(RichText::new(p.display().to_string()).small().color(pal.text_weak));
                }
                egui::ScrollArea::vertical().max_height(ui.available_height() - 4.0).show(ui, |ui| {
                    let l = ui.label("TOML");
                    ui.add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_width(f32::INFINITY).desired_rows(30))
                        .labelled_by(l.id);
                });
            },
        );
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let l = ui.label("Chief ray angle (deg)");
                ui.add(egui::DragValue::new(&mut self.cra).range(0.0..=40.0).speed(0.5)).labelled_by(l.id);
                let l = ui.label("Illuminance (lux)");
                ui.add(egui::DragValue::new(&mut self.lux).range(0.0..=1e6).speed(1.0)).labelled_by(l.id);
                let l = ui.label("CCT (K)");
                ui.add(egui::DragValue::new(&mut self.cct).range(1000.0..=20000.0).speed(50.0)).labelled_by(l.id);
                let l = ui.label("IR cut (nm, 0 = none)");
                ui.add(egui::DragValue::new(&mut self.ir_cut).range(0.0..=1100.0).speed(5.0)).labelled_by(l.id);
                ui.checkbox(&mut self.rcwa, "RCWA").on_hover_text("Rigorous coupled-wave analysis: includes diffraction, slower");
                if self.rcwa {
                    let l = ui.label("Harmonics");
                    ui.add(egui::DragValue::new(&mut self.harmonics).range(3..=25).speed(0.1)).labelled_by(l.id);
                    if self.harmonics.is_multiple_of(2) {
                        self.harmonics += 1;
                    }
                    if let Some(st) = &self.stack {
                        let h = ics_optics::pixel::suggested_harmonics(st.pitch);
                        if self.harmonics < h {
                            ui.label(RichText::new(format!("{h} converge this cell")).small().color(pal.warning));
                        }
                    }
                }
                if self.is_running() {
                    ui.add(egui::Spinner::new());
                    ui.label("Running…");
                } else if ui.button("Run").clicked() {
                    action = Some(PixelAction::Run);
                }
            });
            ui.separator();
            let result = self.result.clone();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match &result {
                None => {
                    ui.label(RichText::new("Run to compute the quantum efficiency of the stack.").color(pal.text_weak));
                }
                Some(Err(e)) => {
                    ui.colored_label(pal.danger, e);
                }
                Some(Ok(r)) => {
                    let colors = [Color32::from_rgb(220, 60, 60), Color32::from_rgb(60, 170, 80), Color32::from_rgb(40, 120, 60), Color32::from_rgb(60, 110, 230)];
                    let plot_h = if r.device.is_some() { 200.0 } else { (ui.available_height() * 0.6).max(240.0) };
                    let stack = self.stack.clone();
                    ui.horizontal(|ui| {
                        let plan_w = if stack.is_some() { plot_h.min(260.0) } else { 0.0 };
                    Plot::new("pixel_qe")
                        .height(plot_h)
                        .width((ui.available_width() - plan_w - 8.0).max(200.0))
                        .legend(Legend::default())
                        .x_axis_label("wavelength (nm)")
                        .y_axis_label("QE")
                        .include_y(0.0)
                        .include_y(1.0)
                        .allow_scroll(false)
                        .show(ui, |pu| {
                            for c in [0, 1, 3] {
                                let pts: PlotPoints =
                                    r.wavelengths.iter().zip(&r.qe[c]).map(|(&l, &q)| [l * 1000.0, q]).collect();
                                pu.line(Line::new(format!("QE {}", COLORS[c]), pts).color(colors[c]).width(2.0));
                            }
                            if let Some((_, m)) = &self.measured {
                                for c in [0, 1, 3] {
                                    let pts: Vec<[f64; 2]> = m
                                        .wavelengths
                                        .iter()
                                        .zip(&m.qe[c])
                                        .filter(|(_, q)| q.is_finite())
                                        .map(|(&l, &q)| [l * 1000.0, q])
                                        .collect();
                                    if !pts.is_empty() {
                                        pu.points(
                                            egui_plot::Points::new(format!("measured {}", COLORS[c]), pts)
                                                .color(colors[c])
                                                .radius(2.5),
                                        );
                                    }
                                }
                            }
                            if r.method == "ray" {
                                let pts: PlotPoints =
                                    r.wavelengths.iter().zip(&r.crosstalk[1]).map(|(&l, &x)| [l * 1000.0, x]).collect();
                                pu.line(Line::new("crosstalk G", pts).color(pal.text_weak).style(egui_plot::LineStyle::dashed_loose()));
                            }
                            if r.crosstalk_electrical[1].iter().any(|&x| x > 0.0) {
                                let pts: PlotPoints =
                                    r.wavelengths.iter().zip(&r.crosstalk_electrical[1]).map(|(&l, &x)| [l * 1000.0, x]).collect();
                                pu.line(Line::new("electrical crosstalk G", pts).color(pal.warning).style(egui_plot::LineStyle::dotted_dense()));
                            }
                        });
                        if let Some(st) = &stack {
                            plan(ui, st, plan_w, &pal);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (c, name) in COLORS.iter().enumerate() {
                            let (k, peak) = r.qe[c].iter().enumerate().fold((0, 0.0), |m, (k, &q)| if q > m.1 { (k, q) } else { m });
                            ui.label(format!("{name} peak {:.0} % at {:.0} nm", peak * 100.0, r.wavelengths[k] * 1000.0));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Measured QE…").on_hover_text("A CSV of measured QE (wavelength in nm, R / Gr / Gb / B or G) to compare and fit").clicked() {
                            action = Some(PixelAction::LoadMeasured);
                        }
                        if let Some((name, m)) = &self.measured {
                            let rms = rms_against(r, m).map_or("-".into(), |v| format!("{v:.4}"));
                            ui.label(format!("{name}: RMS difference {rms}"));
                            let l = ui.label("Fit");
                            ui.add(egui::TextEdit::singleline(&mut self.fit_knobs).desired_width(160.0))
                                .labelled_by(l.id)
                                .on_hover_text("cf_red, cf_green, cf_blue, cf, cf_density, cf_thickness, arc, si_thickness, lens, lens_n, lens_alpha, lens_shape, qe_scale");
                            if self.fit_job.is_some() {
                                ui.add(egui::Spinner::new());
                            } else if ui.button("Fit").clicked() {
                                action = Some(PixelAction::Fit);
                            }
                        }
                    });
                    let mut write = None;
                    match &self.fit {
                        Some(Ok(f)) => {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "Fit ({} runs): RMS {:.4} -> {:.4}; the plot shows the fitted stack.",
                                    f.evaluations, f.rms_before, f.rms_after
                                ));
                                if let Ok(t) = &f.patched
                                    && ui.button("Write into the stack").on_hover_text("Put the fitted values into the stack text (comments kept)").clicked()
                                {
                                    write = Some(t.clone());
                                }
                            });
                            for st in &f.stages {
                                ui.label(RichText::new(st).small().color(pal.text_weak));
                            }
                            for w in &f.warnings {
                                ui.colored_label(pal.warning, w);
                            }
                            let mut lines: Vec<&str> = vec![];
                            for (name, factor, line) in &f.knobs {
                                if !lines.contains(&line.as_str()) {
                                    lines.push(line);
                                }
                                ui.label(RichText::new(format!("  {name}: x {factor:.3}")).color(pal.text_weak));
                            }
                            for l in lines {
                                ui.label(RichText::new(l).monospace());
                            }
                        }
                        Some(Err(e)) => {
                            ui.colored_label(pal.danger, e);
                        }
                        None => {}
                    }
                    if let Some(t) = write {
                        self.text = t;
                    }
                    if let Some(i) = r.wavelengths.iter().position(|&l| (l - 0.53).abs() < 1e-6) {
                        let b = r.budget[i];
                        ui.label(
                            RichText::new(format!(
                                "At 530 nm ({}): collected {:.0} %, reflected {:.0} %, absorbed above the silicon {:.0} %, in silicon outside the photodiode {:.0} %",
                                r.method,
                                b.collected * 100.0,
                                b.reflected * 100.0,
                                (b.color_filter + b.metal + b.other_layers) * 100.0,
                                (b.silicon_shallow + b.silicon_outside + b.silicon_deep) * 100.0
                            ))
                            .color(pal.text_weak),
                        );
                    }
                    if let Some(cur) = self.currents {
                        ui.separator();
                        ui.label(RichText::new("Photocurrent").strong().color(pal.text_strong));
                        ui.horizontal_wrapped(|ui| {
                            for (c, name) in COLORS.iter().enumerate() {
                                ui.label(format!("{name}: {}", ics_core::units::format_eng(cur[c], "A")));
                            }
                        });
                        ui.horizontal(|ui| {
                            let l = ui.label("Current source");
                            ui.add(egui::TextEdit::singleline(&mut self.source).desired_width(60.0)).labelled_by(l.id);
                            egui::ComboBox::from_label("Pixel colour").selected_text(COLORS[self.color]).show_ui(ui, |ui| {
                                for (c, name) in COLORS.iter().enumerate() {
                                    ui.selectable_value(&mut self.color, c, *name);
                                }
                            });
                            if ui.button("Apply to schematic").on_hover_text("Set the source's DC value in the active cell").clicked() {
                                action = Some(PixelAction::SetSource { source: self.source.trim().to_string(), amps: cur[self.color] });
                            }
                        });
                    }
                    if let Some(d) = &r.device {
                        ui.separator();
                        if device_section(ui, d, &mut self.map, self.layout_source.is_some(), &pal) {
                            action = Some(PixelAction::ApplyDevice);
                        }
                    }
                }
            });
        });
        action
    }
}

/// The stack over 380 - 1000 nm, and its photocurrents.
fn spectrum(st: ics_optics::PixelStack, cra: f64, lux: f64, cct: f64, ir_cut: f64, method: Method) -> Outcome {
    let wavelengths: Vec<f64> = (0..=62).map(|i| 0.38 + i as f64 * 0.01).collect();
    let settings = Settings { wavelengths: wavelengths.clone(), cra, azimuth: 0.0, rays_per_side: 32, method };
    match ics_optics::simulate(&st, &settings) {
        Ok(r) => {
            let cut = (ir_cut > 0.0).then_some(ir_cut / 1000.0);
            let sp = Spectrum::blackbody(&wavelengths, cct, lux, cut);
            let cur = std::array::from_fn(|c| sp.photocurrent(&r.qe[c], st.pitch * st.pitch));
            (Ok(r), Some(cur))
        }
        Err(e) => (Err(e), None),
    }
}

/// RMS difference between a result and measured QE, the result interpolated at the measured
/// wavelengths inside its range.
pub fn rms_against(r: &PixelResult, m: &Measured) -> Option<f64> {
    let (mut s, mut n) = (0.0, 0usize);
    let w = &r.wavelengths;
    for (k, &l) in m.wavelengths.iter().enumerate() {
        let Some(i) = w.windows(2).position(|p| p[0] <= l && l <= p[1]) else { continue };
        let f = (l - w[i]) / (w[i + 1] - w[i]);
        for c in 0..4 {
            let meas = m.qe[c][k];
            if meas.is_finite() {
                let model = r.qe[c][i] * (1.0 - f) + r.qe[c][i + 1] * f;
                s += (model - meas).powi(2);
                n += 1;
            }
        }
    }
    (n > 0).then(|| (s / n as f64).sqrt())
}

/// The device model's results: photodiode wells per colour, the charge transfer, and maps.
/// True when the figures are to be written into the technology.
fn device_section(
    ui: &mut egui::Ui,
    d: &ics_optics::device::DeviceResult,
    map: &mut usize,
    from_layout: bool,
    pal: &theme::Palette,
) -> bool {
    let mut apply = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Device model ({} nodes)", d.nodes)).strong().color(pal.text_strong));
        if from_layout
            && ui
                .button("Apply to technology")
                .on_hover_text("Write the mean pinning voltage and storage capacitance into the technology's photodiode, for the circuit simulation")
                .clicked()
        {
            apply = true;
        }
    });
    for w in &d.warnings {
        ui.colored_label(pal.warning, w);
    }
    egui::Grid::new("pixel_device").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
        for h in ["", "Vpin", "Full well", "Fill level", "C", "Collection time", "Lowest barrier (empty well)"] {
            ui.label(RichText::new(h).color(pal.text_weak));
        }
        ui.end_row();
        for (k, p) in d.pixels.iter().enumerate() {
            ui.label(RichText::new(&p.color).strong());
            ui.label(format!("{:.3} V", p.pinning_voltage));
            ui.label(format!("{:.0} e-", p.full_well));
            ui.label(format!("{:.3} V", p.full_well_level));
            ui.label(format!("{:.3} fF", p.capacitance));
            ui.label(format!("{:.1} ps", d.collection_time[k] * 1e3));
            ui.label(format!("{:.3} V to {}", p.barrier, p.barrier_to));
            ui.end_row();
        }
    });
    let h = 230.0;
    ui.horizontal(|ui| {
        if let Some(t) = &d.transfer {
            ui.vertical(|ui| {
                let f = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.2} ns"));
                ui.label(format!(
                    "Transfer of {} ({:.0} e-): 90 % in {}, 99 % in {}; {:.1} e- left ({:.2} %)",
                    t.color,
                    t.electrons,
                    f(t.t90),
                    f(t.t99),
                    t.residual,
                    100.0 * t.residual / t.electrons.max(1.0)
                ));
                Plot::new("pixel_transfer")
                    .width(360.0)
                    .height(h)
                    .legend(Legend::default())
                    .x_axis_label("log10 time (ns)")
                    .y_axis_label("electrons")
                    .include_y(0.0)
                    .allow_scroll(false)
                    .show(ui, |pu| {
                        let pts = |v: &[f64]| -> PlotPoints {
                            t.time.iter().zip(v).skip(1).map(|(&x, &y)| [x.max(1e-4).log10(), y]).collect()
                        };
                        pu.line(Line::new("in the photodiode", pts(&t.in_photodiode)).color(pal.accent).width(2.0));
                        pu.line(Line::new("arrived at n+", pts(&t.transferred)).color(pal.warning));
                    });
            });
        }
        if !d.maps.is_empty() {
            ui.vertical(|ui| {
                *map = (*map).min(d.maps.len() - 1);
                egui::ComboBox::from_label("Map").selected_text(&d.maps[*map].name).show_ui(ui, |ui| {
                    for (i, m) in d.maps.iter().enumerate() {
                        ui.selectable_value(map, i, &m.name);
                    }
                });
                heat_map(ui, &d.maps[*map], h, pal);
            });
        }
    });
    apply
}

/// A map as coloured cells with its value range; sections have depth down.
fn heat_map(ui: &mut egui::Ui, m: &ics_optics::device::Map, height: f32, pal: &theme::Palette) {
    let finite = m.values.iter().filter(|v| v.is_finite());
    let lo = finite.clone().cloned().fold(f64::INFINITY, f64::min);
    let hi = finite.cloned().fold(f64::NEG_INFINITY, f64::max);
    let aspect = (m.width / m.height.max(1e-9)) as f32;
    let (w, h) = if aspect >= 1.0 { (height * aspect.min(2.0), height) } else { (height * aspect.max(0.3), height) };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, pal.bg_sidebar);
    let (cw, ch) = (rect.width() / m.nx as f32, rect.height() / m.ny as f32);
    for j in 0..m.ny {
        for i in 0..m.nx {
            let v = m.values[j * m.nx + i];
            if !v.is_finite() {
                continue;
            }
            let t = if hi > lo { ((v - lo) / (hi - lo)) as f32 } else { 0.5 };
            // Sections list depth rows from the illuminated surface (top); plans list y up.
            let row = if m.name.contains("at depth") { m.ny - 1 - j } else { j };
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.left() + i as f32 * cw, rect.top() + row as f32 * ch),
                egui::vec2(cw + 0.5, ch + 0.5),
            );
            painter.rect_filled(r, 0.0, colormap(t));
        }
    }
    if let Some(pos) = resp.hover_pos() {
        let i = (((pos.x - rect.left()) / cw) as usize).min(m.nx - 1);
        let row = (((pos.y - rect.top()) / ch) as usize).min(m.ny - 1);
        let j = if m.name.contains("at depth") { m.ny - 1 - row } else { row };
        let v = m.values[j * m.nx + i];
        let (x, y) = ((i as f64 + 0.5) * m.width / m.nx as f64, (row as f64 + 0.5) * m.height / m.ny as f64);
        resp.on_hover_text(format!(
            "x {x:.2} µm, {} {y:.2} µm: {v:.3} {}",
            if m.name.contains("at depth") { "y" } else { "depth" },
            m.unit
        ));
    }
    ui.label(
        RichText::new(format!("{:.3} … {:.3} {}  ({:.1} x {:.1} µm)", lo, hi, m.unit, m.width, m.height))
            .small()
            .color(pal.text_weak),
    );
}

/// A perceptually ordered dark-blue → green → yellow ramp.
fn colormap(t: f32) -> Color32 {
    let stops =
        [(0.0, [48, 18, 59]), (0.25, [40, 110, 200]), (0.5, [30, 180, 140]), (0.75, [170, 220, 60]), (1.0, [250, 230, 40])];
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            let f = (t - a) / (b - a);
            let mix = |k: usize| (ca[k] as f32 + (cb[k] as f32 - ca[k] as f32) * f) as u8;
            return Color32::from_rgb(mix(0), mix(1), mix(2));
        }
    }
    Color32::from_rgb(250, 230, 40)
}

/// Top view of one pixel: patterned shapes per layer, the photodiode and the lens footprint.
fn plan(ui: &mut egui::Ui, st: &ics_optics::PixelStack, size: f32, pal: &theme::Palette) {
    use egui_plot::Polygon;
    // A stack from a 2 x 2 cell shows the whole cell.
    let p = st.rect_period();
    let rect = |r: &[f64; 4]| -> PlotPoints { vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]].into() };
    let metal = [
        Color32::from_rgb(120, 140, 200),
        Color32::from_rgb(170, 120, 200),
        Color32::from_rgb(200, 150, 90),
        Color32::from_rgb(110, 180, 180),
    ];
    Plot::new("pixel_plan")
        .width(size)
        .height(size)
        .data_aspect(1.0)
        .legend(Legend::default())
        .allow_scroll(false)
        .allow_drag(false)
        .show(ui, |pu| {
            pu.line(
                Line::new("pixel", PlotPoints::from(vec![[0.0, 0.0], [p, 0.0], [p, p], [0.0, p], [0.0, 0.0]]))
                    .color(pal.text_weak),
            );
            let mut k = 0;
            for l in st.layers.iter().chain(&st.front) {
                if let Some(pt) = &l.pattern {
                    let c = metal[k % metal.len()];
                    for r in &pt.rects {
                        pu.polygon(
                            Polygon::new(l.name.clone(), rect(r))
                                .fill_color(c.gamma_multiply(0.35))
                                .stroke(egui::Stroke::new(1.0, c)),
                        );
                    }
                    k += 1;
                }
            }
            for r in &st.photodiode.rects {
                let c = Color32::from_rgb(90, 200, 110);
                pu.polygon(
                    Polygon::new("photodiode", rect(r)).fill_color(c.gamma_multiply(0.25)).stroke(egui::Stroke::new(2.0, c)),
                );
            }
            if let Some(ml) = &st.microlens
                && !st.cell_layout
            {
                let (ax, ay) = ml.radii(p);
                // The footprint: a circle, or the superellipse |x/Rx|^n + |y/Ry|^n = 1.
                let n = match ml.shape {
                    ics_optics::pixel::LensShape::Sphere => 2.0,
                    ics_optics::pixel::LensShape::Superellipse => ml.exponents().0,
                };
                let (cx, cy) = (p / 2.0 + ml.offset[0], p / 2.0 + ml.offset[1]);
                let circle: Vec<[f64; 2]> = (0..=128)
                    .map(|i| {
                        let t = i as f64 / 128.0 * std::f64::consts::TAU;
                        let f = |c: f64| c.signum() * c.abs().powf(2.0 / n);
                        [
                            (cx + ax * f(t.cos())).clamp(cx - p / 2.0, cx + p / 2.0),
                            (cy + ay * f(t.sin())).clamp(cy - p / 2.0, cy + p / 2.0),
                        ]
                    })
                    .collect();
                pu.line(
                    Line::new("microlens", PlotPoints::from(circle))
                        .color(pal.accent)
                        .style(egui_plot::LineStyle::dashed_dense()),
                );
            }
        });
}
