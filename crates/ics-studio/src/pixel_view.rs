//! Pixel Optics: quantum efficiency, crosstalk and photocurrent of an image sensor pixel stack,
//! with the photocurrent written into a current source of the pixel circuit.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use ics_optics::light::Spectrum;
use ics_optics::pixel::{COLORS, Method, PixelResult, Settings};

use crate::theme;

/// The bundled example stack, shown until another is loaded.
const EXAMPLE: &str = include_str!("../../../pixels/bsi_1p4um.toml");
/// The demo 4T pixel read from the open project's layout.
const LAYOUT_EXAMPLE: &str = include_str!("../../../pixels/aps4t_fsi.toml");

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
}

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
            let px = ics_core::pixel_layout::PixelLayout::new(p, &r, src.boundary.as_deref())?;
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
        self.stack = Some(st.clone());
        let (cra, lux, cct, ir_cut) = (self.cra, self.lux, self.cct, self.ir_cut);
        let method = if self.rcwa { Method::Rcwa { harmonics: self.harmonics } } else { Method::Ray };
        let work = move || -> Outcome {
            let wavelengths: Vec<f64> = (0..=62).map(|i| 0.38 + i as f64 * 0.01).collect();
            let res = Ok(st).and_then(|st: ics_optics::PixelStack| {
                let r = ics_optics::simulate(
                    &st,
                    &Settings { wavelengths: wavelengths.clone(), cra, azimuth: 0.0, rays_per_side: 32, method },
                )?;
                Ok((r, st.pitch))
            });
            match res {
                Ok((r, pitch)) => {
                    let cut = (ir_cut > 0.0).then_some(ir_cut / 1000.0);
                    let sp = Spectrum::blackbody(&wavelengths, cct, lux, cut);
                    let cur = std::array::from_fn(|c| sp.photocurrent(&r.qe[c], pitch * pitch));
                    (Ok(r), Some(cur))
                }
                Err(e) => (Err(e), None),
            }
        };
        if method == Method::Ray {
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
        self.job.is_some()
    }

    /// Wait for a running RCWA job (automation and tests).
    pub fn wait(&mut self) {
        if let Some(rx) = self.job.take()
            && let Ok((r, c)) = rx.recv()
        {
            self.result = Some(r);
            self.currents = c;
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
        crate::float::tool_window(ctx, "pixel_optics", "Pixel Optics", [1000.0, 640.0], &mut open, |ui| {
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
                }
                if self.is_running() {
                    ui.add(egui::Spinner::new());
                    ui.label("Running…");
                } else if ui.button("Run").clicked() {
                    action = Some(PixelAction::Run);
                }
            });
            ui.separator();
            match &self.result {
                None => {
                    ui.label(RichText::new("Run to compute the quantum efficiency of the stack.").color(pal.text_weak));
                }
                Some(Err(e)) => {
                    ui.colored_label(pal.danger, e);
                }
                Some(Ok(r)) => {
                    let colors = [Color32::from_rgb(220, 60, 60), Color32::from_rgb(60, 170, 80), Color32::from_rgb(40, 120, 60), Color32::from_rgb(60, 110, 230)];
                    let plot_h = ui.available_height() * 0.6;
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
                }
            }
        });
        action
    }
}

/// Top view of one pixel: patterned shapes per layer, the photodiode and the lens footprint.
fn plan(ui: &mut egui::Ui, st: &ics_optics::PixelStack, size: f32, pal: &theme::Palette) {
    use egui_plot::Polygon;
    let p = st.pitch;
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
            for l in &st.layers {
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
            if let Some(ml) = &st.microlens {
                let a = ml.radius.unwrap_or(p / std::f64::consts::SQRT_2);
                let (cx, cy) = (p / 2.0 + ml.offset[0], p / 2.0 + ml.offset[1]);
                let circle: Vec<[f64; 2]> = (0..=64)
                    .map(|i| {
                        let t = i as f64 / 64.0 * std::f64::consts::TAU;
                        [
                            (cx + a * t.cos()).clamp(cx - p / 2.0, cx + p / 2.0),
                            (cy + a * t.sin()).clamp(cy - p / 2.0, cy + p / 2.0),
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
