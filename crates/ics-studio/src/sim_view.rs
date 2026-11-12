//! Simulation workspace: analysis setup, background runs and the waveform viewer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use ics_core::tech::ModelLib;
use ics_core::units::{format_eng, parse_spice};
use ics_core::{Analysis, Backend, Cell, DeviceKind, SimSetup};
use ics_sim::{Dataset, SimOptions, SimResults, VecKind};

use crate::schematic_view::OpAnnotation;
use crate::theme;

const TRACE_COLORS_LIGHT: [Color32; 10] = [
    Color32::from_rgb(0x1f, 0x6f, 0xeb),
    Color32::from_rgb(0xc7, 0x6a, 0x00),
    Color32::from_rgb(0x1a, 0x8a, 0x4f),
    Color32::from_rgb(0xd0, 0x2f, 0x2f),
    Color32::from_rgb(0x7c, 0x3a, 0xd8),
    Color32::from_rgb(0x00, 0x8b, 0x8b),
    Color32::from_rgb(0xc2, 0x2a, 0x84),
    Color32::from_rgb(0x6b, 0x7a, 0x00),
    Color32::from_rgb(0xa0, 0x52, 0x2d),
    Color32::from_rgb(0x55, 0x60, 0x70),
];

const TRACE_COLORS_DARK: [Color32; 10] = [
    Color32::from_rgb(0x4c, 0x9b, 0xff),
    Color32::from_rgb(0xf2, 0xa9, 0x3b),
    Color32::from_rgb(0x4c, 0xc3, 0x8a),
    Color32::from_rgb(0xf0, 0x5d, 0x5e),
    Color32::from_rgb(0xb5, 0x7b, 0xff),
    Color32::from_rgb(0x3d, 0xd6, 0xd0),
    Color32::from_rgb(0xff, 0x7e, 0xc8),
    Color32::from_rgb(0xc8, 0xd6, 0x4c),
    Color32::from_rgb(0xff, 0xb4, 0x80),
    Color32::from_rgb(0x9a, 0xa7, 0xb8),
];

#[derive(Default)]
pub struct SimState {
    pub results: Option<SimResults>,
    pub error: Option<String>,
    running: Option<(Receiver<Result<SimResults, String>>, Instant)>,
    pub tab: String,
    pub selected: BTreeMap<String, BTreeSet<String>>,
    pub last_deck: String,
    pub show_netlist: bool,
    pub filter: String,
}

pub enum SimEvent {
    Finished { ok: bool, message: String, log: String },
}

impl SimState {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn op_annotation(&self) -> Option<OpAnnotation> {
        let op = self.results.as_ref()?.dataset("op")?;
        let mut a = OpAnnotation::default();
        for v in &op.vectors {
            if let Some(n) = v.name.strip_prefix("v(").and_then(|s| s.strip_suffix(')')) {
                a.node_v.insert(n.to_string(), v.re[0]);
            }
        }
        a.node_v.insert("0".into(), 0.0);
        for d in &op.devices {
            a.device.insert(d.name.to_ascii_lowercase(), d.values.clone());
        }
        Some(a)
    }

