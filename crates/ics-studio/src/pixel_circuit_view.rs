//! Pixel Circuit: the image sensor pixels found in a layout (photodiodes, TG / FD / RST / SF /
//! SEL) and their characterisation by simulation, run on a worker thread: FD capacitance,
//! conversion gain, follower gain, noise, and per photodiode the capacitance, transfer and lag,
//! full well and dark current.

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{self, RichText};
use ics_core::extract::Extraction;
use ics_core::pixel_circuit::{INJECT_ELECTRONS, PixelCircuit, Testbench, classify};
use ics_core::{CellRef, Project};
use ics_sim::pixel::PixelMetrics;

use crate::theme::{self, Palette};

/// Pixels characterised per run (an array has many identical ones).
pub const MAX_PIXELS: usize = 16;

type Outcome = Vec<(usize, Result<PixelMetrics, String>)>;

pub struct PixelCircuitView {
    pub open: bool,
    /// Bench settings: temperature (°C), supply (V), column capacitance (pF).
    pub temp: f64,
    pub vdd: f64,
    pub ccol_pf: f64,
    /// The cell analysed, its extraction and pixels.
    pub cell: Option<CellRef>,
    extraction: Option<(Extraction, PixelCircuit, String)>,
    pub results: Outcome,
    pub error: Option<String>,
    job: Option<Receiver<Outcome>>,
}

impl Default for PixelCircuitView {
    fn default() -> Self {
        PixelCircuitView {
            open: false,
            temp: 27.0,
            vdd: 1.8,
            ccol_pf: 1.0,
            cell: None,
            extraction: None,
            results: Vec::new(),
            error: None,
            job: None,
        }
    }
}

impl PixelCircuitView {
    /// Extract the layout of `cell` and find its pixels (no simulation yet).
    pub fn load(&mut self, project: &Project, cell: &CellRef) {
        self.open = true;
        self.cell = Some(cell.clone());
        self.results.clear();
        self.job = None;
        let Some(lay) = project.cell(cell).and_then(|c| c.layout.as_ref()) else {
            self.error = Some(format!("{cell} has no layout"));
            self.extraction = None;
            return;
        };
        let x = ics_core::extract::extract(lay, project, &project.tech);
        let pc = classify(&x);
        let report = pc.report(&x, &ics_core::pex::net_caps(&x, &project.tech));
        self.error = None;
        self.extraction = Some((x, pc, report));
    }

