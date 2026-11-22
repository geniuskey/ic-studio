//! Process Emulation window: the cross-section of the active cell's layout along a cut line,
//! built by a process flow step by step (`ics_core::process`), and an editor of the flow.
//!
//! The flow runs on a worker thread and keeps a checkpoint after every step, so moving the
//! step slider back is instant and an edit of step k runs again from step k only.

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use egui_plot::{Legend, Line, Plot, PlotImage, PlotPoint, PlotPoints, Points};
use ics_core::process::{
    CANCELLED, Checkpoint, Cut, Doping, Emulator, GENERIC180, Mask, ProcessFlow, Section, Step, cell_masks, default_cut,
};
use ics_core::process_edit::{self, OPS};
use ics_core::{CellRef, Project};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

pub enum ProcessAction {
    Load,
}

/// Memory the step checkpoints of one cut may take; beyond it every other one is dropped.
const CHECKPOINT_BUDGET: usize = 768 << 20;

/// What a run sends back.
enum Msg {
    /// The state after a step.
    Point(Checkpoint),
    Done(Result<Section, String>),
}

/// A flow running on a worker thread.
struct Job {
    rx: Receiver<Msg>,
    /// 0 while the layout is read, then 1 + the step running (`steps + 1` when done).
    stage: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    /// The label of every step of the flow.
    labels: Vec<String>,
    /// Steps to run to.
    want: usize,
    started: Instant,
    /// The cache the run fills and the steps it runs.
    cache_id: u64,
    steps: Vec<Step>,
}

/// Step checkpoints of one cell, cut, layout and flow settings.
struct Cache {
    id: u64,
    project: Arc<Project>,
    cell: CellRef,
    cut: Cut,
    /// The flow without its steps (and name and colours, which do not change the result).
    head: ProcessFlow,
    /// The steps the checkpoints were taken with.
    steps: Vec<Step>,
    /// `points[k]`: the state after `k` steps, when kept.
    points: Vec<Option<Checkpoint>>,
}

impl Cache {
    fn head(flow: &ProcessFlow) -> ProcessFlow {
        ProcessFlow { name: String::new(), colors: Default::default(), steps: vec![], ..flow.clone() }
    }

    fn store(&mut self, cp: Checkpoint) {
        let k = cp.done;
        if self.points.len() <= k {
            self.points.resize(k + 1, None);
        }
        self.points[k] = Some(cp);
        // Over the budget: drop every other checkpoint between the first and the last.
        while self.bytes() > CHECKPOINT_BUDGET {
            let kept: Vec<usize> = (1..self.points.len().saturating_sub(1)).filter(|&i| self.points[i].is_some()).collect();
            if kept.is_empty() {
                break;
            }
            for i in kept.into_iter().step_by(2) {
                self.points[i] = None;
            }
        }
    }

    fn bytes(&self) -> usize {
        let mut prev: Option<&Checkpoint> = None;
        let mut n = 0;
        for cp in self.points.iter().flatten() {
            n += cp.bytes_over(prev);
            prev = Some(cp);
        }
        n
    }
}

static CACHE_ID: AtomicU64 = AtomicU64::new(1);

pub struct ProcessView {
    pub open: bool,
    pub cell: Option<CellRef>,
    /// Flow text (TOML).
    pub text: String,
    pub cut: Cut,
    /// Steps run (the whole flow when equal to its length).
    pub steps: usize,
    /// The flow of the section shown (or being run).
    pub flow: Option<ProcessFlow>,
    pub section: Option<Section>,
    pub error: Option<String>,
    /// The step the last run started from (0 = the substrate): how much the checkpoints saved.
    pub resumed_from: Option<usize>,
    /// Show the flow editor.
    pub show_flow: bool,
    /// Edit the flow as TOML text instead of the step forms.
    edit_toml: bool,
    /// Step selected in the editor.
    pub selected: Option<usize>,
    /// `text` parsed.
    parsed: Result<ProcessFlow, String>,
    dirty: bool,
    texture: Option<TextureHandle>,
    /// x (µm along the cut) of the stack shown below the picture.
    probe: Option<f64>,
    /// Colour the semiconductors by net doping.
    pub show_doping: bool,
    job: Option<Job>,
    cache: Option<Cache>,
}

impl Default for ProcessView {
    fn default() -> Self {
        Self {
            open: false,
            cell: None,
            text: GENERIC180.to_string(),
            cut: Cut { x0: 0.0, y0: 0.0, x1: 1.0, y1: 0.0 },
            steps: usize::MAX,
            flow: None,
            section: None,
            error: None,
            resumed_from: None,
            show_flow: false,
            edit_toml: false,
            selected: None,
            parsed: ProcessFlow::from_toml(GENERIC180),
            dirty: true,
            texture: None,
            probe: None,
            show_doping: true,
            job: None,
            cache: None,
        }
    }
}