    pub fn start(&mut self, ctx: &egui::Context, deck: String, analyses: Vec<Analysis>, opts: SimOptions) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.last_deck = deck.clone();
        std::thread::spawn(move || {
            let r = ics_sim::simulate(&deck, &analyses, &opts).map_err(|e| e.to_string());
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.running = Some((rx, Instant::now()));
        self.error = None;
    }

    /// Poll the background job; returns an event when it completes.
    pub fn poll(&mut self) -> Option<SimEvent> {
        let (rx, _) = self.running.as_ref()?;
        let r = rx.try_recv().ok()?;
        self.running = None;
        Some(match r {
            Ok(res) => {
                let msg = format!(
                    "Simulation finished in {:.1} ms on {} ({} analyses)",
                    res.elapsed_ms,
                    res.backend,
                    res.datasets.len()
                );
                let log = res.log.clone();
                // default signal selection per analysis
                for d in &res.datasets {
                    let sel = self.selected.entry(d.analysis.clone()).or_default();
                    sel.retain(|n| d.get(n).is_some());
                    if sel.is_empty() {
                        for n in default_signals(d) {
                            sel.insert(n);
                        }
                    }
                }
                if res.dataset(&self.tab).is_none() {
                    self.tab = res
                        .datasets
                        .iter()
                        .find(|d| d.analysis != "op")
                        .or(res.datasets.first())
                        .map(|d| d.analysis.clone())
                        .unwrap_or_default();
                }
                self.results = Some(res);
                SimEvent::Finished { ok: true, message: msg, log }
            }
            Err(e) => {
                self.error = Some(e.clone());
                SimEvent::Finished { ok: false, message: format!("Simulation failed: {e}"), log: String::new() }
            }
        })
    }
}

fn default_signals(d: &Dataset) -> Vec<String> {
    let mut out = vec![];
    for pref in ["v(out)", "v(in)", "v(vout)", "v(vin)"] {
        if d.get(pref).is_some() {
            out.push(pref.to_string());
        }
    }
    if out.is_empty() {
        out = d.signals().filter(|v| v.kind == VecKind::Voltage).take(3).map(|v| v.name.clone()).collect();
    }
    out
}

/// Analysis setup panel. Returns (setup changed, run requested).
pub fn setup_panel(
    ui: &mut egui::Ui,
    cell: &mut Cell,
    state: &mut SimState,
    ngspice: &Option<String>,
    models: Option<&ModelLib>,
) -> (bool, bool) {
    let pal = theme::palette(ui.ctx());
    let before = cell.sim.clone();
    let mut run = false;
    let sources: Vec<String> = cell
        .schematic
        .as_ref()
        .map(|s| {
            s.instances
                .iter()
                .filter(|i| matches!(i.kind, DeviceKind::VSource | DeviceKind::ISource))
                .map(|i| i.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let setup: &mut SimSetup = &mut cell.sim;

    ui.add_space(4.0);
    let running = state.is_running();
    let btn = egui::Button::new(
        RichText::new(if running { "⏳  Running…" } else { "▶  Run Simulation" }).size(15.0).strong().color(if running {
            pal.text_weak
        } else {
            Color32::WHITE
        }),
    )
    .fill(if running { pal.bg_hover } else { pal.simulation.gamma_multiply(0.9) })
    .min_size(egui::vec2(ui.available_width(), 34.0));
    if ui.add_enabled(!running, btn).on_hover_text("F5 / ⌘↩").clicked() {
        run = true;
    }
    ui.add_space(8.0);

    theme::section(ui, "Simulator");
    let backend_label = ui.label("Backend");
    egui::ComboBox::from_id_salt("backend")
        .width(ui.available_width() - 8.0)
        .selected_text(setup.backend.label())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut setup.backend, Backend::Builtin, Backend::Builtin.label());
            ui.selectable_value(&mut setup.backend, Backend::Ngspice, Backend::Ngspice.label());
        })
        .response
        .labelled_by(backend_label.id);
    match (setup.backend, ngspice) {
        (Backend::Ngspice, Some(v)) => {
            ui.label(RichText::new(format!("✔ {v}")).small().color(pal.ok));
        }
        (Backend::Ngspice, None) => {
            ui.label(RichText::new("✖ ngspice not found — install it or set the path in Settings").small().color(pal.danger));
        }
        _ if models.is_some() => {
            ui.label(RichText::new("✖ PDK models need ngspice").small().color(pal.danger));
        }
        _ => {
            ui.label(RichText::new("Fast MNA engine · R C L V I D MOS(L1)").small().color(pal.text_weak));
        }
    }
    if let Some(m) = models.filter(|m| !m.corners.is_empty()) {
        let default = if m.default_corner.is_empty() { &m.corners[0] } else { &m.default_corner };
        let shown = |c: &str| if c.is_empty() { format!("default ({default})") } else { c.to_string() };
        let l = ui.label("Corner");
        egui::ComboBox::from_id_salt("corner")
            .width(ui.available_width() - 8.0)
            .selected_text(shown(&setup.corner))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut setup.corner, String::new(), shown(""));
                for c in &m.corners {
                    ui.selectable_value(&mut setup.corner, c.clone(), c.as_str());
                }
            })
            .response
            .labelled_by(l.id);
    }
    ui.horizontal(|ui| {
        let l = ui.label("Temperature");
        ui.add(egui::DragValue::new(&mut setup.temp).range(-55.0..=200.0).suffix(" °C")).labelled_by(l.id);
    });
    let has_layout = cell.layout.is_some();
    ui.add_enabled(has_layout, egui::Checkbox::new(&mut cell.sim.parasitics, "Include layout parasitics"))
        .on_hover_text("Extract wire capacitance from the layout and add it to the matching schematic nets")
        .on_disabled_hover_text("This cell has no layout");
    let setup: &mut SimSetup = &mut cell.sim;
    ui.add_space(6.0);
    theme::section(ui, "Analyses");
    for entry in &mut setup.analyses {
        let title = entry.analysis.title();
        egui::Frame::new()
            .fill(if entry.enabled { pal.bg_widget } else { pal.bg_panel })
            .stroke(egui::Stroke::new(1.0, if entry.enabled { pal.simulation.gamma_multiply(0.5) } else { pal.border }))
            .corner_radius(6)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.checkbox(&mut entry.enabled, RichText::new(title).strong());
                if !entry.enabled {
                    return;
                }
                let field = |ui: &mut egui::Ui, label: &str, v: &mut String| {
                    let l = ui.label(label);
                    let valid = parse_spice(v).is_some();
                    let mut te = egui::TextEdit::singleline(v).desired_width(90.0);
                    if !valid {
                        te = te.text_color(pal.danger);
                    }
                    ui.add(te).labelled_by(l.id);
                    ui.end_row();
                };
                egui::Grid::new(("an", title)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| match &mut entry.analysis {
                    Analysis::Op => {
                        ui.label(RichText::new("DC bias point; annotates the schematic").small().color(pal.text_weak));
                        ui.end_row();
                    }
                    Analysis::Dc { source, start, stop, step } => {
                        let l = ui.label("Source");
                        egui::ComboBox::from_id_salt("dc_src")
                            .selected_text(if source.is_empty() { "—" } else { source.as_str() })
                            .show_ui(ui, |ui| {
                                for s in &sources {
                                    ui.selectable_value(source, s.clone(), s);
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        ui.end_row();
                        field(ui, "Start", start);
                        field(ui, "Stop", stop);
                        field(ui, "Step", step);
                    }
                    Analysis::Ac { points_per_decade, fstart, fstop } => {
                        field(ui, "Start freq", fstart);
                        field(ui, "Stop freq", fstop);
                        let l = ui.label("Points/dec");
                        ui.add(egui::DragValue::new(points_per_decade).range(1..=1000)).labelled_by(l.id);
                        ui.end_row();
                    }
                    Analysis::Tran { tstep, tstop } => {
                        field(ui, "Step", tstep);
                        field(ui, "Stop time", tstop);
                    }
                });
                if let Err(e) = entry.analysis.validate() {
                    ui.label(RichText::new(e).small().color(pal.danger));
                }
            });
        ui.add_space(4.0);
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Extra SPICE lines").default_open(!setup.extra.is_empty()).show(ui, |ui| {
        let l = ui.label(RichText::new("Appended verbatim to the deck").small().color(pal.text_weak));
        ui.add(
            egui::TextEdit::multiline(&mut setup.extra)
                .code_editor()
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text(".include models.lib\n.options reltol=1e-4"),
        )
        .labelled_by(l.id);
    });
    ui.checkbox(&mut state.show_netlist, "Show netlist");
    (cell.sim != before, run)
}