    /// Characterise the pixels (up to [`MAX_PIXELS`]) on a worker thread.
    pub fn start(&mut self, ctx: Option<&egui::Context>, project: &Project) {
        let Some((x, pc, _)) = &self.extraction else { return };
        if pc.pixels.is_empty() {
            self.error = Some("no pixel to characterise".into());
            return;
        }
        let (x, pc, tech) = (x.clone(), pc.clone(), project.tech.clone());
        let bench = Testbench { temp: self.temp, vdd: self.vdd, ccol: self.ccol_pf * 1e-12, ..Testbench::for_tech(&x, &tech) };
        let corner = self.cell.as_ref().and_then(|r| project.cell(r)).map(|c| c.sim.corner.clone()).unwrap_or_default();
        let opts = ics_sim::SimOptions { backend: ics_core::Backend::Auto, deck_dir: project.root.clone(), ..Default::default() };
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.cloned();
        std::thread::spawn(move || {
            let out: Outcome = (0..pc.pixels.len().min(MAX_PIXELS))
                .map(|k| (k, ics_sim::pixel::characterize(&x, &pc, k, &tech, &bench, &corner, &opts)))
                .collect();
            let _ = tx.send(out);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        self.results.clear();
        self.error = None;
        self.job = Some(rx);
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Wait for a running characterisation (automation and tests).
    pub fn wait(&mut self) {
        if let Some(rx) = self.job.take()
            && let Ok(r) = rx.recv()
        {
            self.results = r;
        }
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(r) => {
                    self.results = r;
                    self.job = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("the characterisation stopped".into());
                    self.job = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    /// The window; returns true when the user asks to characterise again.
    pub fn window(&mut self, ctx: &egui::Context) -> bool {
        if !self.open {
            return false;
        }
        self.poll();
        let mut open = true;
        let run = crate::float::tool_window(ctx, "pixel_circuit", "Pixel Circuit", [820.0, 620.0], &mut open, |ui| {
            let pal = theme::palette(ui.ctx());
            let mut run = false;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| run = self.ui(ui, &pal));
            });
            run
        });
        self.open &= open;
        run.unwrap_or_default()
    }

    fn ui(&mut self, ui: &mut egui::Ui, pal: &Palette) -> bool {
        let mut run = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Image sensor pixels").strong().color(pal.text_strong));
            match &self.cell {
                Some(c) => ui.label(RichText::new(format!("· {c}")).color(pal.text_weak)),
                None => ui.label(RichText::new("· open a layout cell").color(pal.warning)),
            };
        });
        ui.label(
            RichText::new(
                "Photodiodes and the transistors around them, found by connectivity; each pixel simulated alone with the \
                 technology's models.",
            )
            .small()
            .color(pal.text_weak),
        );
        ui.separator();
        theme::section(ui, "Bench");
        egui::Grid::new("pixc_bench").num_columns(6).spacing([10.0, 4.0]).show(ui, |ui| {
            let l = ui.label("Temperature (C)");
            ui.add(egui::DragValue::new(&mut self.temp).range(-55.0..=150.0).speed(1.0)).labelled_by(l.id);
            let l = ui.label("Supply (V)");
            ui.add(egui::DragValue::new(&mut self.vdd).range(0.5..=5.0).speed(0.05)).labelled_by(l.id);
            let l = ui.label("Column C (pF)");
            ui.add(egui::DragValue::new(&mut self.ccol_pf).range(0.01..=100.0).speed(0.1)).labelled_by(l.id);
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let busy = self.is_running();
            let has = self.extraction.as_ref().is_some_and(|e| !e.1.pixels.is_empty());
            if ui.add_enabled(has && !busy, egui::Button::new("Characterize")).clicked() {
                run = true;
            }
            if busy {
                ui.spinner();
                ui.label(RichText::new("simulating…").color(pal.text_weak));
            }
        });
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(pal.danger));
        }
        let Some((x, pc, report)) = &self.extraction else { return run };
        for w in &pc.warnings {
            ui.label(RichText::new(w).color(pal.warning));
        }
        if !self.results.is_empty() {
            theme::section(ui, "Pixels");
            egui::Grid::new("pixc_pixels").striped(true).num_columns(7).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in
                    ["Pixel", "FD reset (V)", "C_FD (fF)", "CG FD (µV/e-)", "CG col (µV/e-)", "SF gain", "Read noise CDS (e-)"]
                {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (k, r) in &self.results {
                    ui.label(format!("{}", k + 1));
                    match r {
                        Ok(m) => {
                            ui.label(format!("{:.3}", m.v_fd_reset));
                            ui.label(format!("{:.3}", m.c_fd * 1e15));
                            ui.label(format!("{:.1}", m.cg_fd_uv));
                            ui.label(format!("{:.1}", m.cg_col_uv));
                            ui.label(format!("{:.3}", m.sf_gain));
                            ui.label(format!("{:.2}", m.noise.readout_e * std::f64::consts::SQRT_2));
                        }
                        Err(e) => {
                            ui.label(RichText::new(e).color(pal.danger));
                        }
                    }
                    ui.end_row();
                }
            });
            theme::section(ui, "Photodiodes");
            egui::Grid::new("pixc_pds").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                let lag = format!("Lag of {INJECT_ELECTRONS:.0} (e-)");
                for h in ["Pixel", "Photodiode", "Reset (V)", "C (fF)", lag.as_str(), "Full well (e-)", "Dark (e-/s)", "Pinned"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (k, m) in self.results.iter().filter_map(|(k, r)| r.as_ref().ok().map(|m| (k, m))) {
                    for d in &m.pds {
                        ui.label(format!("{}", k + 1));
                        ui.label(format!("{} ({})", d.name, d.node));
                        ui.label(format!("{:.3}", d.v_reset));
                        ui.label(format!("{:.3}", d.c * 1e15));
                        ui.label(d.lag_e.map_or("-".into(), |l| format!("{l:.1}")));
                        let fw = format!("{:.0}{}", d.full_well_e, if d.saturated { "" } else { " (not saturated)" });
                        ui.label(fw);
                        ui.label(d.dark.map_or("-".into(), |v| format!("{:.1}", v.0 / ics_core::pixel_circuit::Q_E)));
                        ui.label(if m.pinned { "yes" } else { "no" });
                        ui.end_row();
                    }
                }
            });
            if let Some(Ok(m)) = self.results.first().map(|r| &r.1) {
                ui.label(
                    RichText::new(format!(
                        "kTC at FD {:.1} e- rms (removed by CDS); readout noise over {} pF of column. Lag with level-1 models \
                         (Meyer capacitances) is approximate: they do not conserve the channel charge.",
                        m.noise.ktc_e, self.ccol_pf
                    ))
                    .small()
                    .color(pal.text_weak),
                );
            }
        }
        theme::section(ui, &format!("Recognised ({} photodiodes, {} pixels)", x.photodiodes.len(), pc.pixels.len()));
        ui.label(RichText::new(report).monospace().small());
        run
    }
}