impl ProcessView {
    pub fn show(&mut self, project: &Project, cell: &CellRef) {
        if self.cell.as_ref() != Some(cell) {
            self.cell = Some(cell.clone());
            match default_cut(project, cell) {
                Ok(c) => self.cut = c,
                Err(e) => self.error = Some(e),
            }
            self.dirty = true;
        }
        self.open = true;
    }

    /// Bring the section up to date with the inputs: from the step checkpoints when they hold
    /// it, else on a worker thread from the last checkpoint before it. A run still going is
    /// cancelled first (it stops at its next step), so at most one runs at a time.
    pub fn update(&mut self, ctx: &egui::Context, project: &Project) {
        self.poll();
        if !self.dirty {
            return;
        }
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
            return;
        }
        self.dirty = false;
        let Some(cell) = self.cell.clone() else { return };
        let flow = match &self.parsed {
            Ok(f) => f.clone(),
            Err(e) => {
                self.error = Some(format!("process flow: {e}"));
                self.flow = None;
                self.section = None;
                self.texture = None;
                return;
            }
        };
        let want = self.steps.min(flow.steps.len());
        let head = Cache::head(&flow);
        let valid = self.cache.as_ref().is_some_and(|c| {
            c.cell == cell
                && c.cut == self.cut
                && c.head == head
                && c.project.tech == project.tech
                && c.project.libraries == project.libraries
        });
        if !valid {
            self.cache = Some(Cache {
                id: CACHE_ID.fetch_add(1, Ordering::Relaxed),
                project: Arc::new(project.clone()),
                cell: cell.clone(),
                cut: self.cut,
                head,
                steps: vec![],
                points: vec![],
            });
        }
        let c = self.cache.as_mut().expect("cache");
        // Checkpoints after a changed step are stale.
        let same = c.steps.iter().zip(&flow.steps).take_while(|(a, b)| a == b).count();
        c.points.truncate(same + 1);
        c.steps = flow.steps.clone();
        let start = (0..=want).rev().find(|&k| c.points.get(k).is_some_and(Option::is_some));
        self.resumed_from = Some(start.unwrap_or(0));
        self.flow = Some(flow.clone());
        if let Some(k) = start
            && k == want
        {
            self.section = c.points[k].as_ref().map(Checkpoint::section);
            self.texture = None;
            self.error = None;
            return;
        }
        let from = start.and_then(|k| c.points[k].clone());
        let labels = flow.steps.iter().map(|s| s.label()).collect();
        let stage = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (project, cut) = (c.project.clone(), self.cut);
        let (st, cn, ctx) = (stage.clone(), cancel.clone(), ctx.clone());
        let job = Job { rx, stage, cancel, labels, want, started: Instant::now(), cache_id: c.id, steps: flow.steps.clone() };
        std::thread::spawn(move || {
            let r = (|| {
                let mut mask = cell_masks(&project, &cell, &cut)?;
                let (mut e, mut prev) = match from {
                    Some(cp) => (Emulator::resume(&flow, &cp), Some(cp)),
                    None => {
                        let e = Emulator::new(&flow, cut.length())?;
                        let cp = e.checkpoint(None);
                        let _ = tx.send(Msg::Point(cp.clone()));
                        (e, Some(cp))
                    }
                };
                let mut progress = |k: usize, _: usize| {
                    st.store(k + 1, Ordering::Relaxed);
                    ctx.request_repaint();
                    !cn.load(Ordering::Relaxed)
                };
                e.run(Some(want), &mut mask, &mut progress, &mut |e| {
                    let cp = e.checkpoint(prev.as_ref());
                    let _ = tx.send(Msg::Point(cp.clone()));
                    prev = Some(cp);
                })?;
                Ok(e.into_section())
            })();
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
        self.job = Some(job);
    }

    /// Take what the run sent: its checkpoints, and its section when it is done.
    fn poll(&mut self) {
        loop {
            let Some(job) = &self.job else { return };
            match job.rx.try_recv() {
                Ok(m) => self.take(m),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.take(Msg::Done(Err("the process emulation stopped".into()))),
            }
        }
    }

    fn take(&mut self, m: Msg) {
        let Some(job) = &self.job else { return };
        match m {
            Msg::Point(cp) => {
                // Only into the cache the run was made for, while its steps are still those.
                if let Some(c) = &mut self.cache
                    && c.id == job.cache_id
                    && c.steps.len() >= cp.done
                    && job.steps.len() >= cp.done
                    && c.steps[..cp.done] == job.steps[..cp.done]
                {
                    c.store(cp);
                }
            }
            Msg::Done(r) => {
                self.job = None;
                match r {
                    Ok(s) => {
                        self.section = Some(s);
                        self.texture = None;
                        self.error = None;
                    }
                    // Cancelled: keep the last section.
                    Err(e) if e == CANCELLED => {}
                    Err(e) => {
                        self.section = None;
                        self.texture = None;
                        self.error = Some(e);
                    }
                }
            }
        }
    }

    /// Whether a flow is running.
    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Block until the run in progress ends (for tests and scripts).
    pub fn wait(&mut self) {
        while let Some(job) = &self.job {
            let m = job.rx.recv().unwrap_or_else(|_| Msg::Done(Err("the process emulation stopped".into())));
            self.take(m);
        }
    }

    /// Steps with a checkpoint kept (for tests).
    pub fn checkpoints(&self) -> Vec<usize> {
        self.cache.as_ref().map_or(vec![], |c| (0..c.points.len()).filter(|&k| c.points[k].is_some()).collect())
    }

    /// The progress line of the run: fraction done and what it is doing.
    fn progress(job: &Job, restart: bool) -> (f32, String) {
        let stage = job.stage.load(Ordering::Relaxed);
        let secs = job.started.elapsed().as_secs_f32();
        if job.cancel.load(Ordering::Relaxed) {
            let what = if restart { "Restarting with the new settings" } else { "Cancelling" };
            return (0.0, format!("{what}… ({secs:.1} s)"));
        }
        let n = job.want;
        match stage {
            0 => (0.0, format!("Reading the layout… ({secs:.1} s)")),
            k if k <= n => {
                let f = (k - 1) as f32 / n.max(1) as f32;
                (f, format!("Step {k} of {n}: {} ({secs:.1} s)", job.labels[k - 1]))
            }
            _ => (1.0, format!("Finishing… ({secs:.1} s)")),
        }
    }

    /// Load a flow file.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.steps = usize::MAX;
        self.selected = None;
        self.text_changed();
    }