fn x_formatter(kind: VecKind, log: bool) -> impl Fn(egui_plot::GridMark, &std::ops::RangeInclusive<f64>) -> String {
    move |m, _| {
        let v = if log { 10f64.powf(m.value) } else { m.value };
        format_eng(v, kind.unit())
    }
}

fn plot_dataset(ui: &mut egui::Ui, d: &Dataset, sel: &BTreeSet<String>, colors: &BTreeMap<String, Color32>) {
    let Some(scale) = d.scale_vector() else { return };
    let log = d.analysis == "ac";
    let xs: Vec<f64> = if log { scale.re.iter().map(|f| f.max(1e-30).log10()).collect() } else { scale.re.clone() };
    let xk = match scale.kind {
        VecKind::Sweep if scale.name.starts_with('v') => VecKind::Voltage,
        VecKind::Sweep if scale.name.starts_with('i') => VecKind::Current,
        k => k,
    };
    let label_fmt = move |kind: VecKind, log: bool, yunit: &'static str| {
        move |p: &egui_plot::HoverPosition<'_>| {
            let (name, pos) = match p {
                egui_plot::HoverPosition::NearDataPoint { plot_name, position, .. } => (plot_name.to_string(), *position),
                egui_plot::HoverPosition::Elsewhere { position } => (String::new(), *position),
            };
            let x = if log { 10f64.powf(pos.x) } else { pos.x };
            Some(format!("{name}\n{}\n{}", format_eng(x, kind.unit()), format_eng(pos.y, yunit)))
        }
    };
    let link = ui.scope_id().with("link");
    if log {
        let h = (ui.available_height() - 8.0) / 2.0;
        Plot::new(("mag", &d.analysis))
            .height(h)
            .legend(Legend::default())
            .x_axis_formatter(x_formatter(xk, true))
            .y_axis_label("Magnitude (dB)")
            .label_formatter(label_fmt(xk, true, "dB"))
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .show(ui, |pu| {
                for name in sel {
                    if let Some(v) = d.get(name) {
                        let pts: PlotPoints = xs.iter().zip(v.db()).map(|(x, y)| [*x, y]).collect();
                        pu.line(Line::new(name.clone(), pts).color(colors[name]).width(1.8));
                    }
                }
            });
        Plot::new(("phase", &d.analysis))
            .height(h)
            .x_axis_formatter(x_formatter(xk, true))
            .y_axis_label("Phase (°)")
            .x_axis_label("Frequency")
            .label_formatter(label_fmt(xk, true, "°"))
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .show(ui, |pu| {
                for name in sel {
                    if let Some(v) = d.get(name) {
                        let pts: PlotPoints = xs.iter().zip(v.phase_deg()).map(|(x, y)| [*x, y]).collect();
                        pu.line(Line::new(name.clone(), pts).color(colors[name]).width(1.8));
                    }
                }
            });
    } else {
        let yunit = sel.iter().filter_map(|n| d.get(n)).map(|v| v.kind.unit()).next().unwrap_or("");
        Plot::new(("wave", &d.analysis))
            .legend(Legend::default())
            .x_axis_formatter(x_formatter(xk, false))
            .y_axis_formatter(move |m, _| format_eng(m.value, yunit))
            .x_axis_label(if xk == VecKind::Time { "Time".to_string() } else { format!("{} (swept)", scale.name.to_uppercase()) })
            .label_formatter(label_fmt(xk, false, yunit))
            .show(ui, |pu| {
                for name in sel {
                    if let Some(v) = d.get(name) {
                        let pts: PlotPoints = xs.iter().zip(&v.re).map(|(x, y)| [*x, *y]).collect();
                        pu.line(Line::new(name.clone(), pts).color(colors[name]).width(1.8));
                    }
                }
            });
    }
}

