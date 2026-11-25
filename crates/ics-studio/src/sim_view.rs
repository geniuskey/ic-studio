//! Simulation workspace: analysis setup, background runs and the waveform viewer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use ics_core::sim_setup::AcSweep;
use ics_core::tech::ModelLib;
use ics_core::units::{format_eng, parse_spice};
use ics_core::{Analysis, Backend, Cell, DeviceKind, SimSetup};
use ics_sim::calc::{self, CalcValue};
use ics_sim::{Dataset, SimOptions, SimResults, VecKind, Vector};

use crate::i18n::{t, tf};
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
    /// Calculator expressions plotted per analysis, re-evaluated after every run.
    pub calc: BTreeMap<String, Vec<String>>,
    /// Label of the current results.
    pub label: String,
    runs: u32,
    /// Earlier runs kept for comparison, oldest first.
    pub history: Vec<SavedRun>,
}

/// Results of an earlier run, overlaid on the plots when `shown`.
pub struct SavedRun {
    pub label: String,
    pub results: SimResults,
    pub shown: bool,
}

/// Earlier runs kept per cell.
pub const MAX_HISTORY: usize = 12;

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

    /// Evaluate a calculator expression on the `analysis` dataset. A waveform result is added to
    /// the dataset as a signal named after the expression and plotted when `plot` is set.
    pub fn eval_calc(&mut self, analysis: &str, expr: &str, plot: bool) -> Result<CalcValue, String> {
        let expr = expr.trim();
        let ds = self.results.as_mut().and_then(|r| r.datasets.iter_mut().find(|d| d.analysis == analysis));
        let ds = ds.ok_or_else(|| format!("no {analysis} results; run the simulation first"))?;
        let v = calc::eval(expr, ds)?;
        if plot && let CalcValue::Wave(w) = &v {
            if ds.analysis == "op" {
                return Err("waveforms cannot be plotted for the operating point".into());
            }
            add_wave(ds, expr, w.clone());
            self.selected.entry(analysis.to_string()).or_default().insert(expr.to_string());
            let list = self.calc.entry(analysis.to_string()).or_default();
            if !list.iter().any(|e| e == expr) {
                list.push(expr.to_string());
            }
        }
        Ok(v)
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
                let mut msg = tf(
                    "Simulation finished in {} ms on {} ({} analyses)",
                    &[&format!("{:.1}", res.elapsed_ms), &res.backend.to_string(), &res.datasets.len().to_string()],
                );
                match res.warnings.as_slice() {
                    [] => {}
                    [w] => msg += &format!(" - {w}"),
                    [w, rest @ ..] => msg += &format!(" - {}", tf("{} (+{} more in the log)", &[w, &rest.len().to_string()])),
                }
                let log = res.log.clone();
                let mut res = res;
                for d in &mut res.datasets {
                    for e in self.calc.get(&d.analysis).into_iter().flatten() {
                        if let Ok(CalcValue::Wave(w)) = calc::eval(e, d) {
                            add_wave(d, e, w);
                        }
                    }
                }
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
                if let Some(prev) = self.results.replace(res) {
                    let label = std::mem::take(&mut self.label);
                    self.history.push(SavedRun { label, results: prev, shown: false });
                    if self.history.len() > MAX_HISTORY {
                        self.history.remove(0);
                    }
                }
                self.runs += 1;
                self.label = tf("Run {}", &[&self.runs.to_string()]);
                SimEvent::Finished { ok: true, message: msg, log }
            }
            Err(e) => {
                self.error = Some(e.clone());
                SimEvent::Finished {
                    ok: false,
                    message: tf("Simulation failed: {}", &[&crate::i18n::t_text(&e)]),
                    log: String::new(),
                }
            }
        })
    }
}

/// A real calculator result in an AC dataset (e.g. `db(v(out))`): plotted as is, not in dB.
fn is_derived_real(v: &Vector) -> bool {
    v.im.is_none() && v.kind == VecKind::Other
}