    fn text_changed(&mut self) {
        self.parsed = ProcessFlow::from_toml(&self.text);
        self.dirty = true;
    }

    /// Apply an edit of the flow file (`ics_core::process_edit`).
    pub fn edit(&mut self, edit: impl FnOnce(&str) -> Result<String, String>) {
        match edit(&self.text) {
            Ok(t) => {
                self.text = t;
                self.text_changed();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Select step `k` and show the section after it.
    pub fn select(&mut self, k: usize) {
        let n = self.parsed.as_ref().map_or(0, |f| f.steps.len());
        if n == 0 {
            self.selected = None;
            return;
        }
        let k = k.min(n - 1);
        self.selected = Some(k);
        let steps = if k + 1 == n { usize::MAX } else { k + 1 };
        if steps != self.steps {
            self.steps = steps;
            self.dirty = true;
        }
    }

    /// Colour of a net doping (cm⁻³): red for n, blue for p, deeper with the concentration
    /// (1e14 to 1e21, log).
    pub fn doping_color(net: f64) -> Color32 {
        let t = ((net.abs().max(1.0).log10() - 14.0) / 7.0).clamp(0.0, 1.0) as f32;
        let mix = |a: f32, b: f32| (a + (b - a) * t).round() as u8;
        if net > 0.0 {
            Color32::from_rgb(mix(250.0, 170.0), mix(215.0, 10.0), mix(215.0, 30.0))
        } else {
            Color32::from_rgb(mix(215.0, 20.0), mix(225.0, 60.0), mix(250.0, 170.0))
        }
    }

    fn image(&self, s: &Section, flow: &ProcessFlow) -> ColorImage {
        let colors: Vec<Color32> = s
            .materials
            .iter()
            .map(|m| {
                let c = flow.color(m);
                Color32::from_rgb(c[0], c[1], c[2])
            })
            .collect();
        let mut img = ColorImage::new([s.nx, s.nz], vec![Color32::TRANSPARENT; s.nx * s.nz]);
        let doping = self.show_doping && !s.dopants.is_empty();
        for iz in 0..s.nz {
            let row = s.nz - 1 - iz;
            for ix in 0..s.nx {
                let c = s.cells[iz * s.nx + ix];
                if c > 0 {
                    img.pixels[row * s.nx + ix] = colors[c as usize - 1];
                }
                if doping && s.is_semi(ix, iz) {
                    let net = s.doping(ix, iz).0;
                    if net != 0.0 {
                        // Junctions (sign change to the right or below) are drawn dark.
                        let flips = |jx: usize, jz: usize| s.is_semi(jx, jz) && s.doping(jx, jz).0 * net < 0.0;
                        let edge = (ix + 1 < s.nx && flips(ix + 1, iz)) || (iz > 0 && flips(ix, iz - 1));
                        img.pixels[row * s.nx + ix] = if edge { Color32::from_gray(30) } else { Self::doping_color(net) };
                    }
                }
            }
        }
        img
    }

    pub fn window(&mut self, ctx: &egui::Context, project: Option<&Project>) -> Option<ProcessAction> {
        if !self.open {
            return None;
        }
        match project {
            Some(p) => self.update(ctx, p),
            None => self.poll(),
        }
        let mut action = None;
        let mut open = self.open;
        let title = match &self.cell {
            Some(c) => format!("Process Emulation: {c}"),
            None => "Process Emulation".into(),
        };
        crate::float::tool_window(ctx, "process_emulation", &title, [860.0, 760.0], &mut open, |ui| {
            ui.horizontal(|ui| {
                let l = ui.label("Flow");
                let name = self.flow.as_ref().map_or("—".to_string(), |f| f.name.clone());
                ui.label(RichText::new(name).strong()).labelled_by(l.id);
                if ui.button("Load…").clicked() {
                    action = Some(ProcessAction::Load);
                }
                if ui.button("Generic 180 nm").clicked() {
                    self.set_text(GENERIC180.to_string());
                }
                ui.toggle_value(&mut self.show_flow, "Edit flow").on_hover_text("Edit the steps of the flow");
                if self.section.as_ref().is_some_and(|s| !s.dopants.is_empty())
                    && ui
                        .toggle_value(&mut self.show_doping, "Doping")
                        .on_hover_text("Colour silicon and poly by net doping: red n, blue p")
                        .changed()
                {
                    self.texture = None;
                }
            });
            if self.show_flow {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(340.0);
                        self.editor(ui, project);
                    });
                    ui.separator();
                    ui.vertical(|ui| self.section_view(ui, project));
                });
            } else {
                self.section_view(ui, project);
            }
        });
        self.open = open;
        if !open && let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        action
    }

    /// The flow editor: settings, the step list and the form of the selected step, or the TOML.
    fn editor(&mut self, ui: &mut egui::Ui, project: Option<&Project>) {
        let pal = crate::theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.edit_toml, false, "Steps");
            ui.selectable_value(&mut self.edit_toml, true, "TOML");
        });
        let flow = match &self.parsed {
            Ok(f) if !self.edit_toml => f.clone(),
            parsed => {
                if let Err(e) = parsed {
                    ui.colored_label(pal.danger, format!("process flow: {e}"));
                }
                let l = ui.label(RichText::new("Process flow (TOML)").small().color(pal.text_weak));
                egui::ScrollArea::vertical().id_salt("pe_flow").show(ui, |ui| {
                    let r = ui
                        .add(egui::TextEdit::multiline(&mut self.text).code_editor().desired_width(f32::INFINITY))
                        .labelled_by(l.id);
                    if r.changed() {
                        self.text_changed();
                    }
                });
                return;
            }
        };
        egui::CollapsingHeader::new("Flow settings").id_salt("pe_settings").show(ui, |ui| {
            let mut g = flow.clone();
            if settings_form(ui, &mut g) {
                self.edit(|t| process_edit::set_header(t, &g));
            }
        });
        // The step list.
        let n = flow.steps.len();
        let shown = self.steps.min(n);
        let cached = self.checkpoints();
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("pe_steps").max_height(ui.available_height() * 0.42).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (k, s) in flow.steps.iter().enumerate() {
                // Steps past the one shown are dimmed; computed ones are marked.
                let mut text = RichText::new(format!("{:>2}  {}", k + 1, s.label()));
                if k >= shown {
                    text = text.color(pal.text_weak);
                }
                let r = ui.selectable_label(self.selected == Some(k), text).on_hover_text(if cached.contains(&(k + 1)) {
                    "Computed: instant to show"
                } else {
                    "Not computed yet"
                });
                if r.clicked() {
                    clicked = Some(k);
                }
            }
        });
        if let Some(k) = clicked {
            self.select(k);
        }
        let sel = self.selected.filter(|&k| k < n);
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Add step", |ui| {
                for op in OPS {
                    if ui.button(op).clicked()
                        && let Some(s) = process_edit::new_step(op)
                    {
                        let at = sel.map_or(n, |k| k + 1);
                        self.edit(|t| process_edit::insert_step(t, at, &s));
                        self.select(at);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Insert a step after the selected one");
            if let Some(k) = sel {
                if ui.button("Duplicate").clicked() {
                    let s = flow.steps[k].clone();
                    self.edit(|t| process_edit::insert_step(t, k + 1, &s));
                    self.select(k + 1);
                }
                if ui.add_enabled(k > 0, egui::Button::new("Up")).clicked() {
                    self.edit(|t| process_edit::move_step(t, k, k - 1));
                    self.select(k - 1);
                }
                if ui.add_enabled(k + 1 < n, egui::Button::new("Down")).clicked() {
                    self.edit(|t| process_edit::move_step(t, k, k + 1));
                    self.select(k + 1);
                }
                if ui.button("Delete").clicked() {
                    self.edit(|t| process_edit::remove_step(t, k));
                    if n > 1 {
                        self.select(k.saturating_sub(1));
                    } else {
                        self.selected = None;
                    }
                }
            }
        });
        let Some(k) = sel else {
            ui.label(RichText::new("Select a step to edit it").color(pal.text_weak));
            return;
        };
        ui.separator();
        let layers: Vec<String> = project.map_or(vec![], |p| p.tech.layers.iter().map(|l| l.name.clone()).collect());
        egui::ScrollArea::vertical().id_salt("pe_step_form").show(ui, |ui| {
            let mut s = flow.steps[k].clone();
            if step_form(ui, k, &mut s, &layers) {
                self.edit(|t| process_edit::set_step(t, k, &s));
            }
        });
    }

    /// The section, with the cut and step controls above it.
    fn section_view(&mut self, ui: &mut egui::Ui, project: Option<&Project>) {
        let pal = crate::theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            let l = ui.label("Cut (µm)");
            for (name, v) in
                [("x0", &mut self.cut.x0), ("y0", &mut self.cut.y0), ("x1", &mut self.cut.x1), ("y1", &mut self.cut.y1)]
            {
                let n = ui.label(RichText::new(name).small().color(pal.text_weak));
                let r = ui.add(egui::DragValue::new(v).speed(0.01).max_decimals(3)).labelled_by(n.id).labelled_by(l.id);
                if r.changed() {
                    self.dirty = true;
                }
            }
            if ui.button("Across the diffusion").on_hover_text("Horizontal through the largest diffusion area").clicked()
                && let (Some(p), Some(c)) = (project, &self.cell)
                && let Ok(cut) = default_cut(p, c)
            {
                self.cut = cut;
                self.dirty = true;
            }
        });
        if let Ok(f) = &self.parsed {
            let n = f.steps.len();
            let mut k = self.steps.min(n);
            let what = if k == 0 { "substrate".to_string() } else { f.steps[k - 1].label() };
            ui.horizontal(|ui| {
                let l = ui.label("After step");
                let r = ui.add(egui::Slider::new(&mut k, 0..=n)).labelled_by(l.id);
                ui.label(RichText::new(what).color(pal.text_weak));
                if r.changed() {
                    self.steps = if k == n { usize::MAX } else { k };
                    self.selected = k.checked_sub(1);
                    self.dirty = true;
                }
            });
        }
        if let Some(job) = &self.job {
            let (f, text) = Self::progress(job, self.dirty);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.add(egui::ProgressBar::new(f).desired_width(220.0).show_percentage());
                ui.label(RichText::new(text).color(pal.text_weak));
                if ui.button("Cancel").on_hover_text("Stop the run, keeping the last section").clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            });
            if self.section.is_some() {
                ui.label(RichText::new("Showing the previous section until the run ends").small().color(pal.text_weak));
            }
        }
        if let Some(e) = &self.error {
            ui.colored_label(pal.danger, e);
        }
        let (Some(s), Some(f)) = (&self.section, &self.flow) else { return };
        let tex = match &self.texture {
            Some(t) => t.clone(),
            None => {
                let t = ui.ctx().load_texture("process_section", self.image(s, f), TextureOptions::NEAREST);
                self.texture = Some(t.clone());
                t
            }
        };
        let (w, h) = (s.nx as f64 * s.h, s.nz as f64 * s.h);
        let mut probe = self.probe;
        let doped = !s.dopants.is_empty();
        let text_h = if doped { 64.0 + 44.0 } else { 64.0 };
        let plot_h = (ui.available_height() - text_h).max(220.0);
        let shown = self.probe.unwrap_or(w / 2.0);
        let show_doping = self.show_doping;
        ui.horizontal(|ui| {
            let sec_w = if doped { ui.available_width() * 0.64 } else { ui.available_width() };
            Plot::new("pe_plot")
                .data_aspect(1.0)
                .width(sec_w)
                .height(plot_h)
                .legend(Legend::default().position(egui_plot::Corner::RightTop))
                .x_axis_label("µm along the cut")
                .y_axis_label("z (µm)")
                .allow_scroll(false)
                .show(ui, |pu| {
                    pu.image(PlotImage::new(
                        "",
                        tex.id(),
                        PlotPoint::new(w / 2.0, s.z0 + h / 2.0),
                        egui::vec2(w as f32, h as f32),
                    ));
                    // Legend entries: one point per material, off to the side; with the doping
                    // shown, n and p in place of the semiconductors.
                    let overlay = doped && show_doping;
                    if overlay {
                        for (name, net) in [("n-type", 1e19), ("p-type", -1e19)] {
                            pu.points(Points::new(name, vec![[w / 2.0, s.z0]]).color(Self::doping_color(net)).radius(0.0));
                        }
                    }
                    for (k, m) in s.materials.iter().enumerate() {
                        if overlay && s.semi[k] {
                            continue;
                        }
                        let c = f.color(m);
                        pu.points(
                            Points::new(m.clone(), vec![[w / 2.0, s.z0]]).color(Color32::from_rgb(c[0], c[1], c[2])).radius(0.0),
                        );
                    }
                    if (pu.response().clicked() || pu.response().dragged())
                        && let Some(pt) = pu.pointer_coordinate()
                    {
                        probe = Some(pt.x.clamp(0.0, w));
                    }
                    if let Some(x) = probe {
                        pu.vline(egui_plot::VLine::new("probe", x).color(pal.accent).width(1.0));
                    }
                });
            if doped {
                // The dopant depth profile at the probe, depth up the side like the section.
                let prof = s.profile(shown);
                let colors = [
                    Color32::from_rgb(40, 90, 220),
                    Color32::from_rgb(200, 60, 40),
                    Color32::from_rgb(220, 130, 20),
                    Color32::from_rgb(120, 60, 180),
                ];
                Plot::new("pe_profile")
                    .width(ui.available_width())
                    .height(plot_h)
                    .legend(Legend::default().position(egui_plot::Corner::RightBottom))
                    .x_axis_label(format!("cm⁻³ at x = {shown:.2} µm"))
                    .y_axis_label("z (µm)")
                    .x_axis_formatter(|m, _| {
                        if (m.value - m.value.round()).abs() < 1e-6 { format!("1e{:.0}", m.value) } else { String::new() }
                    })
                    .include_x(14.0)
                    .include_x(21.0)
                    .allow_scroll(false)
                    .show(ui, |pu| {
                        let log = |v: f64| if v > 1e10 { Some(v.log10()) } else { None };
                        for (k, (sp, _)) in s.dopants.iter().enumerate() {
                            let pts: Vec<[f64; 2]> = prof.iter().filter_map(|(z, _, c)| log(c[k]).map(|l| [l, *z])).collect();
                            pu.line(Line::new(sp.name(), PlotPoints::from(pts)).color(colors[k % colors.len()]));
                        }
                        let net: Vec<[f64; 2]> = prof.iter().filter_map(|(z, n, _)| log(n.abs()).map(|l| [l, *z])).collect();
                        pu.line(
                            Line::new("|net|", PlotPoints::from(net))
                                .color(pal.text_weak)
                                .style(egui_plot::LineStyle::dashed_dense()),
                        );
                        for z in s.junctions(shown) {
                            pu.hline(egui_plot::HLine::new("junction", z).color(pal.text_weak).width(0.5));
                        }
                    });
            }
        });
        self.probe = probe;
        let x = self.probe.unwrap_or(w / 2.0);
        let stack: Vec<String> = s.stack_at(x).iter().rev().map(|(m, t)| format!("{m} {:.0} nm", t * 1000.0)).collect();
        ui.label(
            RichText::new(format!("x = {x:.3} µm, surface {:.3} µm (click the section to probe)", s.surface_at(x)))
                .color(pal.text_weak),
        );
        ui.label(RichText::new(stack.join("  |  ")).monospace());
        if doped {
            let layers: Vec<String> = s
                .doped_layers(x)
                .iter()
                .map(|l| {
                    format!(
                        "{} {} to {:.0} nm, peak {:.1e}, {} Ω/□",
                        if l.n_type { "n" } else { "p" },
                        l.material,
                        -l.bottom * 1000.0,
                        l.peak,
                        ics_core::units::format_eng(l.sheet_resistance, "")
                    )
                })
                .collect();
            let j: Vec<String> = s.junctions(x).iter().map(|z| format!("{:.0} nm", -z * 1000.0)).collect();
            ui.label(RichText::new(layers.join("  |  ")).monospace());
            ui.label(
                RichText::new(format!("junctions at {} below z = 0", if j.is_empty() { "—".into() } else { j.join(", ") }))
                    .color(pal.text_weak),
            );
        }
    }
}