fn measurements(ui: &mut egui::Ui, d: &Dataset, sel: &BTreeSet<String>, colors: &BTreeMap<String, Color32>) {
    let pal = theme::palette(ui.ctx());
    let Some(scale) = d.scale_vector() else { return };
    egui::Grid::new(("meas", &d.analysis)).striped(true).num_columns(5).spacing([18.0, 3.0]).show(ui, |ui| {
        let head = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(t).small().strong().color(pal.text_weak));
        };
        head(ui, "SIGNAL");
        if d.analysis == "ac" {
            head(ui, "GAIN @ fstart");
            head(ui, "PEAK");
            head(ui, "f −3 dB");
            head(ui, "UNITY GAIN f");
        } else {
            head(ui, "MIN");
            head(ui, "MAX");
            head(ui, "PEAK-PEAK");
            head(ui, "FINAL");
        }
        ui.end_row();
        for name in sel {
            let Some(v) = d.get(name) else { continue };
            ui.label(RichText::new(name).monospace().color(colors[name]));
            if d.analysis == "ac" {
                let db = v.db();
                let g0 = db.first().copied().unwrap_or(f64::NAN);
                let peak = db.iter().cloned().fold(f64::MIN, f64::max);
                let cross = |lvl: f64| -> Option<f64> {
                    (1..db.len()).find(|&i| db[i - 1] >= lvl && db[i] < lvl).map(|i| {
                        let t = (lvl - db[i - 1]) / (db[i] - db[i - 1]);
                        let (f0, f1) = (scale.re[i - 1].log10(), scale.re[i].log10());
                        10f64.powf(f0 + t * (f1 - f0))
                    })
                };
                ui.label(format!("{g0:.2} dB"));
                ui.label(format!("{peak:.2} dB"));
                ui.label(cross(g0 - 3.0103).map(|f| format_eng(f, "Hz")).unwrap_or("—".into()));
                ui.label(if g0 > 0.0 { cross(0.0).map(|f| format_eng(f, "Hz")).unwrap_or("—".into()) } else { "—".into() });
            } else {
                let u = v.kind.unit();
                let mn = v.re.iter().cloned().fold(f64::MAX, f64::min);
                let mx = v.re.iter().cloned().fold(f64::MIN, f64::max);
                ui.label(format_eng(mn, u));
                ui.label(format_eng(mx, u));
                ui.label(format_eng(mx - mn, u));
                ui.label(format_eng(*v.re.last().unwrap_or(&0.0), u));
            }
            ui.end_row();
        }
    });
}

