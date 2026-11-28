//! Simulation, sweeps, Monte Carlo, verification, silicon and process windows.

use super::*;

impl IcStudio {
    pub(super) fn verify(&mut self, ctx: &egui::Context, what: Verify) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(lay) = p.cell_mut(&r).and_then(|c| c.layout.take()) else {
            self.warn(format!("{r} has no layout"));
            return;
        };
        let tech = p.tech.clone();
        let ed = self.lay_eds.entry(r.clone()).or_default();
        let msg = {
            let cx = LayCtx { cell: &r, src: p, hier: &mut self.hier, tech: &tech, cells: &[], context: None };
            ed.start_verification(what, &lay, &cx, ctx)
        };
        if let Some(c) = p.cell_mut(&r) {
            c.layout = Some(lay);
        }
        self.workspace = Workspace::Layout;
        self.info(msg);
    }

    pub(super) fn pixel_stack_from_pdk(&mut self) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else {
            self.warn("Open the pixel cell first".into());
            return;
        };
        let Some(xs) = &p.tech.cross_section else {
            self.warn(format!("Technology {} has no cross section", p.tech.name));
            return;
        };
        let boundary = p.tech.layer_index("boundary").map(|_| "boundary");
        match xs.pixel_stack_toml(&p.tech.name, &r.to_string(), boundary) {
            Ok(t) => {
                self.pixel.text = t;
                self.pixel.path = None;
                self.pixel.show();
            }
            Err(e) => self.warn(e),
        }
    }

    /// Set current source `source` of the active cell's schematic to `amps` (undoable).
    /// The device model's photodiode figures into the technology (see
    /// [`ics_core::pixel_layout::apply_device_photodiode`]).
    pub(super) fn apply_pixel_device(&mut self) {
        let Some(Ok(r)) = &self.pixel.result else {
            self.warn("Run the pixel model with a [device] section first".into());
            return;
        };
        let (Some(d), Some(src), Some(st)) = (&r.device, self.pixel.layout_source.clone(), &self.pixel.stack) else {
            self.warn("Applying to the technology needs a stack with [device] that reads a layout ([layout])".into());
            return;
        };
        let n = d.pixels.len().max(1) as f64;
        let vpin = d.pixels.iter().map(|p| p.pinning_voltage).sum::<f64>() / n;
        let cap = d.pixels.iter().map(|p| p.capacitance).sum::<f64>() / n;
        let pixels = if st.cell_layout { 4 } else { 1 };
        let Some(p) = self.project.as_mut() else { return };
        let cell = match src.cell.split_once('/') {
            Some((lib, cell)) => Some(ics_core::CellRef::new(lib, cell)),
            None => p.all_cells().find(|(_, c)| c.name == src.cell).map(|(r, _)| r),
        };
        let Some(cell) = cell else {
            self.warn(format!("no cell '{}'", src.cell));
            return;
        };
        match ics_core::pixel_layout::apply_device_photodiode(
            p,
            &cell,
            &src.param_list(),
            src.boundary.as_deref(),
            pixels,
            vpin,
            cap,
        ) {
            Ok(msg) => {
                self.mark_dirty();
                self.info(msg);
            }
            Err(e) => self.warn(e),
        }
    }

    pub(super) fn set_photocurrent(&mut self, source: &str, amps: f64) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else {
            self.warn("Open the pixel circuit's schematic first".into());
            return;
        };
        let Some(sch) = p.cell_mut(&r).and_then(|c| c.schematic.as_mut()) else {
            self.warn(format!("{r} has no schematic"));
            return;
        };
        let before = sch.clone();
        let Some(inst) = sch
            .instances
            .iter_mut()
            .find(|i| i.name.eq_ignore_ascii_case(source) && i.kind == ics_core::schematic::DeviceKind::ISource)
        else {
            self.warn(format!("{r} has no current source '{source}'"));
            return;
        };
        let value = format!("{amps:.4e}");
        inst.params.insert("dc".into(), value.clone());
        self.sch_eds.entry(r.clone()).or_default().history.push(&before);
        self.mark_dirty();
        self.info(format!("{r}: {source} = {value} A (photocurrent)"));
    }

    pub(super) fn import_signoff_results(&mut self, path: Option<PathBuf>) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else {
            self.warn("Open a layout to show sign-off results on".into());
            return;
        };
        let Some(path) =
            path.or_else(|| rfd::FileDialog::new().set_title("Sign-off results (Calibre, KLayout, netgen)").pick_file())
        else {
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return self.warn(format!("{}: {e}", path.display())),
        };
        match ics_core::signoff::import_results(&path, &text, p.tech.dbu_per_um, &r.cell) {
            Ok(rep) => {
                let msg = self.lay_eds.entry(r).or_default().show_signoff(rep);
                self.workspace = Workspace::Layout;
                self.info(msg);
            }
            Err(e) => self.warn(e),
        }
    }

    pub(super) fn run_signoff(&mut self, ctx: &egui::Context, name: Option<&str>) {
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_ref()) else { return };
        if p.tech.signoff.is_empty() {
            self.warn(format!("Technology {} has no sign-off runsets", p.tech.name));
            return;
        }
        if let Some(n) = name {
            match ics_core::signoff::find(&p.tech, n) {
                Ok(rs) => {
                    let i = p.tech.signoff.iter().position(|x| x == rs).unwrap_or(0);
                    self.lay_eds.entry(r).or_default().verify.runset = i;
                }
                Err(e) => {
                    self.warn(e);
                    return;
                }
            }
        }
        self.verify(ctx, Verify::Signoff);
    }

    pub(super) fn run_simulation(&mut self, ctx: &egui::Context) {
        let Some(r) = self.active.clone() else {
            self.warn("Open a cell to simulate".into());
            return;
        };
        if self.sims.get(&r).is_some_and(|s| s.is_running()) {
            self.warn(format!("{r} is already simulating: stop it first"));
            return;
        }
        let Some(cell) = self.active_cell() else { return };
        if cell.schematic.is_none() {
            self.warn(format!("{r} has no schematic to simulate"));
            return;
        }
        let analyses: Vec<_> = cell.sim.enabled().cloned().collect();
        if analyses.is_empty() {
            self.warn("No analyses enabled".into());
            return;
        }
        let backend = cell.sim.backend;
        let parasitics = cell.sim.parasitics && cell.layout.is_some();
        let (deck, warnings) = self.netlist_active().unwrap();
        if deck.is_empty() {
            // The hierarchy could not be flattened (a missing cell, a cell placing itself).
            self.error(format!("Cannot simulate {r}: {}", warnings.join("; ")));
            return;
        }
        for w in warnings {
            self.warn(format!("netlist: {w}"));
        }
        // Extraction can take seconds on a large layout: it runs with the simulation, on a copy.
        let pex = if parasitics { self.project.clone().map(|p| (p, r.clone())) } else { None };
        let opts = self.sim_options(backend);
        self.info(format!(
            "Simulating {r} with {} ({})",
            backend.label(),
            analyses.iter().map(|a| a.kind_name()).collect::<Vec<_>>().join(", ")
        ));
        self.sims.entry(r).or_default().start_with_parasitics(ctx, deck, analyses, opts, pex);
        self.workspace = Workspace::Simulation;
    }

    pub(super) fn run_sweep(&mut self, ctx: &egui::Context) {
        if self.sweep.is_running() {
            self.warn("A sweep is already running".into());
            return;
        }
        let Some(r) = self.active.clone() else { return };
        if self.sweep.cell.as_ref() != Some(&r)
            && let Some(c) = self.active_cell()
        {
            let s = c.sim.sweep.clone();
            self.sweep.show(&r, &s);
            self.sweep.open = false;
        }
        let mut notes = vec![];
        match self.sweep_decks(&r, &mut notes) {
            Ok((decks, analyses, backend, pex)) => {
                for n in notes {
                    self.warn(n);
                }
                let n = decks.len();
                let opts = self.sim_options(backend);
                let mc = &self.sweep.setup.monte_carlo;
                self.sweep.plan = match (mc.runs > 0, self.active_cell().and_then(|c| c.schematic.as_ref())) {
                    (true, Some(sch)) => {
                        ics_core::sweep::mc_plan(mc, sch, self.project.as_ref().and_then(|p| p.tech.variation.as_ref())).ok()
                    }
                    _ => None,
                };
                self.sweep.start(ctx, decks, analyses, opts, pex);
                self.info(format!("Sweep {r}: {n} point(s) with {}", backend.label()));
            }
            Err(e) => {
                for n in notes {
                    self.warn(n);
                }
                self.error(format!("Sweep: {e}"));
            }
        }
    }

    pub(super) fn run_monte_carlo(&mut self, ctx: &egui::Context) {
        if self.monte_carlo.cell.is_none()
            && let (Some(r), Some(c)) = (self.active.clone(), self.active_cell())
        {
            let setup = c.sim.montecarlo.clone();
            self.monte_carlo.show(&r, &setup);
        }
        let Some(r) = self.monte_carlo.cell.clone() else {
            self.warn("Open a cell with a schematic for Monte Carlo".into());
            return;
        };
        let mut notes = vec![];
        self.monte_carlo.process = self.project.as_ref().and_then(|p| p.tech.variation.clone());
        self.monte_carlo.mismatch = self.project.as_ref().and_then(|p| p.tech.mismatch.clone());
        let res = self.mc_deck(&r, &mut notes).and_then(|(deck, analyses, pex)| self.monte_carlo.start(ctx, deck, analyses, pex));
        for n in notes {
            self.warn(n);
        }
        match res {
            Ok(()) => self.info(format!("Monte Carlo {r}: {} run(s) on the built-in engine", self.monte_carlo.setup.runs)),
            Err(e) => {
                self.monte_carlo.error = Some(e.clone());
                self.error(format!("Monte Carlo: {e}"));
            }
        }
    }

    /// The netlist of cell `r` (with layout parasitics when its setup asks) and its enabled analyses.
    #[allow(clippy::type_complexity)]
    pub(super) fn mc_deck(
        &self,
        r: &CellRef,
        notes: &mut Vec<String>,
    ) -> Result<(String, Vec<ics_core::Analysis>, Option<(Project, CellRef)>), String> {
        let p = self.project.as_ref().ok_or("no project")?;
        let cell = p.cell(r).ok_or_else(|| format!("no cell {r}"))?;
        let circuit = p.circuit(r)?;
        let sch = &*circuit;
        let analyses: Vec<_> = cell.sim.enabled().cloned().collect();
        if analyses.is_empty() {
            return Err("no analyses enabled".into());
        }
        let nl = ics_core::netlist::netlist_for(sch, &p.tech, &cell.sim.corner);
        notes.extend(nl.warnings.iter().map(|w| format!("netlist: {w}")));
        let deck = nl.deck(&r.to_string(), &cell.sim);
        // The parasitics are extracted by the run in the background, on a copy.
        let pex = (cell.sim.parasitics && cell.layout.is_some()).then(|| (p.clone(), r.clone()));
        Ok((deck, analyses, pex))
    }

    pub(super) fn run_verification(&mut self, ctx: &egui::Context) {
        if self.verification.is_running() {
            return self.warn("Verification is already running".into());
        }
        let Some(p) = self.project.as_ref() else { return };
        let tests = p.settings.verification.resolve(p);
        if tests.is_empty() {
            return self.warn("No tests: give a cell specs in its Corner / Sweep Manager, or add cells to the plan".into());
        }
        let root = p.root.clone();
        let snapshot = p.clone();
        let opts = self.sim_options(ics_core::Backend::Auto);
        if !self.verification.open {
            self.verification.show(root.as_deref());
        }
        let names: Vec<&str> = tests.iter().map(|(n, _)| n.as_str()).collect();
        self.info(format!("Verification: running {} test(s): {}", tests.len(), names.join(", ")));
        self.verification.start(ctx, snapshot, tests, opts);
    }

    pub(super) fn verification_window(&mut self, ctx: &egui::Context) {
        let active = self.active.clone();
        match self.verification.window(ctx, self.project.as_ref(), active.as_ref()) {
            Some(crate::verify_view::VerifyAction::Plan(plan)) => {
                if let Some(p) = self.project.as_mut() {
                    p.settings.verification = plan;
                    self.mark_dirty();
                }
            }
            Some(crate::verify_view::VerifyAction::Run) => self.queue.push(Command::RunVerification),
            None => {}
        }
    }

    pub fn verification(&mut self) -> &mut crate::verify_view::Verification {
        &mut self.verification
    }

    pub(super) fn monte_carlo_window(&mut self, ctx: &egui::Context) {
        if self.monte_carlo.open {
            let v = self.project.as_ref().and_then(|p| p.tech.variation.as_ref());
            if self.monte_carlo.process.as_ref() != v {
                self.monte_carlo.process = v.cloned();
            }
            let m = self.project.as_ref().and_then(|p| p.tech.mismatch.as_ref());
            if self.monte_carlo.mismatch.as_ref() != m {
                self.monte_carlo.mismatch = m.cloned();
            }
        }
        let (changed, run, stop) = self.monte_carlo.window(ctx);
        if let Some(s) = changed
            && let Some(c) = self.monte_carlo.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r))
        {
            c.sim.montecarlo = s;
            self.mark_dirty();
        }
        if run {
            self.queue.push(Command::RunMonteCarlo);
        }
        if stop {
            self.queue.push(Command::StopMonteCarlo);
        }
    }

    pub fn monte_carlo(&mut self) -> &mut crate::mc_view::MonteCarlo {
        &mut self.monte_carlo
    }

    pub fn silicon(&mut self) -> &mut crate::silicon_view::SiliconView {
        &mut self.silicon
    }

    pub(super) fn run_silicon(&mut self, ctx: &egui::Context, fit: bool) {
        let Some(p) = self.project.clone() else { return };
        let backend = self.silicon.cell.as_ref().and_then(|r| p.cell(r)).map_or(ics_core::Backend::Builtin, |c| c.sim.backend);
        let opts = self.sim_options(backend);
        match self.silicon.start(ctx, p, fit, opts) {
            Ok(()) => self.info(format!("Silicon correlation: {}", if fit { "fitting" } else { "comparing" })),
            Err(e) => {
                self.silicon.error = Some(e.clone());
                self.error(format!("Silicon correlation: {e}"));
            }
        }
    }

    pub fn process_emulation(&mut self) -> &mut crate::process_view::ProcessView {
        &mut self.process
    }

    pub(super) fn process_window(&mut self, ctx: &egui::Context) {
        if let Some(crate::process_view::ProcessAction::Load) = self.process.window(ctx, self.project.as_ref())
            && let Some(path) = rfd::FileDialog::new().set_title("Process flow").add_filter("Process flow", &["toml"]).pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(t) => self.process.set_text(t),
                Err(e) => self.error(format!("{}: {e}", path.display())),
            }
        }
    }

    pub fn process_data(&mut self) -> &mut crate::process_data_view::ProcessDataView {
        &mut self.process_data
    }

    pub(super) fn process_data_window(&mut self, ctx: &egui::Context) {
        use crate::process_data_view::ProcessDataAction;
        let action = self.process_data.window(ctx);
        match &action {
            Some(ProcessDataAction::ExtractCorners) => {
                let p = self.project.clone().unwrap_or_else(ics_core::examples::demo_project);
                match self.process_data.start_corners(ctx, p) {
                    Ok(()) => self.info("Corner models: fitting".into()),
                    Err(e) => self.error(format!("Corner models: {e}")),
                }
                return;
            }
            Some(ProcessDataAction::ApplyVariation(v)) => {
                let Some(p) = self.project.as_mut() else {
                    self.warn("Open a project to store the process variation in its technology".into());
                    return;
                };
                p.tech.variation = Some(v.clone());
                self.mark_dirty();
                self.ok(format!(
                    "Process variation: {} parameter(s) from {} stored in the technology; Monte Carlo draws them",
                    v.params.len(),
                    v.source
                ));
                return;
            }
            Some(ProcessDataAction::ApplyMismatch(m)) => {
                let Some(p) = self.project.as_mut() else {
                    self.warn("Open a project to store the mismatch in its technology".into());
                    return;
                };
                p.tech.mismatch = Some(m.clone());
                self.mark_dirty();
                let txt: Vec<String> = m.devices.iter().map(|d| format!("{} Avt {:.2} mV·µm", d.device, d.avt * 1e9)).collect();
                self.ok(format!("Mismatch: {} stored in the technology; Monte Carlo draws it", txt.join(", ")));
                return;
            }
            Some(ProcessDataAction::LoadPairs) => {
                let mut dialog = rfd::FileDialog::new().set_title("Matched-pair data").add_filter("CSV", &["csv", "txt", "tsv"]);
                if let Some(r) = self.project.as_ref().and_then(|p| p.root.clone()) {
                    dialog = dialog.set_directory(r);
                }
                if let Some(path) = dialog.pick_file() {
                    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    match std::fs::read_to_string(&path) {
                        Ok(t) => self.process_data.set_pairs_text(&t, &name),
                        Err(e) => self.error(format!("{}: {e}", path.display())),
                    }
                }
                return;
            }
            Some(ProcessDataAction::SaveLibrary(text)) => {
                if let Some(path) = rfd::FileDialog::new().set_title("Corner library").set_file_name("corners.lib").save_file() {
                    match std::fs::write(&path, text) {
                        Ok(()) => self.ok(format!("Corner models: wrote {}", path.display())),
                        Err(e) => self.error(format!("{}: {e}", path.display())),
                    }
                }
                return;
            }
            _ => {}
        }
        let (title, filter, exts): (&str, &str, &[&str]) = match action {
            Some(ProcessDataAction::LoadData) => ("WAT / PCM data", "CSV", &["csv", "txt", "tsv"]),
            Some(ProcessDataAction::LoadSpec) => ("Process specification", "TOML", &["toml"]),
            _ => return,
        };
        let mut dialog = rfd::FileDialog::new().set_title(title).add_filter(filter, exts);
        if let Some(r) = self.project.as_ref().and_then(|p| p.root.clone()) {
            dialog = dialog.set_directory(r);
        }
        let Some(path) = dialog.pick_file() else { return };
        let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        match std::fs::read_to_string(&path) {
            Ok(t) if matches!(action, Some(ProcessDataAction::LoadData)) => self.process_data.set_data_text(&t, &name),
            Ok(t) => self.process_data.set_spec_text(&t, &name),
            Err(e) => self.error(format!("{}: {e}", path.display())),
        }
    }

    pub(super) fn silicon_window(&mut self, ctx: &egui::Context) {
        use crate::silicon_view::SiliconAction;
        match self.silicon.window(ctx) {
            Some(SiliconAction::Changed(s)) => {
                if let Some(c) = self.silicon.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                    c.sim.silicon = s;
                    self.mark_dirty();
                }
            }
            Some(SiliconAction::LoadData) => {
                let root = self.project.as_ref().and_then(|p| p.root.clone());
                let mut dialog = rfd::FileDialog::new().set_title("Measured data").add_filter("CSV", &["csv", "txt", "tsv"]);
                if let Some(r) = &root {
                    dialog = dialog.set_directory(r);
                }
                if let Some(path) = dialog.pick_file() {
                    let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).map_or(path.clone(), |p| p.to_path_buf());
                    self.silicon.setup.data = rel.to_string_lossy().into_owned();
                    match self.silicon.load_data(root.as_deref()) {
                        Ok(()) => {
                            let s = self.silicon.setup.clone();
                            if let Some(c) = self.silicon.cell.clone().and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                                c.sim.silicon = s;
                                self.mark_dirty();
                            }
                        }
                        Err(e) => {
                            self.silicon.error = Some(e.clone());
                            self.error(e);
                        }
                    }
                }
            }
            Some(SiliconAction::Compare) => self.queue.push(Command::CompareSilicon),
            Some(SiliconAction::Fit) => self.queue.push(Command::FitSilicon),
            Some(SiliconAction::Apply(r, values)) => {
                if let Some(sch) = self.project.as_mut().and_then(|p| p.cell_mut(&r)).and_then(|c| c.schematic.as_mut()) {
                    for (t, x) in &values {
                        if let Some((inst, param)) = t.split_once('.')
                            && let Some(i) = sch.instances.iter_mut().find(|i| i.name.eq_ignore_ascii_case(inst))
                        {
                            i.params.insert(param.to_ascii_lowercase(), ics_core::optimize::spice_number(*x));
                        }
                    }
                    self.mark_dirty();
                    self.ok(format!("Silicon correlation: applied {} parameter(s) to {r}", values.len()));
                }
            }
            None => {}
        }
    }

    /// The decks of every point of the sweep draft on cell `r`.
    #[allow(clippy::type_complexity)]
    pub(super) fn sweep_decks(&self, r: &CellRef, notes: &mut Vec<String>) -> Result<SweepDecks, String> {
        let p = self.project.as_ref().ok_or("no project")?;
        let cell = p.cell(r).ok_or_else(|| format!("no cell {r}"))?;
        let circuit = p.circuit(r)?;
        let sch = &*circuit;
        let mut setup = cell.sim.clone();
        setup.sweep = self.sweep.setup.clone();
        let analyses: Vec<_> = setup.enabled().cloned().collect();
        if analyses.is_empty() {
            return Err("no analyses enabled".into());
        }
        let points = setup.sweep.expand(&setup, sch, p.tech.variation.as_ref())?;
        // The parasitics are extracted by the sweep in the background, on a copy.
        let pex = (setup.parasitics && cell.layout.is_some()).then(|| (p.clone(), r.clone()));
        let mut decks = Vec::with_capacity(points.len());
        for (k, pt) in points.into_iter().enumerate() {
            let (deck, w) = ics_core::sweep::point_deck(sch, &p.tech, &r.to_string(), &setup, &pt)?;
            if k == 0 {
                notes.extend(w.into_iter().map(|w| format!("netlist: {w}")));
            }
            decks.push((pt, deck));
        }
        Ok((decks, analyses, setup.backend, pex))
    }

    pub(super) fn sweep_window(&mut self, ctx: &egui::Context) {
        match self.sweep.window(ctx, self.project.as_ref()) {
            Some(SweepAction::Changed(s)) => {
                let r = self.sweep.cell.clone();
                if let Some(c) = r.and_then(|r| self.project.as_mut()?.cell_mut(&r)) {
                    c.sim.sweep = s;
                    self.mark_dirty();
                }
            }
            Some(SweepAction::Run) => {
                if let Some(r) = self.sweep.cell.clone() {
                    self.queue.push(Command::OpenCell(r, None));
                    self.queue.push(Command::RunSweep);
                }
            }
            None => {}
        }
    }

    pub fn sweep_manager(&mut self) -> &mut SweepManager {
        &mut self.sweep
    }
}