/// A text field that keeps what is typed while it has the focus and turns it into a value with
/// `parse` (`None` while it does not parse); shows `shown` otherwise.
fn buffered<T>(ui: &mut egui::Ui, id: egui::Id, label: egui::Id, shown: String, parse: impl Fn(&str) -> Option<T>) -> Option<T> {
    let mut buf: String = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| shown.clone());
    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id).desired_width(150.0)).labelled_by(label);
    let out = if r.changed() { parse(buf.trim()) } else { None };
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    out
}

/// A comma-separated list of names.
fn list(ui: &mut egui::Ui, id: egui::Id, label: &str, v: &mut Vec<String>) -> bool {
    let l = ui.label(label);
    let out = buffered(ui, id, l.id, v.join(", "), |s| {
        Some(s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect::<Vec<_>>())
    });
    ui.end_row();
    match out {
        Some(n) if n != *v => {
            *v = n;
            true
        }
        _ => false,
    }
}

fn text(ui: &mut egui::Ui, label: &str, v: &mut String) -> bool {
    let l = ui.label(label);
    let c = ui.add(egui::TextEdit::singleline(v).desired_width(150.0)).labelled_by(l.id).changed();
    ui.end_row();
    c
}

fn number(ui: &mut egui::Ui, label: &str, v: &mut f64, speed: f64, unit: &str) -> bool {
    let l = ui.label(label);
    let c = ui
        .add(egui::DragValue::new(v).speed(speed).max_decimals(4).range(0.0..=f64::MAX).suffix(unit))
        .labelled_by(l.id)
        .changed();
    ui.end_row();
    c
}