fn op_table(ui: &mut egui::Ui, d: &Dataset) {
    let pal = theme::palette(ui.ctx());
    egui::ScrollArea::vertical().id_salt("op_scroll").show(ui, |ui| {
        ui.columns(2, |cols| {
            theme::section(&mut cols[0], "Node voltages / branch currents");
            egui::Grid::new("op_nodes").striped(true).num_columns(2).spacing([24.0, 4.0]).show(&mut cols[0], |ui| {
                for v in &d.vectors {
                    let color = if v.kind == VecKind::Current { pal.simulation } else { pal.wire };
                    ui.label(RichText::new(&v.name).monospace().color(color));
                    ui.label(RichText::new(format_eng(v.re[0], v.kind.unit())).monospace());
                    ui.end_row();
                }
            });
            theme::section(&mut cols[1], "Device operating points");
            if d.devices.is_empty() {
                cols[1].label(
                    RichText::new("Device OP details are available with the built-in engine.").small().color(pal.text_weak),
                );
            }
            for dev in &d.devices {
                let ui = &mut cols[1];
                ui.horizontal(|ui| {
                    ui.label(RichText::new(dev.name.to_uppercase()).strong());
                    if let Some(r) = &dev.region {
                        let c = match r.as_str() {
                            "saturation" => pal.ok,
                            "triode" => pal.warning,
                            _ => pal.text_weak,
                        };
                        ui.label(RichText::new(r).small().color(c));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (k, v) in &dev.values {
                        let unit = match k.as_str() {
                            "id" | "i" => "A",
                            "vgs" | "vds" | "vth" | "vd" | "v" => "V",
                            "gm" | "gds" | "gd" => "S",
                            "p" => "W",
                            _ => "",
                        };
                        ui.label(RichText::new(format!("{k}={}", format_eng(*v, unit))).monospace().small().color(pal.text_weak));
                    }
                });
                ui.add_space(4.0);
            }
        });
    });
}

fn export_csv(d: &Dataset) -> Result<Option<std::path::PathBuf>, String> {
    let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.csv", d.analysis)).add_filter("CSV", &["csv"]).save_file()
    else {
        return Ok(None);
    };
    let mut s = String::new();
    let names: Vec<String> = d
        .vectors
        .iter()
        .flat_map(
            |v| if v.im.is_some() { vec![format!("{}_re", v.name), format!("{}_im", v.name)] } else { vec![v.name.clone()] },
        )
        .collect();
    s += &names.join(",");
    s.push('\n');
    for i in 0..d.len() {
        let row: Vec<String> = d
            .vectors
            .iter()
            .flat_map(|v| match &v.im {
                Some(im) => vec![v.re[i].to_string(), im[i].to_string()],
                None => vec![v.re[i].to_string()],
            })
            .collect();
        s += &row.join(",");
        s.push('\n');
    }
    std::fs::write(&path, s).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

/// Results area: dataset tabs, plots, signal browser and measurements.
pub fn results_panel(ui: &mut egui::Ui, state: &mut SimState) -> Option<String> {
    let pal = theme::palette(ui.ctx());
    let mut msg = None;
    if let Some((_, t0)) = &state.running {
        ui.centered_and_justified(|ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Simulating… {:.1} s", t0.elapsed().as_secs_f32()));
            });
        });
        ui.ctx().request_repaint();
        return None;
    }
    if let Some(e) = &state.error {
        egui::Frame::new().fill(pal.danger.gamma_multiply(0.12)).corner_radius(6).inner_margin(10).show(ui, |ui| {
            ui.colored_label(pal.danger, RichText::new("Simulation error").strong());
            ui.label(RichText::new(e).monospace());
        });
        ui.add_space(8.0);
    }
    let Some(res) = &state.results else {
        if state.error.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(
                        "Configure analyses on the left and press ▶ Run (F5).\nResults, waveforms and measurements appear here.",
                    )
                    .color(pal.text_weak)
                    .size(14.0),
                );
            });
        }
        return None;
    };
    ui.horizontal(|ui| {
        for d in &res.datasets {
            let label = match d.analysis.as_str() {
                "op" => "Operating Point",
                "dc" => "DC Sweep",
                "ac" => "AC",
                "tran" => "Transient",
                other => other,
            };
            if ui.selectable_label(state.tab == d.analysis, label).clicked() {
                state.tab = d.analysis.clone();
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(d) = res.dataset(&state.tab)
                && ui.button("⬇ CSV").on_hover_text("Export this dataset").clicked()
            {
                match export_csv(d) {
                    Ok(Some(p)) => msg = Some(format!("Exported {}", p.display())),
                    Ok(None) => {}
                    Err(e) => msg = Some(format!("Export failed: {e}")),
                }
            }
            ui.label(RichText::new(format!("{} · {:.1} ms", res.backend, res.elapsed_ms)).small().color(pal.text_weak));
        });
    });
    ui.separator();
    let Some(d) = res.dataset(&state.tab) else { return msg };
    if d.analysis == "op" {
        op_table(ui, d);
        return msg;
    }
    let sel = state.selected.entry(d.analysis.clone()).or_default();
    let mut colors = BTreeMap::new();
    let trace = if pal.dark { &TRACE_COLORS_DARK } else { &TRACE_COLORS_LIGHT };
    for (i, v) in d.signals().enumerate() {
        colors.insert(v.name.clone(), trace[i % trace.len()]);
    }
    egui::Panel::right("signals").resizable(true).default_size(190.0).show(ui, |ui| {
        theme::section(ui, "Signals");
        let fl = ui.label(RichText::new("Filter").small().color(pal.text_weak));
        ui.add(egui::TextEdit::singleline(&mut state.filter).hint_text("v(out)…").desired_width(f32::INFINITY))
            .labelled_by(fl.id);
        ui.horizontal(|ui| {
            if ui.small_button("all V").clicked() {
                *sel = d.signals().filter(|v| v.kind == VecKind::Voltage).map(|v| v.name.clone()).collect();
            }
            if ui.small_button("clear").clicked() {
                sel.clear();
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            for v in d.signals() {
                if !state.filter.is_empty() && !v.name.contains(&state.filter.to_ascii_lowercase()) {
                    continue;
                }
                let mut on = sel.contains(&v.name);
                ui.horizontal(|ui| {
                    let (r, p) = ui.allocate_painter(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    p.rect_filled(r.rect, 2.0, if on { colors[&v.name] } else { pal.bg_hover });
                    if ui.checkbox(&mut on, RichText::new(&v.name).monospace()).changed() {
                        if on {
                            sel.insert(v.name.clone());
                        } else {
                            sel.remove(&v.name);
                        }
                    }
                });
            }
        });
    });
    egui::Panel::bottom("meas").resizable(true).default_size(110.0).show(ui, |ui| {
        theme::section(ui, "Measurements");
        egui::ScrollArea::vertical().show(ui, |ui| measurements(ui, d, sel, &colors));
    });
    egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(4)).show(ui, |ui| {
        plot_dataset(ui, d, sel, &colors);
    });
    msg
}
