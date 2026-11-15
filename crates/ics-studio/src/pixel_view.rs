//! Pixel Optics: quantum efficiency, crosstalk and photocurrent of an image sensor pixel stack,
//! with the photocurrent written into a current source of the pixel circuit.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use ics_optics::light::Spectrum;
use ics_optics::pixel::{COLORS, PixelResult, Settings};

use crate::theme;

/// The bundled example stack, shown until another is loaded.
const EXAMPLE: &str = include_str!("../../../pixels/bsi_1p4um.toml");

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
}

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
}

impl PixelOptics {
    pub fn show(&mut self) {
        self.open = true;
    }

    /// Run the model on the current stack text.
    pub fn run(&mut self) {
        let wavelengths: Vec<f64> = (0..=62).map(|i| 0.38 + i as f64 * 0.01).collect();
        let res = ics_optics::PixelStack::from_toml(&self.text).and_then(|st| {
            let r = ics_optics::simulate(
                &st,
                &Settings { wavelengths: wavelengths.clone(), cra: self.cra, azimuth: 0.0, rays_per_side: 32 },
            )?;
            let cut = (self.ir_cut > 0.0).then_some(self.ir_cut / 1000.0);
            let sp = Spectrum::blackbody(&wavelengths, self.cct, self.lux, cut);
            let area = st.pitch * st.pitch;
            self.currents = Some(std::array::from_fn(|c| sp.photocurrent(&r.qe[c], area)));
            Ok(r)
        });
        if res.is_err() {
            self.currents = None;
        }
        self.result = Some(res);
    }

    pub fn load(&mut self, path: PathBuf) -> Result<(), String> {
        self.text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.path = Some(path);
        self.run();
        Ok(())
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<PixelAction> {
        if !self.open {
            return None;
        }
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
        egui::Panel::left("pixel_stack")
            .resizable(true)
            .default_size(360.0)
            .frame(egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Pixel stack").strong().color(pal.text_strong));
                    if ui.button("Load…").clicked() {
                        action = Some(PixelAction::Load);
                    }
                    if ui.button("Example").clicked() {
                        self.text = EXAMPLE.to_string();
                        self.path = None;
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
            });
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
                if ui.button("Run").clicked() {
                    self.run();
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
                    Plot::new("pixel_qe")
                        .height(ui.available_height() * 0.6)
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
                            let pts: PlotPoints =
                                r.wavelengths.iter().zip(&r.crosstalk[1]).map(|(&l, &x)| [l * 1000.0, x]).collect();
                            pu.line(Line::new("crosstalk G", pts).color(pal.text_weak).style(egui_plot::LineStyle::dashed_loose()));
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
                                "At 530 nm: collected {:.0} %, reflected {:.0} %, colour filter {:.0} %, metal {:.0} %, silicon beyond the photodiode {:.0} %",
                                b.collected * 100.0,
                                b.reflected * 100.0,
                                b.color_filter * 100.0,
                                b.metal * 100.0,
                                (b.silicon_shallow + b.silicon_deep) * 100.0
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