/// A number typed in scientific notation (doses, concentrations).
fn sci(ui: &mut egui::Ui, id: egui::Id, label: &str, v: &mut f64, unit: &str) -> bool {
    let l = ui.label(label);
    let out = ui
        .horizontal(|ui| {
            let out = buffered(ui, id, l.id, format!("{v:e}"), |s| s.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0));
            ui.label(unit);
            out
        })
        .inner;
    ui.end_row();
    match out {
        Some(x) if x != *v => {
            *v = x;
            true
        }
        _ => false,
    }
}

fn check(ui: &mut egui::Ui, label: &str, v: &mut bool, hint: &str) -> bool {
    ui.label("");
    let c = ui.checkbox(v, label).on_hover_text(hint).changed();
    ui.end_row();
    c
}

/// An optional number: a check box to have it, and its value.
fn optional(ui: &mut egui::Ui, label: &str, v: &mut Option<f64>, default: f64, speed: f64, unit: &str) -> bool {
    let l = ui.label(label);
    let mut c = false;
    ui.horizontal(|ui| {
        let mut on = v.is_some();
        if ui.checkbox(&mut on, "").labelled_by(l.id).changed() {
            *v = on.then_some(default);
            c = true;
        }
        if let Some(x) = v {
            c |= ui
                .add(egui::DragValue::new(x).speed(speed).max_decimals(4).range(0.0..=f64::MAX).suffix(unit))
                .labelled_by(l.id)
                .changed();
        }
    });
    ui.end_row();
    c
}