fn add_wave(ds: &mut Dataset, name: &str, mut w: Vector) {
    w.name = name.to_string();
    match ds.vectors.iter_mut().find(|v| v.name == name) {
        Some(v) => *v = w,
        None => ds.vectors.push(w),
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
    let fill = if running { pal.bg_hover } else { pal.simulation };
    let btn = egui::Button::new(
        RichText::new(if running { t("⏳  Running…") } else { t("▶  Run Simulation") }).size(15.0).strong().color(if running {
            pal.text_weak
        } else {
            theme::on_fill(fill)
        }),
    )
    .fill(fill)
    .min_size(egui::vec2(ui.available_width(), 34.0));
    let b = ui.add_enabled(!running, btn);
    crate::spot::mark(ui.ctx(), "sim.run", b.rect);
    if b.on_hover_text(crate::theme::kbd("F5 / ⌘↩")).clicked() {
        run = true;
    }
    ui.add_space(8.0);

    theme::section(ui, t("Simulator"));
    let backend_label = ui.label(t("Backend"));
    egui::ComboBox::from_id_salt("backend")
        .width(ui.available_width() - 8.0)
        .selected_text(t(setup.backend.label()))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut setup.backend, Backend::Builtin, t(Backend::Builtin.label()));
            ui.selectable_value(&mut setup.backend, Backend::Ngspice, t(Backend::Ngspice.label()));
            ui.selectable_value(&mut setup.backend, Backend::Auto, t(Backend::Auto.label()));
        })
        .response
        .labelled_by(backend_label.id);
    match (setup.backend, ngspice) {
        (Backend::Ngspice, Some(v)) => {
            ui.label(RichText::new(format!("✔ {v}")).small().color(pal.ok));
        }
        (Backend::Ngspice, None) => {
            ui.label(RichText::new(t("✖ ngspice not found — install it or set the path in Settings")).small().color(pal.danger));
        }
        (Backend::Auto, v) => {
            let text = match v {
                Some(v) => tf("Built-in, {} for PDK or unsupported models", &[v]),
                None => t("Built-in (ngspice not found for PDK models)").into(),
            };
            ui.label(RichText::new(text).small().color(pal.text_weak));
        }
        _ if models.is_some() => {
            ui.label(RichText::new(t("✖ This PDK's models may need ngspice (Auto picks it)")).small().color(pal.danger));
        }
        _ => {
            ui.label(
                RichText::new(t("MNA engine · R C L V I D, MOS (L1, BSIM4), BJT, JFET, T, E F G H")).small().color(pal.text_weak),
            );
        }
    }
    if let Some(m) = models.filter(|m| !m.corners.is_empty()) {
        let default = if m.default_corner.is_empty() { &m.corners[0] } else { &m.default_corner };
        let shown = |c: &str| if c.is_empty() { tf("default ({})", &[default]) } else { c.to_string() };
        let l = ui.label(t("Corner"));
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
        let l = ui.label(t("Temperature"));
        ui.add(egui::DragValue::new(&mut setup.temp).range(-55.0..=200.0).suffix(" °C")).labelled_by(l.id);
    });
    let has_layout = cell.layout.is_some();
    ui.add_enabled(has_layout, egui::Checkbox::new(&mut cell.sim.parasitics, t("Include layout parasitics")))
        .on_hover_text(t("Extract wire capacitance from the layout and add it to the matching schematic nets"))
        .on_disabled_hover_text(t("This cell has no layout"));
    let setup: &mut SimSetup = &mut cell.sim;
    ui.add_space(6.0);
    let top = ui.cursor().min;
    theme::section(ui, t("Analyses"));
    let first = 40.0f32.min(ui.available_height());
    crate::spot::mark(ui.ctx(), "sim.analyses", egui::Rect::from_min_size(top, egui::vec2(ui.available_width(), first + 20.0)));
    for entry in &mut setup.analyses {
        let title = entry.analysis.title();
        egui::Frame::new()
            .fill(if entry.enabled { pal.bg_widget } else { pal.bg_panel })
            .stroke(egui::Stroke::new(1.0, if entry.enabled { pal.simulation.gamma_multiply(0.5) } else { pal.border }))
            .corner_radius(6)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.checkbox(&mut entry.enabled, RichText::new(t(title)).strong());
                if !entry.enabled {
                    return;
                }
                let field = |ui: &mut egui::Ui, label: &str, v: &mut String| {
                    let l = ui.label(field_label(label));
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
                        ui.label(RichText::new(t("DC bias point; annotates the schematic")).small().color(pal.text_weak));
                        ui.end_row();
                    }
                    Analysis::Dc { source, start, stop, step } => {
                        let l = ui.label(t("Source"));
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
                    Analysis::Ac { points_per_decade, fstart, fstop, sweep } => {
                        field(ui, "Start freq", fstart);
                        field(ui, "Stop freq", fstop);
                        let l = ui.label(t("Sweep"));
                        egui::ComboBox::from_id_salt("ac_sweep")
                            .selected_text(sweep.keyword())
                            .show_ui(ui, |ui| {
                                for s in [AcSweep::Dec, AcSweep::Oct, AcSweep::Lin] {
                                    ui.selectable_value(sweep, s, s.keyword());
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label(match sweep {
                            AcSweep::Dec => t("Points/dec"),
                            AcSweep::Oct => t("Points/oct"),
                            AcSweep::Lin => t("Points"),
                        });
                        ui.add(egui::DragValue::new(points_per_decade).range(1..=100_000)).labelled_by(l.id);
                        ui.end_row();
                    }
                    Analysis::Tran { tstep, tstop, tstart, tmax, uic } => {
                        field(ui, "Step", tstep);
                        field(ui, "Stop time", tstop);
                        field(ui, "Start (opt.)", tstart);
                        field(ui, "Max step (opt.)", tmax);
                        let l = ui.label(t("Use .ic (uic)"));
                        ui.checkbox(uic, "").labelled_by(l.id);
                        ui.end_row();
                    }
                    Analysis::Noise { output, source, points_per_decade, fstart, fstop } => {
                        let l = ui.label(t("Output node"));
                        ui.add(egui::TextEdit::singleline(output).desired_width(90.0)).labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label(t("Input source"));
                        egui::ComboBox::from_id_salt("noise_src")
                            .selected_text(if source.is_empty() { "—" } else { source.as_str() })
                            .show_ui(ui, |ui| {
                                for s in &sources {
                                    ui.selectable_value(source, s.clone(), s);
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        ui.end_row();
                        field(ui, "Start freq", fstart);
                        field(ui, "Stop freq", fstop);
                        let l = ui.label(t("Points/dec"));
                        ui.add(egui::DragValue::new(points_per_decade).range(1..=1000)).labelled_by(l.id);
                        ui.end_row();
                    }
                });
                if let Err(e) = entry.analysis.validate() {
                    ui.label(RichText::new(crate::i18n::t_text(&e)).small().color(pal.danger));
                }
            });
        ui.add_space(4.0);
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(t("Extra SPICE lines")).id_salt("extra_spice").default_open(!setup.extra.is_empty()).show(
        ui,
        |ui| {
            let l = ui.label(RichText::new(t("Appended verbatim to the deck")).small().color(pal.text_weak));
            ui.add(
                egui::TextEdit::multiline(&mut setup.extra)
                    .code_editor()
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text(".include models.lib\n.options reltol=1e-4"), // i18n: ignore
            )
            .labelled_by(l.id);
        },
    );
    ui.checkbox(&mut state.show_netlist, t("Show netlist"));
    (cell.sim != before, run)
}

/// An analysis field's label. "Stop" here is a sweep's end value, not the Stop button, so it
/// has its own translation.
fn field_label(en: &str) -> &str {
    if en == "Stop" && crate::i18n::current() == crate::i18n::Lang::Ko { t("Stop (end value)") } else { t(en) }
}

fn x_formatter(kind: VecKind, log: bool) -> impl Fn(egui_plot::GridMark, &std::ops::RangeInclusive<f64>) -> String {
    move |m, _| {
        let v = if log { 10f64.powf(m.value) } else { m.value };
        format_eng(v, kind.unit())
    }
}

/// A shown earlier run's dataset of the same analysis.
type Other<'a> = (&'a str, &'a Dataset);

fn overlay_line(name: String, pts: PlotPoints<'static>, color: Color32) -> Line<'static> {
    Line::new(name, pts).color(color.gamma_multiply(0.75)).width(1.4).style(egui_plot::LineStyle::dashed_loose())
}

fn xs_of(d: &Dataset, log: bool) -> Vec<f64> {
    d.scale_vector().map_or(vec![], |s| if log { s.re.iter().map(|f| f.max(1e-30).log10()).collect() } else { s.re.clone() })
}

fn plot_dataset(
    ui: &mut egui::Ui,
    d: &Dataset,
    sel: &BTreeSet<String>,
    colors: &BTreeMap<String, Color32>,
    others: &[Other<'_>],
) {
    let Some(scale) = d.scale_vector() else { return };
    let log = matches!(d.analysis.as_str(), "ac" | "noise");
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
            .y_axis_label(t("Magnitude (dB)"))
            .label_formatter(label_fmt(xk, true, "dB"))
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .show(ui, |pu| {
                for name in sel {
                    if let Some(v) = d.get(name) {
                        let ys = if is_derived_real(v) { v.re.clone() } else { v.db() };
                        let pts: PlotPoints = xs.iter().zip(ys).map(|(x, y)| [*x, y]).collect();
                        pu.line(Line::new(name.clone(), pts).color(colors[name]).width(1.8));
                    }
                    for (label, o) in others {
                        if let Some(v) = o.get(name) {
                            let ys = if is_derived_real(v) { v.re.clone() } else { v.db() };
                            let pts: PlotPoints = xs_of(o, true).into_iter().zip(ys).map(|(x, y)| [x, y]).collect();
                            pu.line(overlay_line(format!("{label}: {name}"), pts, colors[name]));
                        }
                    }
                }
            });
        Plot::new(("phase", &d.analysis))
            .height(h)
            .x_axis_formatter(x_formatter(xk, true))
            .y_axis_label(t("Phase (°)"))
            .x_axis_label(t("Frequency"))
            .label_formatter(label_fmt(xk, true, "°"))
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .show(ui, |pu| {
                for name in sel {
                    if let Some(v) = d.get(name).filter(|v| !is_derived_real(v)) {
                        let pts: PlotPoints = xs.iter().zip(v.phase_deg()).map(|(x, y)| [*x, y]).collect();
                        pu.line(Line::new(name.clone(), pts).color(colors[name]).width(1.8));
                    }
                    for (label, o) in others {
                        if let Some(v) = o.get(name).filter(|v| !is_derived_real(v)) {
                            let pts: PlotPoints = xs_of(o, true).into_iter().zip(v.phase_deg()).map(|(x, y)| [x, y]).collect();
                            pu.line(overlay_line(format!("{label}: {name}"), pts, colors[name]));
                        }
                    }
                }
            });
    } else {
        let yunit = sel.iter().filter_map(|n| d.get(n)).map(|v| v.kind.unit()).next().unwrap_or("");
        Plot::new(("wave", &d.analysis))
            .legend(Legend::default())
            .x_axis_formatter(x_formatter(xk, false))
            .y_axis_formatter(move |m, _| format_eng(m.value, yunit))
            .x_axis_label(if xk == VecKind::Time {
                t("Time").to_string()
            } else {
                tf("{} (swept)", &[&scale.name.to_uppercase()])
            })
            .label_formatter(label_fmt(xk, false, yunit))
            .show(ui, |pu| {
                for name in sel {
                    if let Some(v) = d.get(name) {
                        let pts: PlotPoints = xs.iter().zip(&v.re).map(|(x, y)| [*x, *y]).collect();
                        pu.line(Line::new(name.clone(), pts).color(colors[name]).width(1.8));
                    }
                    for (label, o) in others {
                        if let Some(v) = o.get(name) {
                            let pts: PlotPoints = xs_of(o, false).into_iter().zip(&v.re).map(|(x, y)| [x, *y]).collect();
                            pu.line(overlay_line(format!("{label}: {name}"), pts, colors[name]));
                        }
                    }
                }
            });
    }
}

fn measurements(
    ui: &mut egui::Ui,
    d: &Dataset,
    sel: &BTreeSet<String>,
    colors: &BTreeMap<String, Color32>,
    others: &[Other<'_>],
) {
    let pal = theme::palette(ui.ctx());
    if d.scale_vector().is_none() {
        return;
    }
    egui::Grid::new(("meas", &d.analysis)).striped(true).num_columns(5).spacing([18.0, 3.0]).show(ui, |ui| {
        let head = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(crate::i18n::t(t)).small().strong().color(pal.text_weak));
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
        let rows = sel.iter().flat_map(|name| {
            std::iter::once((name.clone(), name, d, colors[name])).chain(
                others.iter().map(move |(label, o)| (format!("{label}: {name}"), name, *o, colors[name].gamma_multiply(0.75))),
            )
        });
        for (shown, name, ds, color) in rows {
            let (Some(v), Some(scale)) = (ds.get(name), ds.scale_vector()) else { continue };
            ui.label(RichText::new(&shown).monospace().color(color));
            if d.analysis == "ac" {
                let db = if is_derived_real(v) { v.re.clone() } else { v.db() };
                let g0 = db.first().copied().unwrap_or(f64::NAN);
                let peak = db.iter().cloned().fold(f64::MIN, f64::max);
                let hz = |f: Option<f64>| f.map(|f| format_eng(f, "Hz")).unwrap_or("—".into());
                ui.label(format!("{g0:.2} dB"));
                ui.label(format!("{peak:.2} dB"));
                ui.label(hz(ics_sim::calc::bandwidth_at(&scale.re, &db, ics_sim::calc::HALF_POWER_DB)));
                ui.label(if g0 > 0.0 { hz(ics_sim::calc::unity_gain_at(&scale.re, &db)) } else { "—".into() });
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
            theme::section(&mut cols[0], t("Node voltages / branch currents"));
            egui::Grid::new("op_nodes").striped(true).num_columns(2).spacing([24.0, 4.0]).show(&mut cols[0], |ui| {
                for v in &d.vectors {
                    let color = if v.kind == VecKind::Current { pal.simulation } else { pal.wire };
                    ui.label(RichText::new(&v.name).monospace().color(color));
                    ui.label(RichText::new(format_eng(v.re[0], v.kind.unit())).monospace());
                    ui.end_row();
                }
            });
            theme::section(&mut cols[1], t("Device operating points"));
            let count = |r: &str| d.devices.iter().filter(|x| x.region.as_deref() == Some(r)).count();
            let (sat, tri, off) = (count("saturation"), count("triode"), count("cutoff") + count("subthreshold"));
            if sat + tri + off > 0 {
                cols[1]
                    .label(
                        RichText::new(crate::i18n::tf(
                            "{} in saturation · {} in triode · {} off",
                            &[&sat.to_string(), &tri.to_string(), &off.to_string()],
                        ))
                        .small()
                        .color(pal.text_weak),
                    )
                    .on_hover_text(crate::i18n::t(
                        "Saturation: the transistor works as a current source (amplifiers, loads, mirrors). Triode: it works \
                         as a resistor or closed switch (logic, switches). Off: no current. Hover a device's region for advice.",
                    ));
            }
            if d.devices.is_empty() {
                cols[1].label(
                    RichText::new(t("Device OP details are available with the built-in engine.")).small().color(pal.text_weak),
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
                        let l = ui.label(RichText::new(r).small().color(c));
                        if let Some(a) = ics_core::advice::mos_advice(r, &dev.values) {
                            l.on_hover_text(crate::i18n::t_text(&a));
                        }
                    }
                });
                if let Some(r) = &dev.region
                    && r == "saturation"
                    && let Some(a) = ics_core::advice::mos_advice(r, &dev.values)
                {
                    ui.label(RichText::new(crate::i18n::t_text(&a)).small().color(pal.warning));
                }
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
    crate::spot::mark(ui.ctx(), "sim.results", ui.max_rect());
    let mut msg = None;
    if let Some((_, t0)) = &state.running {
        ui.centered_and_justified(|ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tf("Simulating… {} s", &[&format!("{:.1}", t0.elapsed().as_secs_f32())]));
            });
        });
        ui.ctx().request_repaint();
        return None;
    }
    if let Some(e) = &state.error {
        egui::Frame::new().fill(pal.danger.gamma_multiply(0.12)).corner_radius(6).inner_margin(10).show(ui, |ui| {
            ui.colored_label(pal.danger, RichText::new(t("Simulation error")).strong());
            ui.label(RichText::new(crate::i18n::t_text(e)).monospace());
        });
        ui.add_space(8.0);
    }
    let Some(res) = &state.results else {
        if state.error.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(t(
                        "Configure analyses on the left and press ▶ Run (F5).\nResults, waveforms and measurements appear here.",
                    ))
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
                "op" => t("Operating Point"),
                "dc" => t("DC Sweep"),
                "ac" => "AC",
                "tran" => t("Transient"),
                "noise" => t("Noise"),
                other => other,
            };
            if ui.selectable_label(state.tab == d.analysis, label).clicked() {
                state.tab = d.analysis.clone();
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(d) = res.dataset(&state.tab)
                && ui.button("⬇ CSV").on_hover_text(t("Export this dataset")).clicked()
            {
                match export_csv(d) {
                    Ok(Some(p)) => msg = Some(tf("Exported {}", &[&p.display().to_string()])),
                    Ok(None) => {}
                    Err(e) => msg = Some(tf("Export failed: {}", &[&e])),
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
    theme::aside(egui::Panel::right("signals").resizable(true).default_size(190.0), ui, |ui| {
        crate::spot::mark(ui.ctx(), "sim.signals", ui.max_rect());
        theme::section(ui, t("Signals"));
        let fl = ui.label(RichText::new(t("Filter")).small().color(pal.text_weak));
        let hint = "v(out)…"; // i18n: ignore
        ui.add(egui::TextEdit::singleline(&mut state.filter).hint_text(hint).desired_width(f32::INFINITY)).labelled_by(fl.id);
        ui.horizontal(|ui| {
            if ui.small_button(t("all V")).clicked() {
                *sel = d.signals().filter(|v| v.kind == VecKind::Voltage).map(|v| v.name.clone()).collect();
            }
            if ui.small_button(t("clear")).clicked() {
                sel.clear();
            }
        });
        egui::CollapsingHeader::new(tf("Compare runs ({})", &[&state.history.len().to_string()]))
            .id_salt("compare_runs")
            .default_open(state.history.iter().any(|r| r.shown))
            .show(ui, |ui| runs_ui(ui, &mut state.label, &mut state.history));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for v in d.signals() {
                if !state.filter.is_empty() && !v.name.contains(&state.filter.to_ascii_lowercase()) {
                    continue;
                }
                let mut on = sel.contains(&v.name);
                ui.horizontal(|ui| {
                    let (r, p) = ui.allocate_painter(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    p.rect_filled(r.rect, 2.0, if on { colors[&v.name] } else { pal.bg_hover });
                    let r = ui.checkbox(&mut on, RichText::new(&v.name).monospace());
                    if r.changed() {
                        if on {
                            sel.insert(v.name.clone());
                        } else {
                            sel.remove(&v.name);
                        }
                    }
                    r.context_menu(|ui| {
                        if ui.button(t("Plot Only This")).clicked() {
                            *sel = [v.name.clone()].into_iter().collect();
                        }
                        if ui.button(if sel.contains(&v.name) { t("Hide") } else { t("Show") }).clicked() && !sel.remove(&v.name)
                        {
                            sel.insert(v.name.clone());
                        }
                        let same_kind: Vec<String> = d.signals().filter(|x| x.kind == v.kind).map(|x| x.name.clone()).collect();
                        if ui.button(t("Show All of This Kind")).clicked() {
                            sel.extend(same_kind);
                        }
                        if ui.button(t("Clear Plot")).clicked() {
                            sel.clear();
                        }
                        ui.separator();
                        if ui.button(t("Copy Name")).clicked() {
                            ui.ctx().copy_text(v.name.clone());
                        }
                    });
                });
            }
        });
    });
    let others: Vec<Other<'_>> = state
        .history
        .iter()
        .filter(|r| r.shown)
        .filter_map(|r| Some((r.label.as_str(), r.results.dataset(&d.analysis)?)))
        .collect();
    egui::Panel::bottom("meas").resizable(true).default_size(110.0).show(ui, |ui| {
        crate::spot::mark(ui.ctx(), "sim.measurements", ui.max_rect());
        theme::section(ui, t("Measurements"));
        egui::ScrollArea::vertical().show(ui, |ui| measurements(ui, d, sel, &colors, &others));
    });
    egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(4)).show(ui, |ui| {
        plot_dataset(ui, d, sel, &colors, &others);
    });
    msg
}

/// Results Browser: the current run and the earlier ones kept for comparison. Checked runs are
/// overlaid (dashed) on the plots and listed in the measurements.
pub fn runs_ui(ui: &mut egui::Ui, label: &mut String, history: &mut Vec<SavedRun>) {
    let pal = theme::palette(ui.ctx());
    let l = ui.label(RichText::new(t("Current")).small().color(pal.text_weak));
    ui.add(egui::TextEdit::singleline(label).desired_width(f32::INFINITY)).labelled_by(l.id);
    if history.is_empty() {
        ui.label(RichText::new(t("Earlier runs appear here after the next simulation.")).small().color(pal.text_weak));
        return;
    }
    let mut remove = None;
    for (i, r) in history.iter_mut().enumerate().rev() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut r.shown, "").on_hover_text(t("Overlay on the plots"));
            let l = ui.label(RichText::new(format!("{} ms", r.results.elapsed_ms.round())).small().color(pal.text_weak));
            ui.add(egui::TextEdit::singleline(&mut r.label).desired_width(110.0)).labelled_by(l.id);
            if ui.small_button(t("Delete")).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        history.remove(i);
    }
}