const SPECIES: [&str; 5] = ["boron", "bf2", "phosphorus", "arsenic", "antimony"];

/// Where a step acts: everywhere, where a layer expression is drawn, or where it is not.
fn mask_form(ui: &mut egui::Ui, k: usize, m: &mut Mask, layers: &[String]) -> bool {
    let mut c = false;
    let l = ui.label("Where");
    let mode = if m.mask.is_some() {
        1
    } else if m.keep.is_some() {
        2
    } else {
        0
    };
    let names = ["everywhere", "drawn", "not drawn"];
    let mut new = mode;
    egui::ComboBox::from_id_salt(("pe_mask_mode", k))
        .selected_text(names[mode])
        .show_ui(ui, |ui| {
            for (i, n) in names.iter().enumerate() {
                ui.selectable_value(&mut new, i, *n);
            }
        })
        .response
        .on_hover_text("Where the step acts: the layer mask drawn (`mask`) or not drawn (`keep`)")
        .labelled_by(l.id);
    ui.end_row();
    if new != mode {
        let e = m.mask.take().or(m.keep.take()).unwrap_or_else(|| layers.first().cloned().unwrap_or_default());
        match new {
            1 => m.mask = Some(e),
            2 => m.keep = Some(e),
            _ => {}
        }
        c = true;
    }
    if let Some(e) = m.mask.as_mut().or(m.keep.as_mut()) {
        let l = ui.label("Layers");
        ui.horizontal(|ui| {
            c |= ui
                .add(egui::TextEdit::singleline(e).desired_width(110.0))
                .labelled_by(l.id)
                .on_hover_text("A layer or a derived-layer expression, e.g. active and not poly")
                .changed();
            ui.menu_button("Pick", |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for n in layers {
                        if ui.button(n).clicked() {
                            *e = n.clone();
                            c = true;
                            ui.close();
                        }
                    }
                });
            });
        });
        ui.end_row();
    }
    c
}

/// The form of step `k`; true when it changed.
fn step_form(ui: &mut egui::Ui, k: usize, s: &mut Step, layers: &[String]) -> bool {
    let id = |f: &str| egui::Id::unique(("pe_field", k, f.to_string()));
    let mut c = false;
    ui.label(RichText::new(format!("Step {}: {}", k + 1, process_edit::op(s))).strong());
    egui::Grid::new(("pe_step_grid", k)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| match s {
        Step::Deposit { name, material, thickness, conformal } => {
            c |= text(ui, "Name", name);
            c |= text(ui, "Material", material);
            c |= number(ui, "Thickness", thickness, 0.001, " µm");
            c |= check(ui, "Conformal", conformal, "Also on sidewalls, as CVD");
        }
        Step::Etch { name, mask, materials, depth, isotropic } => {
            c |= text(ui, "Name", name);
            c |= mask_form(ui, k, mask, layers);
            c |= list(ui, id("materials"), "Materials", materials);
            c |= optional(ui, "Depth", depth, 0.1, 0.001, " µm");
            c |= check(ui, "Isotropic", isotropic, "Etch sideways as deep as down (wet etch)");
        }
        Step::Implant { name, mask, material, into, depth, species, dose, energy } => {
            c |= text(ui, "Name", name);
            c |= mask_form(ui, k, mask, layers);
            let l = ui.label("Species");
            let mut sp = species.clone();
            egui::ComboBox::from_id_salt(("pe_species", k))
                .selected_text(sp.as_deref().unwrap_or("none"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut sp, None, "none");
                    for n in SPECIES {
                        ui.selectable_value(&mut sp, Some(n.to_string()), n);
                    }
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
            if sp != *species {
                if sp.is_some() && species.is_none() {
                    *dose = dose.or(Some(1e13));
                    *energy = energy.or(Some(20.0));
                }
                if sp.is_none() {
                    (*dose, *energy) = (None, None);
                }
                *species = sp;
                c = true;
            }
            if species.is_some() {
                let mut d = dose.unwrap_or(1e13);
                if sci(ui, id("dose"), "Dose", &mut d, "cm⁻²") {
                    *dose = Some(d);
                    c = true;
                }
                let mut e = energy.unwrap_or(20.0);
                if number(ui, "Energy", &mut e, 0.5, " keV") {
                    *energy = Some(e);
                    c = true;
                }
            }
            let l = ui.label("Material");
            let mut conv = material.is_some();
            if ui
                .checkbox(&mut conv, "turn into")
                .labelled_by(l.id)
                .on_hover_text("Also turn the implanted cells into a material")
                .changed()
            {
                *material = conv.then(|| "ndiff".to_string());
                *depth = conv.then_some(depth.unwrap_or(0.1));
                if conv && into.is_empty() {
                    *into = vec!["si".into()];
                }
                if !conv {
                    into.clear();
                }
                c = true;
            }
            ui.end_row();
            if let Some(m) = material {
                c |= text(ui, "New material", m);
                c |= list(ui, id("into"), "Of", into);
                let mut d = depth.unwrap_or(0.1);
                if number(ui, "Depth", &mut d, 0.001, " µm") {
                    *depth = Some(d);
                    c = true;
                }
            }
        }
        Step::Anneal { name, temperature, time } => {
            c |= text(ui, "Name", name);
            c |= number(ui, "Temperature", temperature, 1.0, " °C");
            c |= number(ui, "Time", time, 0.1, " min");
        }
        Step::Oxidize { name, temperature, time, wet, material } => {
            c |= text(ui, "Name", name);
            c |= number(ui, "Temperature", temperature, 1.0, " °C");
            c |= number(ui, "Time", time, 0.1, " min");
            c |= check(ui, "Wet (steam)", wet, "Wet oxidation grows faster");
            c |= text(ui, "Oxide", material);
        }
        Step::Planarize { name, height, stop } => {
            c |= text(ui, "Name", name);
            let l = ui.label("Down to");
            let mut by_stop = stop.is_some();
            ui.horizontal(|ui| {
                c |= ui.radio_value(&mut by_stop, false, "height").labelled_by(l.id).changed();
                c |= ui.radio_value(&mut by_stop, true, "a material").labelled_by(l.id).changed();
            });
            ui.end_row();
            if by_stop && stop.is_none() {
                (*stop, *height) = (Some("sin".into()), None);
            } else if !by_stop && height.is_none() {
                (*stop, *height) = (None, Some(0.0));
            }
            if let Some(h) = height {
                let l = ui.label("Height");
                c |= ui.add(egui::DragValue::new(h).speed(0.001).max_decimals(4).suffix(" µm")).labelled_by(l.id).changed();
                ui.end_row();
            }
            if let Some(m) = stop {
                c |= text(ui, "Stop on", m);
            }
        }
        Step::Strip { name, material } => {
            c |= text(ui, "Name", name);
            c |= text(ui, "Material", material);
        }
    });
    c
}

/// The flow settings form; true when they changed.
fn settings_form(ui: &mut egui::Ui, f: &mut ProcessFlow) -> bool {
    let mut c = false;
    egui::Grid::new("pe_settings_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        c |= text(ui, "Name", &mut f.name);
        c |= text(ui, "Substrate", &mut f.substrate);
        c |= number(ui, "Depth shown", &mut f.substrate_depth, 0.01, " µm");
        let l = ui.label("Resolution");
        c |= ui
            .add(egui::DragValue::new(&mut f.resolution).speed(0.0005).range(0.001..=1.0).max_decimals(4).suffix(" µm"))
            .labelled_by(l.id)
            .on_hover_text("Grid step: finer is slower")
            .changed();
        ui.end_row();
        let l = ui.label("Doping");
        let mut sp = f.substrate_doping.as_ref().map(|d| d.species.clone());
        egui::ComboBox::from_id_salt("pe_sub_species")
            .selected_text(sp.as_deref().unwrap_or("none"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut sp, None, "none");
                for n in SPECIES {
                    ui.selectable_value(&mut sp, Some(n.to_string()), n);
                }
            })
            .response
            .labelled_by(l.id);
        ui.end_row();
        if sp != f.substrate_doping.as_ref().map(|d| d.species.clone()) {
            let conc = f.substrate_doping.as_ref().map_or(1e15, |d| d.concentration);
            f.substrate_doping = sp.map(|species| Doping { species, concentration: conc });
            c = true;
        }
        if let Some(d) = &mut f.substrate_doping {
            c |= sci(ui, egui::Id::unique("pe_sub_conc"), "Concentration", &mut d.concentration, "cm⁻³");
        }
        c |= list(ui, egui::Id::unique("pe_semis"), "Semiconductors", &mut f.semiconductors);
    });
    c
}
